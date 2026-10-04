// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// ctxCleanupSeed adds to the runR fixture's metadata database a chart of b6child in the context q.x, with a label
// and two dimensions no tier holds, an extra dimension with no data on the chart b6.c0 (context b6.ctx, whose other
// dimensions have data), and cleanup rows for q.x, b6.ctx and q.z, a context no chart has.
const ctxCleanupSeed = `
INSERT INTO chart (chart_id, host_id, type, id, name, family, context, title, unit, plugin, module, priority,
  update_every, chart_type, memory_mode, history_entries)
  SELECT x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', host_id, 'q', 'q.x', 'q.x', 'f', 'q.x', 't', 'u', 'p', '', 1000, 1, 0,
  4, 3600 FROM host WHERE hostname = 'b6child';
INSERT INTO chart_label (chart_id, source_type, label_key, label_value, date_created)
  VALUES (x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', 1, 'k', 'v', 1);
INSERT INTO dimension (dim_id, chart_id, id, name, multiplier, divisor, algorithm)
  VALUES (x'd1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1', x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', 'a', 'a', 1, 1, 0),
         (x'd2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2', x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', 'b', 'b', 1, 1, 0);
INSERT INTO dimension (dim_id, chart_id, id, name, multiplier, divisor, algorithm)
  SELECT x'd3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3', chart_id, 'extra', 'extra', 1, 1, 0 FROM chart
  WHERE type = 'b6' AND id = 'c0' AND host_id = (SELECT host_id FROM host WHERE hostname = 'b6child');
INSERT INTO ctx_metadata_cleanup (host_id, context, date_created)
  SELECT host_id, c, 1 FROM host, (SELECT 'q.x' AS c UNION ALL SELECT 'b6.ctx' UNION ALL SELECT 'q.z')
  WHERE hostname = 'b6child';
`

// ctxCleanupRecords are the context cleanup scan's records.
var ctxCleanupRecords = regexp.MustCompile(`msg="((Verifying the retention|Verified the contexts)[^"]*)"`)

// scanRecords are a daemon's context cleanup records, in order.
func scanRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if m := ctxCleanupRecords.FindStringSubmatch(l); m != nil {
			out = append(out, m[1])
		}
	}
	return out
}

// TestSQLiteCtxCleanup (check `sqlite.ctx-cleanup`): both daemons start on copies of the runR fixture's cache with
// the rows of ctxCleanupSeed, and run the metadata writer's context cleanup scan (about 12 s after the start).
// Compared once both stop: the dimension, chart, chart label and cleanup rows, and the scan's records. With the
// dbengine (as tier 0, or for the children an alloc agent receives) the dimensions no tier holds go, every one when
// the dbengine's files are gone (with the last one of a chart, the chart and its labels); an alloc agent that
// receives alloc children runs no dbengine, and every dimension stays, files or not (C's check is false without the
// dbengine, where a freed dimension's row would go without files, D75.12); either way the cleanup rows of the
// contexts a chart has go and q.z's stays.
func TestSQLiteCtxCleanup(t *testing.T) {
	fx, id := runRParent(t)
	for _, v := range []struct {
		name, mode, children string
		tiers                int
		noFiles              bool
	}{
		{"dbengine", "", "", 3, false},
		{"alloc", "alloc", "", 3, false},
		{"alloc-no-datafiles", "alloc", "", 3, true},
		{"no-dbengine", "alloc", "alloc", 1, false},
		{"no-dbengine-no-datafiles", "alloc", "alloc", 1, true},
	} {
		t.Run(v.name, func(t *testing.T) {
			seed := t.TempDir()
			copyTree(t, filepath.Join(fx, "runR", "cache"), seed)
			execDB(t, filepath.Join(seed, "netdata-meta.db"), ctxCleanupSeed)
			if v.noFiles {
				for _, dir := range []string{"dbengine", "dbengine-tier1", "dbengine-tier2"} {
					if err := os.RemoveAll(filepath.Join(seed, dir)); err != nil {
						t.Fatal(err)
					}
				}
			}
			p := StartPair(t, daemon.Options{StorageTiers: v.tiers, TierRetentionMB: [3]int{25, 25, 25}, DBMode: v.mode,
				StreamMemoryMode: v.children, SeedCache: seed, PulseOff: true, LogsExtra: "    level = debug\n"}, id)
			// the second store job scans b6child, about 12 s after the start
			deadline := time.Now().Add(40 * time.Second)
			for _, side := range p.Each() {
				for !strings.Contains(strings.Join(scanRecords(t, side.Daemon), "\n"),
					"Verified the contexts of host b6child") {
					if time.Now().After(deadline) {
						t.Fatalf("%s: no scan of b6child: %q", side.Role, scanRecords(t, side.Daemon))
					}
					time.Sleep(500 * time.Millisecond)
				}
			}
			for _, side := range p.Each() {
				if err := side.Daemon.Stop(); err != nil {
					t.Fatalf("stop %s: %v", side.Role, err)
				}
			}
			args := []string{"--table", "dimension", "--table", "chart", "--table", "chart_label",
				"--table", "ctx_metadata_cleanup", "--mask", "ctx_metadata_cleanup.date_created"}
			var dumps, records [2]string
			for i, side := range p.Each() {
				dumps[i] = dumpDB(t, filepath.Join(side.Daemon.Opts.RunDir, "cache", "netdata-meta.db"), args...)
				records[i] = strings.Join(scanRecords(t, side.Daemon), "\n")
			}
			if dumps[0] != dumps[1] {
				t.Errorf("tables differ:\noracle:\n%s\ncandidate:\n%s", dumps[0], dumps[1])
			}
			if records[0] != records[1] {
				t.Errorf("scan records differ:\noracle:\n%s\ncandidate:\n%s", records[0], records[1])
			}
			t.Logf("oracle scan records:\n%s", records[0])
		})
	}
}

// cyclesSeed adds orphans to the runR fixture's metadata database: a chart of b6child in its own context with two
// dimensions no tier holds and a label (the context loader finds the context empty and queues it, so the context
// cleanup scan deletes them about 12 s after the start), a dimension no tier holds on b6.c0 (whose context has data,
// so it waits for the dimension cycle), a chart of b6child without dimensions, and a label of a chart that does not
// exist.
const cyclesSeed = `
INSERT INTO chart (chart_id, host_id, type, id, name, family, context, title, unit, plugin, module, priority,
  update_every, chart_type, memory_mode, history_entries)
  SELECT x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', host_id, 'q', 'q.x', 'q.x', 'f', 'q.x', 't', 'u', 'p', '', 1000, 1, 0,
  4, 3600 FROM host WHERE hostname = 'b6child';
INSERT INTO chart (chart_id, host_id, type, id, name, family, context, title, unit, plugin, module, priority,
  update_every, chart_type, memory_mode, history_entries)
  SELECT x'c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2', host_id, 'q', 'q.y', 'q.y', 'f', 'q.y', 't', 'u', 'p', '', 1000, 1, 0,
  4, 3600 FROM host WHERE hostname = 'b6child';
INSERT INTO chart_label (chart_id, source_type, label_key, label_value, date_created)
  VALUES (x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', 1, 'k', 'v', 1), (x'c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3', 1, 'k', 'v', 1);
INSERT INTO dimension (dim_id, chart_id, id, name, multiplier, divisor, algorithm)
  VALUES (x'd1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1', x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', 'a', 'a', 1, 1, 0),
         (x'd2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2', x'c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1', 'b', 'b', 1, 1, 0);
INSERT INTO dimension (dim_id, chart_id, id, name, multiplier, divisor, algorithm)
  SELECT x'd3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3', chart_id, 'extra', 'extra', 1, 1, 0 FROM chart
  WHERE type = 'b6' AND id = 'c0' AND host_id = (SELECT host_id FROM host WHERE hostname = 'b6child');
`

// cyclesHealthSeed adds alert log rows for the health log's cleanup, which the metadata thread runs with the cycles:
// first 1800 s after its first store job, for every host it knows with the host's own retention, then the orphans
// (sqlite_metadata.c:1648-1682, :1884; sqlite_health.c:356-385). The agents run without health, so each host's
// retention is 0 (it is set at a host's first health pass, health_event_loop.c:267): every entry another one replaced
// goes, whatever its age, but an alert's last transition.
//   - Alert 901 is the parent's, 902 the archived child's, 903 a host's the host table does not hold.
//   - Each has five entries: 1 replaced and old (goes); 2 not replaced (stays); 3 replaced, and the alert's last
//     transition (stays); 4 replaced, an hour old (goes: inside any retention but 0); 5 replaced, dated 2100 (stays:
//     not before now).
//   - 903 and its entries go as orphans, as does the entry of alert 999, which no row has; of alert_version's three
//     rows 901's stays.
const cyclesHealthSeed = `
INSERT INTO health_log (health_log_id, host_id, alarm_id, config_hash_id, name, chart, recipient, units, chart_context,
  last_transition_id, chart_name)
  SELECT 901, host_id, 1, x'a1000000000040008000000000000001', 'local', 'b6.c0', 'root', 'u', 'b6.ctx',
    x'c1000000000040008000000000000003', 'b6.c0' FROM host WHERE hostname = '%[1]s';
INSERT INTO health_log (health_log_id, host_id, alarm_id, config_hash_id, name, chart, recipient, units, chart_context,
  last_transition_id, chart_name)
  SELECT 902, host_id, 1, x'a2000000000040008000000000000001', 'child', 'b6.c0', 'root', 'u', 'b6.ctx',
    x'c2000000000040008000000000000003', 'b6.c0' FROM host WHERE hostname = 'b6child';
INSERT INTO health_log (health_log_id, host_id, alarm_id, config_hash_id, name, chart, recipient, units, chart_context,
  last_transition_id, chart_name)
  VALUES (903, x'f0000000000040008000000000000903', 1, x'a3000000000040008000000000000001', 'nohost', 'b6.c0', 'root',
    'u', 'b6.ctx', x'c3000000000040008000000000000003', 'b6.c0');
WITH e(n, replaced_by, at) AS (VALUES (1, 2, 1700000000), (2, 0, 1700000000), (3, 9, 1700000000),
  (4, 5, UNIXEPOCH() - 3600), (5, 6, 4102444800))
INSERT INTO health_log_detail (health_log_id, unique_id, alarm_id, alarm_event_id, updated_by_id, updates_id, when_key,
  duration, non_clear_duration, flags, exec_run_timestamp, delay_up_to_timestamp, info, exec_code, new_status,
  old_status, delay, new_value, old_value, last_repeat, transition_id, global_id, summary)
  SELECT hl.health_log_id, hl.health_log_id * 10 + e.n, 1, e.n, e.replaced_by, 0, e.at, 0, 0, 3, 0, e.at, 'info', 0, 1, 0, 0,
    1.5, NULL, 0, unhex(printf('c%%d0000000000400080000000000000%%02d', hl.health_log_id - 900, e.n)),
    1700000000000000 + hl.health_log_id * 10 + e.n, NULL
  FROM health_log hl, e WHERE hl.health_log_id IN (901, 902, 903) ORDER BY hl.health_log_id, e.n;
INSERT INTO health_log_detail (health_log_id, unique_id, alarm_id, alarm_event_id, updated_by_id, updates_id, when_key,
  flags, new_status, old_status, transition_id, global_id)
  VALUES (999, 9991, 1, 1, 0, 0, 1700000000, 3, 1, 0, x'c9000000000040008000000000000001', 1700000000009991);
INSERT INTO alert_version (health_log_id, unique_id, status, version, date_submitted)
  VALUES (901, 9013, 1, 1, 1), (903, 9033, 1, 1, 1), (999, 9991, 1, 1, 1);
`

// cyclesHealthWant are the alert log's rows the cleanup leaves of cyclesHealthSeed: two alerts, their entries 2, 3 and
// 5, and alert 901's version.
var cyclesHealthWant = map[string]int{"row health_log ": 2, "row health_log_detail ": 6, "row alert_version ": 1,
	"row health_log_detail health_log_id=901 unique_id=9012 ": 1, "row health_log_detail health_log_id=901 unique_id=9013 ": 1,
	"row health_log_detail health_log_id=901 unique_id=9015 ": 1, "row health_log_detail health_log_id=902 unique_id=9022 ": 1,
	"row health_log_detail health_log_id=902 unique_id=9023 ": 1, "row health_log_detail health_log_id=902 unique_id=9025 ": 1,
	"row alert_version health_log_id=901 ": 1}

// cycleRecords are the dimension, chart and chart-label cycles' records.
var cycleRecords = regexp.MustCompile(`msg="((Dimension|Chart|Chart label) metadata check [^"]*|Checking (dimensions|charts|chart labels) [^"]*|(Dimensions|Charts|Chart labels) checked [^"]*)"`)

func cycleLines(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if m := cycleRecords.FindStringSubmatch(l); m != nil {
			out = append(out, m[1])
		}
	}
	return out
}

// TestSQLiteMetadataCycles (check `sqlite.metadata-cycles`, about 32 minutes: set PARITY_LONG=1): both daemons on
// copies of the runR cache with cyclesSeed's orphans run the metadata writer's dimension, chart and chart-label cycles
// 1800 s after their first store job. Compared once the chart cycle completed on both (the label cycle's completion
// comes an hour later): the dimension, chart and chart-label rows and the cycles' records in order. And, with
// cyclesHealthSeed's rows (M9 commit 5), the alert log's tables after the health log's cleanup, which the store job
// that starts the dimension cycle runs too (it writes no record; the oracle's rows must be cyclesHealthWant's).
func TestSQLiteMetadataCycles(t *testing.T) {
	if os.Getenv("PARITY_LONG") == "" {
		t.Skip("PARITY_LONG unset (the cycles start 1800 s after the start)")
	}
	fx, id := runRParent(t)
	seed := t.TempDir()
	copyTree(t, filepath.Join(fx, "runR", "cache"), seed)
	execDB(t, filepath.Join(seed, "netdata-meta.db"), cyclesSeed)
	execDB(t, filepath.Join(seed, "netdata-meta.db"), fmt.Sprintf(cyclesHealthSeed, id.Hostname))
	if got := strings.Count(dumpDB(t, filepath.Join(seed, "netdata-meta.db"), "--table", "health_log", "--table", "health_log_detail"),
		"\nrow health_log"); got != 3+3*5+1 {
		t.Fatalf("harness: the seed holds %d alert log rows, want 3 alerts with 5 entries each and one more entry", got)
	}
	p := StartPair(t, daemon.Options{StorageTiers: 3, TierRetentionMB: [3]int{25, 25, 25}, SeedCache: seed,
		PulseOff: true, LogsExtra: "    level = debug\n"}, id)
	deadline := time.Now().Add(40 * time.Minute)
	for _, side := range p.Each() {
		for !strings.Contains(strings.Join(cycleLines(t, side.Daemon), "\n"), "Chart metadata check completed") {
			if time.Now().After(deadline) {
				t.Fatalf("%s: the chart cycle did not complete: %q", side.Role, cycleLines(t, side.Daemon))
			}
			time.Sleep(5 * time.Second)
		}
	}
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("stop %s: %v", side.Role, err)
		}
	}
	args := []string{"--table", "dimension", "--table", "chart", "--table", "chart_label", "--sort", "chart_label",
		"--table", "health_log", "--table", "health_log_detail", "--table", "alert_version"}
	var dumps, records [2]string
	for i, side := range p.Each() {
		dumps[i] = dumpDB(t, filepath.Join(side.Daemon.Opts.RunDir, "cache", "netdata-meta.db"), args...)
		records[i] = strings.Join(cycleLines(t, side.Daemon), "\n")
	}
	for part, want := range cyclesHealthWant {
		if got := strings.Count(dumps[0], "\n"+part); got != want {
			t.Errorf("oracle: %d rows `%s` after the health log's cleanup, want %d", got, part, want)
		}
	}
	if dumps[0] != dumps[1] {
		t.Errorf("tables differ:\noracle:\n%s\ncandidate:\n%s", dumps[0], dumps[1])
	}
	if records[0] != records[1] {
		t.Errorf("cycle records differ:\noracle:\n%s\ncandidate:\n%s", records[0], records[1])
	}
	t.Logf("oracle cycle records:\n%s", records[0])
}
