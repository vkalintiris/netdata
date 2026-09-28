// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// seedFromOracle runs the C agent with the identity while a fake child (stored in childMode) streams two charts and
// a host label, long enough for the metadata sync to store them, then stops it and returns its cache directory: a
// C-written netdata-meta.db that knows the child, and the dbengine files.
func seedFromOracle(t *testing.T, id daemon.Identity, childMode string) string {
	t.Helper()
	return seedFrom(t, os.Getenv("PARITY_ORACLE"), id, childMode)
}

// seedFrom is seedFromOracle with the agent binary given.
func seedFrom(t *testing.T, binary string, id daemon.Identity, childMode string) string {
	t.Helper()
	d, err := daemon.Start(daemon.Options{
		Binary:           binary,
		RunDir:           runDir(t, Role("seed")),
		Identity:         &id,
		StorageTiers:     1,
		StreamMemoryMode: childMode,
	})
	if err != nil {
		t.Fatalf("seed: %v", err)
	}
	t.Cleanup(func() { _ = d.Stop() })
	conn, err := stream.Connect(d.Addr, d.StreamKey, childHost, stream.CapsLive)
	if err != nil {
		t.Fatalf("seed: %v", err)
	}
	conn.Linef(`LABEL "role" = 1 "seeded"`)
	conn.Linef("OVERWRITE labels")
	now := time.Now().Unix()
	for _, chart := range []string{"seed.one", "seed.two"} {
		conn.Linef("CHART '%s' '' 'title' 'units' 'family' '%s' line 1000 1 '' fixture-pusher corpus", chart, chart)
		conn.Linef("DIMENSION 'd1' '' absolute 1 1 ''")
		for i := int64(3); i >= 1; i-- {
			conn.Linef("BEGIN2 '%s' 1 %d #", chart, now-i)
			conn.Linef("SET2 'd1' %d %d A", i, i)
			conn.Linef("END2")
		}
	}
	if err := conn.Flush(); err != nil {
		t.Fatalf("seed: %v", err)
	}
	// the metadata sync stores hosts every 5 seconds
	time.Sleep(7 * time.Second)
	conn.Close()
	time.Sleep(time.Second)
	if err := d.Stop(); err != nil {
		t.Fatalf("seed: stop: %v", err)
	}
	return filepath.Join(d.Opts.RunDir, "cache")
}

// archivedHostsView is what /api/v1/info says about the hosts.
func archivedHostsView(t *testing.T, d *daemon.Daemon) string {
	t.Helper()
	b, err := rawExchange(d.Addr, []byte("GET /api/v1/info HTTP/1.1\r\n\r\n"), 10*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	var doc map[string]any
	if err := json.Unmarshal(httpBody(b), &doc); err != nil {
		t.Fatal(err)
	}
	out, _ := json.Marshal(map[string]any{
		"hosts-available":       doc["hosts-available"],
		"mirrored_hosts":        doc["mirrored_hosts"],
		"mirrored_hosts_status": doc["mirrored_hosts_status"],
	})
	return string(out)
}

// member is one member of a JSON answer, re-encoded.
func member(t *testing.T, d *daemon.Daemon, path, name string) string {
	t.Helper()
	b, err := rawExchange(d.Addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 10*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	var doc map[string]any
	if err := json.Unmarshal(httpBody(b), &doc); err != nil {
		t.Fatalf("%s: %v: %s", path, err, httpBody(b))
	}
	out, _ := json.Marshal(doc[name])
	return string(out)
}

// archivedRecords are the main-thread records of the archived hosts' load.
var archivedRecords = regexp.MustCompile(`msg="(Creating archived hosts|Created \d+ archived hosts|ACLK sync initialization completed|Host '[^']*' \(at registry as|[A-Za-z ]+ ephemeral hostname)`)

func compareArchived(t *testing.T, p *Pair, children ...string) {
	t.Helper()
	compareArchivedTimes(t, p, entryTimes, children...)
}

// compareArchivedTimes is compareArchived with the stream path's times hidden by times.
func compareArchivedTimes(t *testing.T, p *Pair, times *regexp.Regexp, children ...string) {
	t.Helper()
	compare := func(name string, get func(d *daemon.Daemon) string) {
		t.Helper()
		if o, c := get(p.Oracle), get(p.Candidate); o != c {
			t.Errorf("%s:\noracle:    %s\ncandidate: %s", name, o, c)
		}
	}
	compare("hosts", func(d *daemon.Daemon) string { return archivedHostsView(t, d) })
	compare("charts hosts", func(d *daemon.Daemon) string { return member(t, d, "/api/v1/charts", "hosts") })
	for _, child := range children {
		// an archived host's labels come from the database on both sides, C's _hw_* ones included
		compare(child+" info", func(d *daemon.Daemon) string {
			text, labels := infoIdentity(t, d.Addr, "/host/"+child+"/api/v1/info")
			sorted, _ := json.Marshal(labels)
			return text + "\n" + string(sorted)
		})
		compare(child+" contexts", func(d *daemon.Daemon) string {
			return member(t, d, "/host/"+child+"/api/v1/contexts", "contexts")
		})
		// what a child asks before it connects: an archived host (no receiver since the start) reads "archived"
		compare(child+" stream_info", func(d *daemon.Daemon) string {
			uid := strings.Trim(member(t, d, "/host/"+child+"/api/v1/info", "uid"), `"`)
			b, err := rawExchange(d.Addr, []byte("GET /api/v3/stream_info?machine_guid="+uid+" HTTP/1.1\r\n\r\n"),
				10*time.Second)
			if err != nil {
				t.Fatal(err)
			}
			return string(maskStreamInfo(httpBody(b), false, 0, 0))
		})
		addrs := [2]string{p.Oracle.Addr, p.Candidate.Addr}
		compareStreamPath(t, "archived", addrs, "/api/v3/stream_path?nodes="+child, times)
	}
	compare("records", func(d *daemon.Daemon) string {
		var out []string
		for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
			if threadOf(l) == "" && archivedRecords.MatchString(l) {
				out = append(out, normalizeLog(l, d.Opts.RunDir, ""))
			}
		}
		return strings.Join(out, "\n")
	})
}

// TestArchivedHosts starts both daemons on copies of one C-written cache that knows a child (check
// `sqlite.archived-hosts`): the child is an archived host, as C shows it in /api/v1/info, /api/v1/charts, the child's
// own info, contexts and stream path, with C's records. Then with a new machine GUID (the old localhost becomes an
// archived child too), and with the child connecting again in the archived host's memory mode and in another.
func TestArchivedHosts(t *testing.T) {
	seed := seedFromOracle(t, parentIdentity, "ram")
	opts := daemon.Options{DBMode: "alloc", StorageTiers: 1, StreamMemoryMode: "alloc", SeedCache: seed,
		LogsExtra: "    level = debug\n"}
	t.Run("seeded", func(t *testing.T) {
		p := StartPair(t, opts, parentIdentity)
		compareArchived(t, p, childHost.Hostname)
		// the whole daemon log: the context load's pool and CTXLOAD records, METASYNC, the exit (the access log's
		// sizes differ on endpoints not fully ported)
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		compareLogFiles(t, p, "daemon.log")
	})
	// a new name too, so that the old localhost's name means only the archived host
	changed := parentIdentity
	changed.Hostname = "parity-newparent"
	changed.MachineGUID = "5a1e0000-0000-4000-8000-0000000000ab"
	t.Run("guid-change", func(t *testing.T) {
		compareArchived(t, StartPair(t, opts, changed), childHost.Hostname, parentIdentity.Hostname)
	})
	// the same after a Rust-only run: its localhost's pulse charts keep the old localhost through C's startup
	// cleanup, as C's do (D61.7)
	t.Run("guid-change-rust-seed", func(t *testing.T) {
		o := opts
		o.SeedCache = seedFrom(t, os.Getenv("PARITY_CANDIDATE"), parentIdentity, "ram")
		p := StartPair(t, o, changed)
		compareArchived(t, p, childHost.Hostname, parentIdentity.Hostname)
		for _, side := range p.Each() {
			if view := archivedHostsView(t, side.Daemon); !strings.Contains(view, parentIdentity.Hostname) {
				t.Errorf("%s: the old localhost is not an archived host: %s", side.Role, view)
			}
		}
	})
	for _, mode := range []string{"alloc", "ram"} {
		t.Run("reconnect-"+mode, func(t *testing.T) {
			o := opts
			o.StreamMemoryMode = mode
			p := StartPair(t, o, parentIdentity)
			for _, side := range p.Each() {
				conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsLive)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				conn.Linef("CHART 'seed.one' '' 'title' 'units' 'family' 'seed.one' line 1000 1 '' fixture-pusher corpus")
				conn.Linef("DIMENSION 'd1' '' absolute 1 1 ''")
				if err := conn.Flush(); err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				time.Sleep(time.Second)
				t.Cleanup(func() { _ = conn.Close() })
			}
			if o, c := archivedHostsView(t, p.Oracle), archivedHostsView(t, p.Candidate); o != c {
				t.Errorf("hosts:\noracle:    %s\ncandidate: %s", o, c)
			}
			reconnect := regexp.MustCompile(`msg="(Archived host '|Host '[^']*' has |Host [^ ]+ is not in archived mode anymore)`)
			var got [2]string
			for i, side := range p.Each() {
				for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
					if reconnect.MatchString(l) {
						got[i] += fmt.Sprintln(normalizeLog(l, side.Daemon.Opts.RunDir, ""))
					}
				}
			}
			if got[0] != got[1] {
				t.Errorf("reconnect records:\noracle:\n%s\ncandidate:\n%s", got[0], got[1])
			}
		})
	}
}

// TestArchivedHostsDbengine (check `sqlite.archived-hosts`) starts both daemons in dbengine mode on copies of a
// C-written cache whose child C stored in dbengine: the child is an archived dbengine host whose contexts load from
// the metadata databases with retention from the engine's registry, as C shows them; the whole daemon log (the
// engine's start, the context loads' records) and the context cleanup rows are C's.
func TestArchivedHostsDbengine(t *testing.T) {
	seed := seedFromOracle(t, parentIdentity, "dbengine")
	// a stored context without charts: the load deletes it and queues its cleanup, which METASYNC stores
	execDB(t, filepath.Join(seed, "context-meta.db"), "INSERT INTO context (host_id, id, version, title, "+
		"chart_type, unit, priority, first_time_t, last_time_t, deleted, family) VALUES ("+
		hexID(childHost.MachineGUID)+", 'seed.orphan', 1, 't', 'line', 'u', 1, 1, 2, 0, 'f')")
	opts := daemon.Options{StorageTiers: 1, StreamMemoryMode: "dbengine", SeedCache: seed,
		LogsExtra: "    level = debug\n"}
	p := StartPair(t, opts, parentIdentity)
	compareArchived(t, p, childHost.Hostname)
	// METASYNC's first store job runs 6 s after it starts and its context cleanup scan 5 s after the job reached the
	// host: both are waited for, where a fixed wait raced each agent's startup time
	scanned := "Verified the contexts of host " + childHost.Hostname
	deadline := time.Now().Add(30 * time.Second)
	for _, side := range p.Each() {
		for !slices.ContainsFunc(logLines(t, side.Daemon.Opts.RunDir, "daemon.log"), func(l string) bool {
			return strings.Contains(l, scanned)
		}) {
			if time.Now().After(deadline) {
				t.Fatalf("%s: no context cleanup scan of the child", side.Role)
			}
			time.Sleep(500 * time.Millisecond)
		}
	}
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("stop %s: %v", side.Role, err)
		}
	}
	compareLogFiles(t, p, "daemon.log")
	var cleanup [2]string
	for i, side := range p.Each() {
		cleanup[i] = dumpDB(t, filepath.Join(side.Daemon.Opts.RunDir, "cache", "netdata-meta.db"),
			"--table", "ctx_metadata_cleanup", "--mask", "ctx_metadata_cleanup.date_created")
	}
	if cleanup[0] != cleanup[1] {
		t.Errorf("ctx_metadata_cleanup:\noracle:\n%s\ncandidate:\n%s", cleanup[0], cleanup[1])
	}
	if !strings.Contains(cleanup[1], "seed.orphan") {
		t.Errorf("the chartless context's cleanup was not stored:\n%s", cleanup[1])
	}
	// The child connects again with replication: the archived dbengine host comes back (no discard), asks for the
	// points after the registry's last time, and stores them under the dimensions it had (check C5, D70.7).
	t.Run("reconnect-dbengine", func(t *testing.T) {
		p := StartPair(t, opts, parentIdentity)
		// an archived host has no charts: its registry's last time comes through its contexts
		var last struct {
			Contexts map[string]struct {
				LastTime int64 `json:"last_time_t"`
			} `json:"contexts"`
		}
		b, err := rawExchange(p.Oracle.Addr,
			[]byte("GET /host/"+childHost.Hostname+"/api/v1/contexts HTTP/1.1\r\n\r\n"), 10*time.Second)
		if err != nil || json.Unmarshal(httpBody(b), &last) != nil {
			t.Fatalf("the archived child's contexts: %v: %s", err, httpBody(b))
		}
		l0 := last.Contexts["seed.one"].LastTime
		if l0 == 0 {
			t.Fatalf("the archived child has no retention: %s", httpBody(b))
		}
		now := time.Now().Unix()
		charts := map[string]stream.ReplayChart{"seed.one": {FirstT: l0 - 3, LastT: l0 + 5, UpdateEvery: 1},
			"seed.two": {FirstT: l0 - 3, LastT: l0 + 5, UpdateEvery: 1}}
		var windows [2][]string
		for i, side := range p.Each() {
			conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsReplication)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			t.Cleanup(func() { _ = conn.Close() })
			for _, chart := range []string{"seed.one", "seed.two"} {
				conn.Linef("CHART '%s' '' 'title' 'units' 'family' '%s' line 1000 1 '' fixture-pusher corpus", chart, chart)
				conn.Linef("DIMENSION 'd1' '' absolute 1 1 ''")
				conn.ChartDefinitionEnd(l0-3, l0+5, now)
			}
			var mu sync.Mutex
			_, err = conn.ServeReplication(charts, now, func(chart string, after, before int64) []stream.ReplayRow {
				mu.Lock()
				windows[i] = append(windows[i], fmt.Sprintf("%s (%d, %d]", chart, after-l0, before-l0))
				mu.Unlock()
				var rows []stream.ReplayRow
				for t := after + 1; t <= before; t++ {
					rows = append(rows, stream.ReplayRow{T: t, Dims: []stream.ReplayValue{
						{ID: "d1", Collected: strconv.FormatInt(t%100, 10), Flags: stream.FlagNotAnomalous}}})
				}
				return rows
			}, 30*time.Second)
			if err != nil {
				t.Fatalf("%s: replication: %v", side.Role, err)
			}
		}
		sort.Strings(windows[0])
		sort.Strings(windows[1])
		if strings.Join(windows[0], "; ") != strings.Join(windows[1], "; ") {
			t.Errorf("replication windows (relative to the registry's last time):\noracle:    %v\ncandidate: %v",
				windows[0], windows[1])
		}
		if len(windows[0]) == 0 || !strings.HasPrefix(windows[0][0], "seed.one (0, ") {
			t.Errorf("oracle: replication does not start at the registry's last time: %v", windows[0])
		}
		for _, side := range p.Each() {
			waitChartsLast(t, side.Daemon, childHost.Hostname, "seed.", 2, l0+5, 30*time.Second)
		}
		if o, c := archivedHostsView(t, p.Oracle), archivedHostsView(t, p.Candidate); o != c {
			t.Errorf("hosts:\noracle:    %s\ncandidate: %s", o, c)
		}
		rules := dbengineReadRules
		rules.Masks = append(append([]Mask{}, dbengineReadRules.Masks...),
			Mask{Pattern: "**.contexts_hard_hash", Reason: "context events"})
		compareGetWith(t, p, fmt.Sprintf("/host/%s/api/v3/data?contexts=seed.*&after=%d&before=%d&points=7",
			childHost.Hostname, l0-2, l0+5), rules)
		// the query created the points-generated chart: both sides' metadata then compare whole, localhost's rows in
		// their natural order, so a pulse dimension given a new UUID shows as an added row (R29 B2)
		waitLocalCharts(t, p, 10*time.Second, "netdata.db_points_results")
		compareFiles(t, p, writerArgs(true)...)
		reconnect := regexp.MustCompile(`msg="(Archived host '|Host '[^']*' has |Host [^ ]+ is not in archived mode anymore)`)
		var got [2]string
		for i, side := range p.Each() {
			for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
				if reconnect.MatchString(l) {
					got[i] += fmt.Sprintln(normalizeLog(l, side.Daemon.Opts.RunDir, ""))
				}
			}
		}
		if got[0] != got[1] || strings.Contains(got[1], "Discarding archived state") {
			t.Errorf("reconnect records:\noracle:\n%s\ncandidate:\n%s", got[0], got[1])
		}
		// the child keeps its dimensions: no new UUIDs (localhost may add the pulse charts created as their data came,
		// so its rows are compared apart)
		localhost := strings.ReplaceAll(parentIdentity.MachineGUID, "-", "")
		dims := []string{"--table", "dimension", "--skip-host-charts", localhost}
		seeded := dumpDB(t, filepath.Join(seed, "netdata-meta.db"), dims...)
		for _, side := range p.Each() {
			if got := dumpDB(t, filepath.Join(side.Daemon.Opts.RunDir, "cache", "netdata-meta.db"),
				dims...); got != seeded {
				t.Errorf("%s: the dimension rows changed:\n%s", side.Role, firstDifference([]byte(seeded), []byte(got)))
			}
		}
		// and every seeded localhost dimension row survives as it was: no pulse dimension's UUID replaced
		all := []string{"--table", "dimension"}
		seededAll := strings.Split(dumpDB(t, filepath.Join(seed, "netdata-meta.db"), all...), "\n")
		rowCount := func(lines []string) int {
			n := 0
			for _, l := range lines {
				if strings.HasPrefix(l, "row dimension") {
					n++
				}
			}
			return n
		}
		if rowCount(seededAll) <= rowCount(strings.Split(seeded, "\n")) {
			t.Fatalf("the seed has no localhost dimension rows")
		}
		for _, side := range p.Each() {
			rows := map[string]bool{}
			for _, l := range strings.Split(dumpDB(t, filepath.Join(side.Daemon.Opts.RunDir, "cache",
				"netdata-meta.db"), all...), "\n") {
				rows[l] = true
			}
			for _, l := range seededAll {
				if strings.HasPrefix(l, "row dimension") && !rows[l] {
					t.Errorf("%s: a seeded dimension row is gone or changed: %s", side.Role, l)
				}
			}
		}
	})
}
