// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// The maintenance check's children: A stays and loses a dimension and a chart, B (alloc) and E (dbengine, ephemeral)
// disconnect and are cleaned up.
var (
	maintA = stream.HostInfo{Hostname: "maint-a", MachineGUID: "b6b6b6b6-2222-4222-8222-00000000000a"}
	maintB = stream.HostInfo{Hostname: "maint-b", MachineGUID: "b6b6b6b6-2222-4222-8222-00000000000b"}
	maintE = stream.HostInfo{Hostname: "maint-e", MachineGUID: "b6b6b6b6-2222-4222-8222-00000000000e"}
)

// liveChild streams its charts' dimensions every second until it is closed.
type liveChild struct {
	conn *stream.Conn
	mu   sync.Mutex
	// chart id → dimension ids still collected
	charts map[string][]string
	stop   chan struct{}
	done   chan struct{}
}

func startLiveChild(t *testing.T, d *daemon.Daemon, host stream.HostInfo, labels [][2]string,
	charts map[string][]string) *liveChild {
	t.Helper()
	conn, err := stream.Connect(d.Addr, d.StreamKey, host, stream.CapsLive)
	if err != nil {
		t.Fatalf("%s: %v", host.Hostname, err)
	}
	for _, l := range labels {
		conn.Linef("LABEL '%s' 1 '%s'", l[0], l[1])
	}
	conn.Linef("OVERWRITE labels")
	ids := make([]string, 0, len(charts))
	for id := range charts {
		ids = append(ids, id)
	}
	slices.Sort(ids)
	for _, id := range ids {
		conn.Linef("CHART '%s' '' 'title' 'units' 'family' '%s' line 1000 1 '' maint corpus", id, id)
		for _, dim := range charts[id] {
			conn.Linef("DIMENSION '%s' '' absolute 1 1 ''", dim)
		}
	}
	c := &liveChild{conn: conn, charts: charts, stop: make(chan struct{}), done: make(chan struct{})}
	go func() {
		defer close(c.done)
		tick := time.NewTicker(time.Second)
		defer tick.Stop()
		for {
			c.mu.Lock()
			now := time.Now().Unix()
			for _, id := range ids {
				dims := c.charts[id]
				if len(dims) == 0 {
					continue
				}
				conn.Linef("BEGIN2 '%s' 1 %d #", id, now)
				for _, dim := range dims {
					conn.Linef("SET2 '%s' %d %d A", dim, now%100, now%100)
				}
				conn.Linef("END2")
			}
			err := conn.Flush()
			c.mu.Unlock()
			if err != nil {
				return
			}
			select {
			case <-c.stop:
				return
			case <-tick.C:
			}
		}
	}()
	t.Cleanup(c.close)
	return c
}

// obsolete sends the line (a CHART or DIMENSION with its `obsolete` option) and stops collecting what it names.
func (c *liveChild) obsolete(chart string, lines []string, keep []string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	for _, l := range lines {
		c.conn.Linef("%s", l)
	}
	c.charts[chart] = keep
	_ = c.conn.Flush()
}

func (c *liveChild) close() {
	select {
	case <-c.stop:
		return
	default:
	}
	close(c.stop)
	<-c.done
	_ = c.conn.Close()
}

// maintenanceView is what the check compares at each step: the chart answers of A, the hosts the agent lists and
// mirrors, and the metadata rows (UUIDs masked).
func maintenanceView(t *testing.T, d *daemon.Daemon) string {
	t.Helper()
	get := func(path string) (int, []byte) {
		b, err := rawExchange(d.Addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 10*time.Second)
		if err != nil {
			t.Fatalf("%s: %v", path, err)
		}
		var code int
		fmt.Sscanf(string(b), "HTTP/1.1 %d", &code)
		return code, httpBody(b)
	}
	var out []string
	for _, chart := range []string{"a.c1", "a.c2"} {
		code, body := get("/host/" + maintA.Hostname + "/api/v1/chart?chart=" + chart)
		var doc struct {
			Dimensions map[string]any `json:"dimensions"`
		}
		_ = json.Unmarshal(body, &doc)
		dims := make([]string, 0, len(doc.Dimensions))
		for k := range doc.Dimensions {
			dims = append(dims, k)
		}
		slices.Sort(dims)
		out = append(out, fmt.Sprintf("chart %s: %d %v", chart, code, dims))
	}
	_, body := get("/api/v1/charts")
	var charts struct {
		HostsCount int `json:"hosts_count"`
		Hosts      []struct {
			Hostname string `json:"hostname"`
		} `json:"hosts"`
	}
	_ = json.Unmarshal(body, &charts)
	var listed []string
	for _, h := range charts.Hosts {
		listed = append(listed, h.Hostname)
	}
	slices.Sort(listed)
	out = append(out, fmt.Sprintf("charts hosts: %d %v", charts.HostsCount, listed))
	_, body = get("/api/v1/info")
	var info struct {
		Mirrored []string `json:"mirrored_hosts"`
	}
	_ = json.Unmarshal(body, &info)
	slices.Sort(info.Mirrored)
	out = append(out, fmt.Sprintf("mirrored: %v", info.Mirrored))
	rows := dumpDB(t, filepath.Join(d.Opts.RunDir, "cache", "netdata-meta.db"), "--table", "dimension",
		"--mask", "dimension.dim_id", "--mask", "dimension.chart_id", "--sort", "dimension")
	for _, l := range strings.Split(rows, "\n") {
		if strings.HasPrefix(l, "row dimension") {
			out = append(out, l)
		}
	}
	return strings.Join(out, "\n")
}

// TestRRDMaintenance (check `rrd.maintenance`, S7b commit 9, D94; about 4 minutes, PARITY_LONG=1): the cleanup times
// at their minimum or a little above (obsolete charts 10 s, orphan hosts 90 s, ephemeral hosts 120 s) and a HEALTH pass
// every second. Each agent runs the same timeline and the check waits, at each step, until both show the same view
// (A's chart answers, the hosts listed and mirrored, the dimension rows with the UUIDs masked), then compares the
// records of the maintenance.
//   - A obsoletes a dimension and a chart: gone from the chart answers at once, their rows once freed.
//   - B (alloc) and E (dbengine, `_is_ephemeral`) disconnect: B's charts are marked obsolete and freed, B is archived
//     after the orphan time, then freed once a deep pass leaves it no retention; E is freed after the ephemeral time.
func TestRRDMaintenance(t *testing.T) {
	if os.Getenv("PARITY_LONG") == "" {
		t.Skip("PARITY_LONG unset (the orphan and ephemeral times run for minutes)")
	}
	opts := daemon.Options{StorageTiers: 1, StreamMemoryMode: "alloc", PulseOff: true,
		DBExtra: "    cleanup obsolete charts after = 10s\n    cleanup orphan hosts after = 90s\n" +
			"    cleanup ephemeral hosts after = 120s\n",
		HealthExtra: "    run at least every = 1s\n",
		StreamExtra: fmt.Sprintf("\n[%s]\n    type = machine\n    db = dbengine\n", maintE.MachineGUID),
		LogsExtra:   "    level = debug\n"}
	p := StartPair(t, opts, parentIdentity)
	var a, b, e [2]*liveChild
	for i, side := range p.Each() {
		a[i] = startLiveChild(t, side.Daemon, maintA, nil, map[string][]string{"a.c1": {"d1", "d2"}, "a.c2": {"x"}})
		b[i] = startLiveChild(t, side.Daemon, maintB, nil, map[string][]string{"b.c1": {"d"}})
		e[i] = startLiveChild(t, side.Daemon, maintE, [][2]string{{"_is_ephemeral", "true"}},
			map[string][]string{"e.c1": {"d"}})
	}
	// equal views on both sides, within the deadline
	settle := func(step string, deadline time.Duration, want func(view string) bool) {
		t.Helper()
		end := time.Now().Add(deadline)
		var v [2]string
		for {
			for i, side := range p.Each() {
				v[i] = maintenanceView(t, side.Daemon)
			}
			if v[0] == v[1] && want(v[0]) {
				t.Logf("%s:\n%s", step, v[0])
				return
			}
			if time.Now().After(end) {
				t.Fatalf("%s: not reached\noracle:\n%s\ncandidate:\n%s", step, v[0], v[1])
			}
			time.Sleep(2 * time.Second)
		}
	}
	has := func(s string) func(string) bool { return func(v string) bool { return strings.Contains(v, s) } }
	all := func(parts ...string) func(string) bool {
		return func(v string) bool {
			for _, p := range parts {
				if !strings.Contains(v, p) {
					return false
				}
			}
			return true
		}
	}
	// A's charts answer and every dimension has its row (METASYNC stores them within about 6 s)
	settle("connected", 60*time.Second, func(v string) bool {
		return all("chart a.c1: 200 [d1 d2]", "chart a.c2: 200 [x]",
			"mirrored: [maint-a maint-b maint-e parity-parent]")(v) && strings.Count(v, "row dimension") == 5
	})

	for i := range a {
		a[i].obsolete("a.c1", []string{
			"CHART 'a.c1' '' 'title' 'units' 'family' 'a.c1' line 1000 1 '' maint corpus",
			"DIMENSION 'd2' '' absolute 1 1 'obsolete'",
		}, []string{"d1"})
		a[i].obsolete("a.c2", []string{"CHART 'a.c2' '' 'title' 'units' 'family' 'a.c2' line 1000 1 'obsolete' maint corpus"},
			nil)
		b[i].close()
		e[i].close()
	}
	// out of the chart answers at once
	settle("obsoleted", 20*time.Second, all("chart a.c1: 200 [d1]", "chart a.c2: 404"))
	// freed after 10 s quiet and a sweep: the rows go at the job after
	settle("freed", 60*time.Second, func(v string) bool {
		return !strings.Contains(v, `id="d2"`) && !strings.Contains(v, `id="x"`)
	})
	// B and E archived after 90 s and more than 10 HEALTH passes: left out of the charts' hosts
	settle("archived", 150*time.Second, has("charts hosts: 4 [maint-a parity-parent]"))
	// E freed after 120 s; B once a deep pass leaves it no retention
	settle("hosts freed", 240*time.Second, has("mirrored: [maint-a parity-parent]"))
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("stop %s: %v", side.Role, err)
		}
	}
	records := regexp.MustCompile(`is now in archive mode|is archived, ephemeral clean up`)
	var got [2][]string
	for i, side := range p.Each() {
		for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
			if records.MatchString(l) {
				got[i] = append(got[i], normalizeLog(l, side.Daemon.Opts.RunDir, ""))
			}
		}
		slices.Sort(got[i])
	}
	if strings.Join(got[0], "\n") != strings.Join(got[1], "\n") {
		t.Errorf("maintenance records:\noracle:\n%s\ncandidate:\n%s", strings.Join(got[0], "\n"),
			strings.Join(got[1], "\n"))
	}
	if len(got[0]) == 0 {
		t.Errorf("the oracle logged no archive or free")
	}
}
