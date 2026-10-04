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

// workspaceTool is a tool of the Rust workspace: the path in env, else the workspace's debug build of name (build
// says how to make it).
func workspaceTool(t *testing.T, env, name, build string) string {
	t.Helper()
	bin := os.Getenv(env)
	if bin == "" {
		bin = filepath.Join("..", "..", "src", "crates", "target", "debug", name)
	}
	if _, err := os.Stat(bin); err != nil {
		t.Fatalf("parity: %s (%s, or set %s): %v", name, build, env, err)
	}
	return bin
}

// metadataDump is the metadata crate's dump tool.
func metadataDump(t *testing.T) string {
	t.Helper()
	return workspaceTool(t, "PARITY_METADATA_DUMP", "metadata-dump",
		"cargo build -p netdata-agent-metadata --bin metadata-dump")
}

// dbengineInspect is the storage crate's inspector.
func dbengineInspect(t *testing.T) string {
	t.Helper()
	return workspaceTool(t, "PARITY_DBENGINE_INSPECT", "dbengine-inspect",
		"cargo build -p netdata-agent-storage --bin dbengine-inspect")
}

// dumpDB prints a database as metadata-dump does (on a copy: a read-only open of a WAL database needs its -shm).
func dumpDB(t *testing.T, db string, args ...string) string {
	t.Helper()
	out, err := dumpLiveDB(t, db, args...)
	if err != nil {
		t.Fatal(err)
	}
	return out
}

// dumpLiveDB is dumpDB for a database an agent may be writing: a copy taken in the middle of a write may not open,
// which is an error for the caller to poll over.
func dumpLiveDB(t *testing.T, db string, args ...string) (string, error) {
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
		return "", fmt.Errorf("metadata-dump %s: %v: %s", db, err, out)
	}
	return string(out), nil
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

// writerArgs has metadata-dump print the metadata writer's tables as the checks compare them: label and node
// instance rows sorted (C writes labels in pointer order), the chart, dimension and chart label rows in their
// natural order (the pulse charts are created as their data comes, D83.2), chart and dimension ids aliased (random
// UUIDs, still joinable), and the last connection masked (a clock). Without chartRows the dimension and chart label
// rows compare as multisets, their ids masked: on a chart table too old to store charts in, the pulse charts leave
// rows that no chart row ties to localhost (4g).
func writerArgs(chartRows bool) []string {
	args := []string{"--natural-order", "--mask", "host.last_connected"}
	for _, table := range []string{"host", "host_info", "host_label", "node_instance", "chart", "dimension",
		"chart_label"} {
		args = append(args, "--table", table)
	}
	sorted := []string{"host_label", "node_instance", "chart_label"}
	ids, keep := []string{"dimension.dim_id", "dimension.chart_id", "chart_label.chart_id"}, "--alias"
	if !chartRows {
		sorted, keep = append(sorted, "dimension"), "--mask"
	}
	for _, table := range sorted {
		args = append(args, "--sort", table)
	}
	args = append(args, "--alias", "chart.chart_id")
	for _, column := range ids {
		args = append(args, keep, column)
	}
	return args
}

// compareFiles stops both daemons, together, once localhost's pulse charts stored a point and both sides have the
// same lazy ones (C's exit stores one more cycle, D81.3), and compares their databases: the whole context database
// but its rows, and the metadata database's header, pragmas, schema and tables as metadata-dump prints them with args,
// then the migration records.
func compareFiles(t *testing.T, p *Pair, args ...string) {
	t.Helper()
	if !p.Oracle.Opts.PulseOff {
		for _, side := range p.Each() {
			waitPulseStored(t, side.Daemon)
		}
		waitLazyEqual(t, p)
	}
	stopBoth(t, p)
	args = append([]string{"--mask", "agent_event_log.value", "--mask", "health_log_detail.global_id",
		"--mask", "health_log_detail.transition_id"}, args...)
	var got [2]string
	for i, side := range p.Each() {
		cache := filepath.Join(side.Daemon.Opts.RunDir, "cache")
		got[i] = dumpDB(t, filepath.Join(cache, "netdata-meta.db"), args...) + dumpDB(t, filepath.Join(cache, "context-meta.db"))
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
	writer := writerArgs(true)
	t.Run("fresh", func(t *testing.T) {
		compareFiles(t, StartPair(t, opts, parentIdentity),
			append([]string{"--table", "agent_event_log"}, writer...)...)
	})
	for _, name := range []string{"child", "child-final"} {
		t.Run(name, func(t *testing.T) {
			o := opts
			if name == "child-final" {
				// the periodic job's record, which must not appear, is a debug one
				o.LogsExtra = "    level = debug\n"
			}
			p := StartPair(t, o, parentIdentity)
			for _, side := range p.Each() {
				writerChild(t, side.Daemon)
			}
			if name == "child" {
				time.Sleep(8 * time.Second)
			} else {
				// the periodic job first runs 6 s after METASYNC starts: child-final stops before it, both sides
				// together, once the child's pulse charts exist on both; the job's record fails it if it ran first
				// (D83.3)
				waitLocalCharts(t, p, 5*time.Second,
					append(childPulseCharts(childHost.MachineGUID), "netdata.network_streaming")...)
				for _, side := range p.Each() {
					t.Logf("%s: the child's pulse charts %.1f s after the launch", side.Role,
						time.Since(side.Daemon.LaunchStartedAt).Seconds())
				}
			}
			if name == "child" {
				// the periodic job stored the child while both run (the final store would too, later)
				for _, side := range p.Each() {
					db := filepath.Join(side.Daemon.Opts.RunDir, "cache", "netdata-meta.db")
					if !strings.Contains(dumpDB(t, db, "--table", "host"), hexID(childHost.MachineGUID)) {
						t.Errorf("%s: no child host row before the stop", side.Role)
					}
				}
			}
			compareFiles(t, p, writer...)
			if name == "child-final" {
				if o, c := writerRecords(t, p.Oracle), writerRecords(t, p.Candidate); o != c {
					t.Errorf("final store records:\noracle:\n%s\ncandidate:\n%s", o, c)
				}
				for _, side := range p.Each() {
					for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
						if strings.Contains(l, `msg="Checking all hosts completed in `) {
							t.Errorf("%s: the periodic job ran before the stop: %s", side.Role, l)
						}
					}
				}
			}
		})
	}
	seed := seedFromOracle(t, parentIdentity, "ram")
	t.Run("seeded", func(t *testing.T) {
		o := opts
		o.SeedCache = seed
		compareFiles(t, StartPair(t, o, parentIdentity),
			append([]string{"--table", "agent_event_log"}, writer...)...)
	})
	t.Run("node-ids", func(t *testing.T) {
		// an agent without a claimed id drops every node id at start (D61.3)
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
		execDB(t, filepath.Join(dir, "netdata-meta.db"),
			"UPDATE node_instance SET node_id = x'0102030405060708090a0b0c0d0e0f10'")
		o := opts
		o.SeedCache = dir
		p := StartPair(t, o, parentIdentity)
		compareFiles(t, p, append([]string{"--table", "agent_event_log"}, writer...)...)
		for _, side := range p.Each() {
			db := filepath.Join(side.Daemon.Opts.RunDir, "cache", "netdata-meta.db")
			if strings.Contains(dumpDB(t, db, "--table", "node_instance"), "0102030405060708090a0b0c0d0e0f10") {
				t.Errorf("%s: a node id survived the start", side.Role)
			}
		}
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
		compareFiles(t, StartPair(t, o, parentIdentity),
			append([]string{"--table", "agent_event_log"}, writer...)...)
	})
	t.Run("alert-cleanup", func(t *testing.T) { alertCleanup(t, seed) })
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
			compareFiles(t, StartPair(t, o, parentIdentity), append([]string{"--table",
				"agent_event_log", "--table", "health_log", "--table", "health_log_detail",
				"--mask", "health_log.last_transition_id"},
				writerArgs(false)...)...)
		})
	}
}

// alertCleanupSeed adds alert log rows to the seeded cache (seedFromOracle: the child's charts seed.one and seed.two
// are stored): for the child an alert on a chart it has and one on a chart it does not have, for the parent one on a
// chart it does not have, and one of a host the host table does not hold; each with one entry.
const alertCleanupSeed = `
INSERT INTO health_log (health_log_id, host_id, alarm_id, config_hash_id, name, chart, recipient, units, chart_context,
  last_transition_id, chart_name)
  SELECT 1, host_id, 11, x'a0000000000040008000000000000001', 'kept', 'seed.one', 'root', 'u', 'seed.one',
    x'b0000000000040008000000000000001', 'seed.one' FROM host WHERE hostname = '%[1]s';
INSERT INTO health_log (health_log_id, host_id, alarm_id, config_hash_id, name, chart, recipient, units, chart_context,
  last_transition_id, chart_name)
  SELECT 2, host_id, 12, x'a0000000000040008000000000000002', 'gone', 'seed.gone', 'root', 'u', 'seed.gone',
    x'b0000000000040008000000000000002', 'seed.gone' FROM host WHERE hostname = '%[1]s';
INSERT INTO health_log (health_log_id, host_id, alarm_id, config_hash_id, name, chart, recipient, units, chart_context,
  last_transition_id, chart_name)
  SELECT 3, host_id, 13, x'a0000000000040008000000000000003', 'parent_gone', 'seed.one', 'root', 'u', 'seed.one',
    x'b0000000000040008000000000000003', 'seed.one' FROM host WHERE hostname = '%[2]s';
INSERT INTO health_log (health_log_id, host_id, alarm_id, config_hash_id, name, chart, recipient, units, chart_context,
  last_transition_id, chart_name)
  VALUES (4, x'f0000000000040008000000000000004', 14, x'a0000000000040008000000000000004', 'no_host', 'nosuch.chart',
    'root', 'u', 'nosuch', x'b0000000000040008000000000000004', 'nosuch.chart');
INSERT INTO health_log_detail (health_log_id, unique_id, alarm_id, alarm_event_id, updated_by_id, updates_id, when_key,
  duration, non_clear_duration, flags, exec_run_timestamp, delay_up_to_timestamp, info, exec_code, new_status,
  old_status, delay, new_value, old_value, last_repeat, transition_id, global_id, summary)
  SELECT health_log_id, 100 + health_log_id, alarm_id, 1, 0, 0, 1700000000, 0, 0, 1, 0, 1700000000, 'info', 0, 1, 0, 0,
    1.5, NULL, 0, last_transition_id, 1700000000000000 + health_log_id, NULL FROM health_log;
`

// alertCleanup compares `-W sqlite-alert-cleanup` (M9 commit 5; sqlite_health.c:617-687, daemon/main.c:452-455): each
// binary runs it on its own copy of the seeded cache with alertCleanupSeed's rows, named by a netdata.conf given
// before the option (C sets its directories when it loads `-c`, netdata-conf.c:6-40; the option alone would open the
// compiled-in cache directory). C opens the database as an agent does (migrations, the start's cleanup batch), then,
// per row of the host table, deletes the alert log rows of charts the host does not have. Compared: the alert log's
// tables after, what was written to stderr, the exit status, what was written to stdout, and the files the command
// leaves in the cache directory (C exits with the database open: its -wal and -shm files stay).
func alertCleanup(t *testing.T, seed string) {
	var tables, records, files [2]string
	var codes [2]int
	for i, bin := range binaries(t) {
		dir := filepath.Join(t.TempDir(), string([]Role{Oracle, Candidate}[i]))
		cache := filepath.Join(dir, "cache")
		for _, sub := range []string{"cache", "lib", "log", "etc"} {
			if err := os.MkdirAll(filepath.Join(dir, sub), 0o755); err != nil {
				t.Fatal(err)
			}
		}
		for _, f := range []string{"netdata-meta.db", "context-meta.db"} {
			b, err := os.ReadFile(filepath.Join(seed, f))
			if err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(cache, f), b, 0o644); err != nil {
				t.Fatal(err)
			}
		}
		execDB(t, filepath.Join(cache, "netdata-meta.db"), fmt.Sprintf(alertCleanupSeed, childHost.Hostname, parentIdentity.Hostname))
		conf := filepath.Join(dir, "etc", "netdata.conf")
		text := fmt.Sprintf("[directories]\n    config = %[1]s/etc\n    cache = %[1]s/cache\n    lib = %[1]s/lib\n    log = %[1]s/log\n"+
			"    home = %[1]s/lib\n", dir)
		if err := os.WriteFile(conf, []byte(text), 0o644); err != nil {
			t.Fatal(err)
		}
		stdout, stderr, code := runPrint(t, bin, "-c", conf, "-W", "sqlite-alert-cleanup")
		// before the dump opens the database
		left, err := os.ReadDir(cache)
		if err != nil {
			t.Fatal(err)
		}
		for _, f := range left {
			files[i] += f.Name() + "\n"
		}
		tables[i] = strings.Join(alertCleanupRows(dumpDB(t, filepath.Join(cache, "netdata-meta.db"), "--table", "health_log",
			"--table", "health_log_detail", "--table", "alert_queue", "--table", "aclk_queue")), "\n")
		records[i] = "stdout: " + stdout + "\nstderr:\n" + errnoRe.ReplaceAllString(strings.ReplaceAll(stderr, dir, "<RUN>"), "")
		codes[i] = code
	}
	// the oracle did the work: the two alerts on charts their host does not have are gone, the child's alert on its
	// own chart and the unknown host's stay, and every entry stays (the entries of a deleted alert go at an agent's
	// hourly cleanup)
	for _, want := range []struct {
		part string
		n    int
	}{{`row health_log `, 2}, {` name="kept" `, 1}, {` name="no_host" `, 1}, {`row health_log_detail `, 4}} {
		if got := strings.Count(tables[0], want.part); got != want.n {
			t.Fatalf("oracle: %d rows with %q after the cleanup, want %d:\n%s", got, want.part, want.n, tables[0])
		}
	}
	if codes[0] != 0 || !strings.Contains(records[0], `msg="Alert cleanup done"`) {
		t.Fatalf("oracle: exit %d, records:\n%s", codes[0], records[0])
	}
	if !strings.Contains(files[0], "netdata-meta.db-wal\n") {
		t.Fatalf("oracle: no write-ahead log left in the cache directory:\n%s", files[0])
	}
	if tables[0] != tables[1] {
		t.Fatalf("the alert log's tables differ after the cleanup\noracle:\n%s\ncandidate:\n%s", tables[0], tables[1])
	}
	if records[0] != records[1] {
		t.Errorf("the output differs\noracle:\n%s\ncandidate:\n%s", records[0], records[1])
	}
	if codes[0] != codes[1] {
		t.Errorf("exit status: oracle %d, candidate %d", codes[0], codes[1])
	}
	if files[0] != files[1] {
		t.Errorf("the files left in the cache directory differ\noracle:\n%s\ncandidate:\n%s", files[0], files[1])
	}
	t.Logf("both sides: exit %d\n%s\n%s\nleft in the cache directory:\n%s", codes[0], tables[0], records[0], files[0])
}

// alertCleanupRows are the rows of a metadata-dump output (the header, the pragmas and the schema are `sqlite.files`'
// other subtests').
func alertCleanupRows(dump string) []string {
	var out []string
	for _, l := range strings.Split(strings.TrimRight(dump, "\n"), "\n") {
		if strings.HasPrefix(l, "row ") || strings.HasPrefix(l, "missing ") {
			out = append(out, l)
		}
	}
	return out
}

// TestSQLiteHandBack gives C a cache the Rust agent ran on (check `sqlite.handback`): a C-written cache with a
// dbengine child goes through an alloc start and exit, then C starts on it in dbengine mode. Both final runs must list
// the same hosts and the child's contexts, with the same context load records. The agent-event medians differ by
// design (the Rust run added its own events). `pulse-off`: Rust alone runs, next to C on an untouched copy (an alloc
// localhost gives its pulse dimensions new UUIDs, as C's does, which C would then find without retention, D81).
// `pulse-on` (4g): C and Rust both run on the seed with their pulse charts, and C takes back each one's cache.
func TestSQLiteHandBack(t *testing.T) {
	seed := seedFromOracle(t, parentIdentity, "dbengine")
	for name, pulse := range map[string]bool{"pulse-off": false, "pulse-on": true} {
		t.Run(name, func(t *testing.T) { handBack(t, seed, pulse) })
	}
}

func handBack(t *testing.T, seed string, pulse bool) {
	caches := [2]string{seed, ""}
	opts := daemon.Options{DBMode: "alloc", StorageTiers: 1, StreamMemoryMode: "alloc", PulseOff: !pulse}
	if pulse {
		mid := startPair(t, opts, parentIdentity, binaries(t), [2]string{seed, seed},
			[2]Role{Role("c-alloc"), Role("rust")})
		for _, side := range mid.Each() {
			waitPulseStored(t, side.Daemon)
		}
		waitLazyEqual(t, mid)
		caches = stopBoth(t, mid)
	} else {
		opts.Binary, opts.RunDir, opts.Identity, opts.SeedCache = os.Getenv("PARITY_CANDIDATE"),
			runDir(t, Role("rust")), &parentIdentity, seed
		rust, err := daemon.Start(opts)
		if err != nil {
			t.Fatalf("rust: %v", err)
		}
		time.Sleep(2 * time.Second)
		if err := rust.Stop(); err != nil {
			t.Fatalf("rust: stop: %v", err)
		}
		caches[1] = filepath.Join(rust.Opts.RunDir, "cache")
	}
	p := &Pair{}
	for i, cache := range caches {
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
			t.Errorf("%s:\nC after C (or on the untouched cache): %s\nC after the Rust agent: %s", check.name, o, c)
		}
	}
	if got := member(t, p.Candidate, "/host/"+childHost.Hostname+"/api/v1/contexts", "contexts"); got == "{}" ||
		got == "null" {
		t.Errorf("no child contexts after the hand-back: %s", got)
	}
}
