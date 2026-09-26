// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// metadataDump is the metadata crate's dump tool: PARITY_METADATA_DUMP, else the workspace's debug build.
func metadataDump(t *testing.T) string {
	t.Helper()
	bin := os.Getenv("PARITY_METADATA_DUMP")
	if bin == "" {
		bin = filepath.Join("..", "..", "src", "crates", "target", "debug", "metadata-dump")
	}
	if _, err := os.Stat(bin); err != nil {
		t.Fatalf("parity: metadata-dump (cargo build -p netdata-agent-metadata --bin metadata-dump, or set "+
			"PARITY_METADATA_DUMP): %v", err)
	}
	return bin
}

// dumpDB prints a database as metadata-dump does (on a copy: a read-only open of a WAL database needs its -shm).
func dumpDB(t *testing.T, db string, args ...string) string {
	t.Helper()
	dir := t.TempDir()
	for _, suffix := range []string{"", "-wal", "-shm"} {
		if b, err := os.ReadFile(db + suffix); err == nil {
			if err := os.WriteFile(filepath.Join(dir, "db"+suffix), b, 0o644); err != nil {
				t.Fatal(err)
			}
		}
	}
	out, err := exec.Command(metadataDump(t), append([]string{filepath.Join(dir, "db")}, args...)...).CombinedOutput()
	if err != nil {
		t.Fatalf("metadata-dump %s: %v: %s", db, err, out)
	}
	return string(out)
}

// execDB runs statements on a database through metadata-dump.
func execDB(t *testing.T, db, sql string) {
	t.Helper()
	if out, err := exec.Command(metadataDump(t), db, "--exec", sql).CombinedOutput(); err != nil {
		t.Fatalf("metadata-dump --exec: %v: %s", err, out)
	}
}

// migrationRecords are the migration and open records of the main thread.
var migrationRecords = regexp.MustCompile(`msg="(SQLite database |[a-z]+ database version is|Database version is|Running database|Database [a-z]+ migration|SQLite error|SQLite failed statement|Database is corrupted)`)

// hwLabels are C's `_hw_*` host labels, which come from its daemon status file (D49.3): C-only for now.
var hwLabels = regexp.MustCompile(`label_key="_hw_`)

// writerArgs has metadata-dump print the metadata writer's tables as the checks compare them: label and node
// instance rows sorted (C writes labels in pointer order), chart and dimension ids aliased (random UUIDs, still
// joinable), the localhost's charts left out (C's pulse charts, D48.6), and the last connection masked (a clock).
// Without chartRows the chart dimension and label rows are left out too: on a chart table too old to store charts
// in, C's pulse charts leave dimension and label rows that no chart row ties to the localhost.
func writerArgs(localhost string, chartRows bool) []string {
	args := []string{"--skip-host-charts", strings.Trim(strings.TrimPrefix(localhost, "x"), "'"),
		"--mask", "host.last_connected"}
	tables := []string{"host", "host_info", "host_label", "node_instance", "chart"}
	if chartRows {
		tables = append(tables, "dimension", "chart_label")
	}
	for _, table := range tables {
		args = append(args, "--table", table)
	}
	for _, table := range []string{"host_label", "node_instance", "chart_label"} {
		args = append(args, "--sort", table)
	}
	for _, column := range []string{"chart.chart_id", "dimension.dim_id", "dimension.chart_id",
		"chart_label.chart_id"} {
		args = append(args, "--alias", column)
	}
	return args
}

// compareFiles stops both daemons and compares their databases: the whole context database but its rows, and the
// metadata database's header, pragmas, schema and tables as metadata-dump prints them with args (lines matching
// drop left out on both sides), then the migration records.
func compareFiles(t *testing.T, p *Pair, drop *regexp.Regexp, args ...string) {
	t.Helper()
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("stop %s: %v", side.Role, err)
		}
	}
	args = append([]string{"--mask", "agent_event_log.value", "--mask", "health_log_detail.global_id",
		"--mask", "health_log_detail.transition_id"}, args...)
	var got [2]string
	for i, side := range p.Each() {
		cache := filepath.Join(side.Daemon.Opts.RunDir, "cache")
		var kept []string
		for _, l := range strings.Split(dumpDB(t, filepath.Join(cache, "netdata-meta.db"), args...), "\n") {
			if drop == nil || !drop.MatchString(l) {
				kept = append(kept, l)
			}
		}
		got[i] = strings.Join(kept, "\n") + dumpDB(t, filepath.Join(cache, "context-meta.db"))
		for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
			if threadOf(l) == "" && migrationRecords.MatchString(l) {
				got[i] += normalizeLog(l, side.Daemon.Opts.RunDir, "") + "\n"
			}
		}
	}
	if got[0] != got[1] {
		t.Errorf("databases differ\n%s", firstDifference([]byte(got[0]), []byte(got[1])))
	}
}

// hexID is a GUID as metadata-dump prints a blob.
func hexID(guid string) string {
	return "x'" + strings.ReplaceAll(guid, "-", "") + "'"
}

// writerChild streams what the metadata writer stores of a child: a host label, a chart with a chart label and a
// hidden incremental dimension, and a named stacked chart, with a few points.
func writerChild(t *testing.T, d *daemon.Daemon) {
	t.Helper()
	conn, err := stream.Connect(d.Addr, d.StreamKey, childHost, stream.CapsLive)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	conn.Linef(`LABEL "role" = 1 "writer"`)
	conn.Linef("OVERWRITE labels")
	conn.Linef("CHART 'w.one' '' 'title' 'units' 'family' 'w.one' line 1000 1 '' fixture-pusher corpus")
	conn.Linef("CLABEL 'tier' 'gold' 2")
	conn.Linef("CLABEL_COMMIT")
	conn.Linef("DIMENSION 'd1' '' absolute 1 1 ''")
	conn.Linef("DIMENSION 'd2' 'second' incremental 3 7 'hidden'")
	conn.Linef("CHART 'w.two' 'named' 'title two' 'units' 'family' 'w.ctx' stacked 900 1 '' fixture-pusher")
	conn.Linef("DIMENSION 'd1' '' percentage-of-absolute-row 1 1 ''")
	now := time.Now().Unix()
	for _, chart := range []string{"w.one", "w.two"} {
		conn.Linef("BEGIN2 '%s' 1 %d #", chart, now-1)
		conn.Linef("SET2 'd1' 1 1 A")
		conn.Linef("END2")
	}
	if err := conn.Flush(); err != nil {
		t.Fatal(err)
	}
}

// writerRecords are the final store's METASYNC records.
func writerRecords(t *testing.T, d *daemon.Daemon) string {
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if strings.Contains(l, `msg="METADATA: Progress of metadata storage`) {
			out = append(out, normalizeLog(l, d.Opts.RunDir, ""))
		}
	}
	return strings.Join(out, "\n")
}

// TestSQLiteFiles compares the database files both daemons leave behind (check `sqlite.files`): a fresh cache, a
// C-written one, the same with newer versions than both agents know, synthetic old versions that the migrations
// bring up to date (the version 8 per-host health log among them), and what the metadata writer stores of a child,
// by its periodic job and by the final store at shutdown (with that store's records).
func TestSQLiteFiles(t *testing.T) {
	opts := daemon.Options{DBMode: "alloc", StorageTiers: 1, StreamMemoryMode: "alloc"}
	writer := writerArgs(hexID(parentIdentity.MachineGUID), true)
	t.Run("fresh", func(t *testing.T) {
		compareFiles(t, StartPair(t, opts, parentIdentity), hwLabels,
			append([]string{"--table", "agent_event_log"}, writer...)...)
	})
	for _, name := range []string{"child", "child-final"} {
		t.Run(name, func(t *testing.T) {
			p := StartPair(t, opts, parentIdentity)
			for _, side := range p.Each() {
				writerChild(t, side.Daemon)
			}
			// the periodic job first runs 6 s after METASYNC starts; child-final stops before it
			wait := 8 * time.Second
			if name == "child-final" {
				wait = time.Second
			}
			time.Sleep(wait)
			compareFiles(t, p, hwLabels, writer...)
			if name == "child-final" {
				if o, c := writerRecords(t, p.Oracle), writerRecords(t, p.Candidate); o != c {
					t.Errorf("final store records:\noracle:\n%s\ncandidate:\n%s", o, c)
				}
			}
		})
	}
	seed := seedFromOracle(t, parentIdentity, "ram")
	t.Run("seeded", func(t *testing.T) {
		o := opts
		o.SeedCache = seed
		compareFiles(t, StartPair(t, o, parentIdentity), hwLabels,
			append([]string{"--table", "agent_event_log"}, writer...)...)
	})
	t.Run("newer-versions", func(t *testing.T) {
		dir := t.TempDir()
		for _, f := range []string{"netdata-meta.db", "context-meta.db"} {
			b, err := os.ReadFile(filepath.Join(seed, f))
			if err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(dir, f), b, 0o644); err != nil {
				t.Fatal(err)
			}
		}
		execDB(t, filepath.Join(dir, "netdata-meta.db"), "PRAGMA user_version=19")
		execDB(t, filepath.Join(dir, "context-meta.db"), "PRAGMA user_version=7")
		o := opts
		o.SeedCache = dir
		compareFiles(t, StartPair(t, o, parentIdentity), hwLabels,
			append([]string{"--table", "agent_event_log"}, writer...)...)
	})
	old := map[string]string{
		"v0": `CREATE TABLE host(host_id BLOB PRIMARY KEY, hostname TEXT NOT NULL, registry_hostname TEXT NOT NULL
			default 'unknown', update_every INT NOT NULL default 1, os TEXT NOT NULL default 'unknown', timezone TEXT
			NOT NULL default 'unknown', tags TEXT NOT NULL default '');
			INSERT INTO host (host_id, hostname) VALUES (x'5a1e0000000040008000000000000cc0', 'old-child');
			CREATE TABLE chart(chart_id blob PRIMARY KEY, host_id blob);
			INSERT INTO chart VALUES (x'01', x'5a1e0000000040008000000000000cc0');`,
		"v8": `CREATE TABLE host(host_id BLOB PRIMARY KEY, hostname TEXT NOT NULL);
			CREATE TABLE alert_hash(hash_id blob PRIMARY KEY);
			CREATE TABLE health_log_5a1e0000_0000_4000_8000_0000000000aa(alarm_id int, config_hash_id blob,
			  name text, chart text, family text, exec text, recipient text, units text, chart_context text,
			  unique_id int, alarm_event_id int, updated_by_id int, updates_id int, when_key int, duration int,
			  non_clear_duration int, flags int, exec_run_timestamp int, delay_up_to_timestamp int, info text,
			  exec_code int, new_status real, old_status real, delay int, new_value double, old_value double,
			  last_repeat int, transition_id blob);
			INSERT INTO health_log_5a1e0000_0000_4000_8000_0000000000aa (alarm_id, name, unique_id, alarm_event_id)
			  VALUES (3, 'cpu', 11, 1), (4, 'ram', 12, 1);
			PRAGMA user_version=8;`,
		"v13": `CREATE TABLE host(host_id BLOB PRIMARY KEY, hostname TEXT NOT NULL, hops INT NOT NULL DEFAULT 0);
			CREATE TABLE alert_hash(hash_id blob PRIMARY KEY, summary text);
			CREATE TABLE health_log_detail(health_log_id int, summary text);
			PRAGMA user_version=13;`,
	}
	for name, sql := range old {
		t.Run("old-"+name, func(t *testing.T) {
			dir := t.TempDir()
			execDB(t, filepath.Join(dir, "netdata-meta.db"), sql)
			o := opts
			o.SeedCache = dir
			// the migrations give old health log rows random transition ids
			compareFiles(t, StartPair(t, o, parentIdentity), hwLabels, append([]string{"--table",
				"agent_event_log", "--table", "health_log", "--table", "health_log_detail",
				"--mask", "health_log.last_transition_id"},
				writerArgs(hexID(parentIdentity.MachineGUID), false)...)...)
		})
	}
}

// TestSQLiteHandBack gives C a cache the Rust agent ran on (check `sqlite.handback`): a C-written cache with a
// dbengine child goes through a Rust start and exit, then C starts on it in dbengine mode, next to C on an
// untouched copy. Both must list the same hosts and the child's contexts, with the same context load records. The
// agent-event medians differ by design (the Rust run added its own events).
func TestSQLiteHandBack(t *testing.T) {
	seed := seedFromOracle(t, parentIdentity, "dbengine")
	rust, err := daemon.Start(daemon.Options{Binary: os.Getenv("PARITY_CANDIDATE"), RunDir: runDir(t, Role("rust")),
		Identity: &parentIdentity, DBMode: "alloc", StorageTiers: 1, StreamMemoryMode: "alloc", SeedCache: seed})
	if err != nil {
		t.Fatalf("rust: %v", err)
	}
	time.Sleep(2 * time.Second)
	if err := rust.Stop(); err != nil {
		t.Fatalf("rust: stop: %v", err)
	}
	p := &Pair{}
	for i, cache := range []string{seed, filepath.Join(rust.Opts.RunDir, "cache")} {
		d, err := daemon.Start(daemon.Options{Binary: os.Getenv("PARITY_ORACLE"),
			RunDir: runDir(t, Role(fmt.Sprintf("c%d", i))), Identity: &parentIdentity, StorageTiers: 1,
			StreamMemoryMode: "dbengine", SeedCache: cache, LogsExtra: "    level = debug\n"})
		if err != nil {
			t.Fatalf("c%d: %v", i, err)
		}
		t.Cleanup(func() { _ = d.Stop() })
		if i == 0 {
			p.Oracle = d
		} else {
			p.Candidate = d
		}
	}
	// the context load runs after the start
	time.Sleep(2 * time.Second)
	for _, check := range []struct {
		name string
		get  func(d *daemon.Daemon) string
	}{
		{"hosts", func(d *daemon.Daemon) string { return archivedHostsView(t, d) }},
		{"child contexts", func(d *daemon.Daemon) string {
			return member(t, d, "/host/"+childHost.Hostname+"/api/v1/contexts", "contexts")
		}},
		{"records", func(d *daemon.Daemon) string {
			var out []string
			for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
				if strings.Contains(l, `msg="RRDCONTEXT: metadata for node `) ||
					strings.Contains(l, `msg="Created `) {
					// both sides are C, whose errno on these records is a stale leftover (D36)
					out = append(out, errnoRe.ReplaceAllString(normalizeLog(l, d.Opts.RunDir, ""), ""))
				}
			}
			return strings.Join(out, "\n")
		}},
	} {
		if o, c := check.get(p.Oracle), check.get(p.Candidate); o != c {
			t.Errorf("%s:\nC on the untouched cache: %s\nC after the Rust agent: %s", check.name, o, c)
		}
	}
	if got := member(t, p.Candidate, "/host/"+childHost.Hostname+"/api/v1/contexts", "contexts"); got == "{}" ||
		got == "null" {
		t.Errorf("no child contexts after the hand-back: %s", got)
	}
}
