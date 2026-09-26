// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
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
	d, err := daemon.Start(daemon.Options{
		Binary:           os.Getenv("PARITY_ORACLE"),
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
			text, labels := infoIdentity(t, d.Addr, "/host/"+child+"/api/v1/info", false)
			sorted, _ := json.Marshal(labels)
			return text + "\n" + string(sorted)
		})
		compare(child+" contexts", func(d *daemon.Daemon) string {
			return member(t, d, "/host/"+child+"/api/v1/contexts", "contexts")
		})
		addrs := [2]string{p.Oracle.Addr, p.Candidate.Addr}
		compareStreamPath(t, "archived", addrs, "/api/v3/stream_path?nodes="+child, times,
			[2]string{parentIdentity.Hostname, ""})
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
	t.Run("guid-change", func(t *testing.T) {
		// a new name too, so that the old localhost's name means only the archived host
		id := parentIdentity
		id.Hostname = "parity-newparent"
		id.MachineGUID = "5a1e0000-0000-4000-8000-0000000000ab"
		compareArchived(t, StartPair(t, opts, id), childHost.Hostname, parentIdentity.Hostname)
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
