// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// cOnlyRecords are the oracle's records of subsystems the candidate does not have yet, each with the reason. They
// are left out on the oracle side only: the candidate writing one of them fails the check, so an entry is removed
// when its subsystem lands.
var cOnlyRecords = []struct {
	re     *regexp.Regexp
	reason string
}{
	{regexp.MustCompile(`msg="Cannot find a status file in any location"`), "daemon status file"},
	{regexp.MustCompile(`msg="OS_MACHINE_ID: `), "daemon status file (machine id probe)"},
	{regexp.MustCompile(`msg="Netdata Agent version '.*' is starting\.\.\.`), "daemon status file (startup event)"},
	{regexp.MustCompile(`msg="MCP[: ]`), "MCP"},
	{regexp.MustCompile(`msg="(JSON-RPC protocol|Echo protocol|MCP WebSocket adapter|WebSocket server subsystem) initialized`), "WebSocket"},
	{regexp.MustCompile(`msg="Flushing DBENGINE|msg="DBENGINE: flushing`), "C's pulse charts write into dbengine (S3/S6)"},
	{regexp.MustCompile(`msg="ACLK[: ]`), "ACLK"},
	{regexp.MustCompile(`msg="SQL: (suppressing SQLite teardown|skipping )`), "SQLite teardown"},
	{regexp.MustCompile(`msg="CLAIM: `), "claiming"},
	{regexp.MustCompile(`msg="SERVICE CONTROL: waiting for the following|msg="SERVICE: Signal to stop : `), "service registry of C's static threads (D44)"},
	{regexp.MustCompile(`msg="PLUGINSD: cleaning up\.\.\."|msg="PLUGINSD: cleanup completed\."`), "plugins.d"},
	{regexp.MustCompile(`msg="Failed to delete socket \d+ from nd_poll\(\) - called from poll_events_cleanup\(\)`), "D43 (shared listening sockets)"},
	{regexp.MustCompile(`msg="To use encryption it is necessary to set \\"ssl certificate\\" and \\"ssl key\\" in \[web\] !`), "web TLS"},
}

// portedRecords are records the candidate writes too although an entry of cOnlyRecords matches them: the ACLK proxy
// resolution (D49 point 7), the end of the archived hosts' load and the metadata database's close (D4 S1), compared
// despite the ACLK and METADATA entries.
var portedRecords = regexp.MustCompile(`msg="ACLK: (proxy is|using |proxy is explicitly)|msg="ACLK sync initialization completed"|msg="METADATA: Closing sqlite database"`)

// timedRecords depend on when a run stops rather than on what it did: the metadata writer's periodic job (from 6 s
// after METASYNC starts) and the per-host lines of its final store, whose hosts are the ones changed since the last
// job (C's localhost has pulse charts, D48.6). Both sides drop them; a check whose state is fixed compares them (D61.6).
// The dbengine population's progress lines race its workers, so they drop too.
var timedRecords = regexp.MustCompile(`msg="Checking all hosts completed in |msg="METADATA: Progress of metadata storage: +[0-9.]+% completed"|msg="DBENGINE: tier \d+: MRG population completed: `)

// cOnlyThreads are threads of subsystems the candidate does not have: all their records are the oracle's alone.
var cOnlyThreads = map[string]string{
	"ACLKSYNC": "ACLK", "SDBUSWATCHER": "systemd bus watcher",
	"PULSE": "pulse charts", "PLUGINSD": "plugins.d",
	"SERVICE": "service thread", "HEALTH": "health", "ANALYTICS": "analytics",
	"EXPORTING": "exporting engine", "STATSD_FLUSH": "statsd",
	"ACLK_MAIN": "ACLK", "BACKFILL": "dbengine tier backfill", "EXTENT_PGC": "dbengine evictors (S6)",
	"MAIN_PGC": "dbengine evictors (S6)", "OPEN_PGC": "dbengine evictors (S6)", "REPLAY": "replication sender threads",
}

// logMasks hide what differs between any two runs of the same binary: clocks, ids, ports, descriptors, timings.
var logMasks = []struct {
	re   *regexp.Regexp
	with string
}{
	{regexp.MustCompile(`^time=\S+ `), "time=T "},
	{regexp.MustCompile(` tid=\d+`), " tid=N"},
	{regexp.MustCompile(` transaction=[0-9a-f]+`), " transaction=X"},
	// the Rust agent masks the stream API key (D31, D34; unit-tested in the daemon and streaming crates)
	{regexp.MustCompile(`key=[^&" ]*`), "key=K"},
	{regexp.MustCompile(`api_key:'[^']*'`), "api_key:'K'"},
	{regexp.MustCompile(` src_port=\d+`), " src_port=P"},
	{regexp.MustCompile(`\]:\d+`), "]:P"},
	{regexp.MustCompile(` ([a-z_]+_ut)=\d+`), " ${1}=U"},
	{regexp.MustCompile(`thread=(WEB|STREAM|UV_WORKER)\[\d+\]`), "thread=${1}[n]"},
	// whichever tier thread takes the spawn lock first logs the registry's pre-population (D63.2)
	{regexp.MustCompile(`thread=DBENGINIT\[\d+\] (msg="MRG: Loaded )`), "thread=DBENGINIT[n] ${1}"},
	{regexp.MustCompile(`STREAM RCV\[\d+\]`), "STREAM RCV[n]"},
	{regexp.MustCompile(`in +\d+ ms, `), "in N ms, "},
	{regexp.MustCompile(`connected=\d+s idle=\d+s`), "connected=Ns idle=Ns"},
	{regexp.MustCompile(`completed in \d+ ms`), "completed in N ms"},
	{regexp.MustCompile(`\{at [^}]*\}`), "{at T}"},
	{regexp.MustCompile(`(finished '[^']*') in [^"]*"`), "${1} in D\""},
	{regexp.MustCompile(`Shutdown process ended in [^"]*"`), "Shutdown process ended in D\""},
	{regexp.MustCompile(`0x[0-9A-F]{16}`), "0xPTR"},
	{regexp.MustCompile(`task id \d+`), "task id N"},
	{regexp.MustCompile(`(loaded in|handled directly, in) [^"]*"`), "${1} D\""},
	{regexp.MustCompile(`(Progress of metadata storage: +[0-9.]+% completed) in [^"]*"`), "${1} in D\""},
	{regexp.MustCompile(`\(fd \d+\)|on fd \d+`), "fd N"},
	{regexp.MustCompile(`stopped after \d+ connects, \d+ disconnects \(max concurrent \d+\), \d+ receptions and \d+ sends`), "stopped after C"},
	{regexp.MustCompile(`(MRG: Loaded \d+ metrics from database in) [^"]*"`), "${1} D\""},
	{regexp.MustCompile(`currently available: [^,]*,`), "currently available: M,"},
	{regexp.MustCompile(`(populated, size: [^,]*, metrics: [^,]*), [0-9.]+ ms"`), "${1}, N ms\""},
	{regexp.MustCompile(`, mmap: [0-9.]+ ms, validate: [0-9.]+ ms"`), ", mmap: N ms, validate: N ms\""},
	{regexp.MustCompile(`(host context cleanup items|dimension delete items) in [0-9.]+ ms"`), "${1} in N ms\""},
}

var (
	threadRe  = regexp.MustCompile(` thread=(\S+)`)
	errnoRe   = regexp.MustCompile(` errno="[^"]*"`)
	webStopRe = regexp.MustCompile(`stopped after (\d+) connects, (\d+) disconnects \(max concurrent \d+\), (\d+) receptions and (\d+) sends`)
)

// logLines reads one log file of a run directory; a missing file is empty.
func logLines(t *testing.T, runDir, name string) []string {
	t.Helper()
	f, err := os.Open(filepath.Join(runDir, "log", name))
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		t.Fatalf("parity: %v", err)
	}
	defer f.Close()
	var lines []string
	s := bufio.NewScanner(f)
	s.Buffer(make([]byte, 1<<20), 16<<20)
	for s.Scan() {
		if s.Text() != "" {
			lines = append(lines, s.Text())
		}
	}
	if err := s.Err(); err != nil {
		t.Fatalf("parity: %s: %v", name, err)
	}
	return lines
}

// threadOf is a record's thread tag; the main thread has none.
func threadOf(line string) string {
	if m := threadRe.FindStringSubmatch(line); m != nil {
		return m[1]
	}
	return ""
}

// normalizeLog masks a record. The main thread's and the shutdown watcher's records carry a stale errno in C, which
// the check ignores (D36), as do the command server's own lifecycle records (D57.2), the registry's pre-population
// record (D63.1) and the context loads' record (D64); the command server's read and libuv error records keep theirs. The harness picks each daemon's port.
func normalizeLog(line, runDir, port string) string {
	line = strings.ReplaceAll(line, runDir, "<RUN>")
	if port != "" {
		line = strings.ReplaceAll(line, "port "+port+",", "port <PORT>,")
		line = strings.ReplaceAll(line, ":"+port, ":<PORT>")
	}
	th := threadOf(line)
	if th == "" || th == "EXIT_WATCHER" || strings.Contains(line, `msg="MRG: Loaded `) ||
		strings.Contains(line, `msg="RRDCONTEXT: metadata for node `) ||
		(th == "DAEMON_COMMAND" && !strings.Contains(line, `msg="pipe_read_cb: `) && !strings.Contains(line, `msg="uv_`)) {
		line = errnoRe.ReplaceAllString(line, "")
	}
	for _, m := range logMasks {
		line = m.re.ReplaceAllString(line, m.with)
	}
	return line
}

// webTotals sums the web threads' connects, disconnects and receptions: C's connections spread over the threads
// differently than Rust's. The sends are left out: C counts one more writable event per response, which its client
// state machine needs and Rust's does not (D44).
func webTotals(lines []string) [3]int {
	var total [3]int
	for _, l := range lines {
		if m := webStopRe.FindStringSubmatch(l); m != nil {
			for i := range total {
				n, _ := strconv.Atoi(m[i+1])
				total[i] += n
			}
		}
	}
	return total
}

// probeRe is the harness's readiness probe: its body is the `/api/v1/info` answer, which the candidate does not
// produce byte for byte yet, and C answers it 503 until it is ready, so its connections (found by their client
// port) are left out on both sides.
var (
	probeRe = regexp.MustCompile(` src_port=(\d+) .* request=/api/v1/info$`)
	portRe  = regexp.MustCompile(` src_port=(\d+) `)
)

// probePorts are the client ports of the readiness probe's connections.
func probePorts(access []string) map[string]bool {
	ports := map[string]bool{}
	for _, l := range access {
		if m := probeRe.FindStringSubmatch(l); m != nil {
			ports[m[1]] = true
		}
	}
	return ports
}

// logClasses splits records into what must match in order (the main thread, the shutdown watcher, the access log)
// and what must match as a multiset (worker threads whose interleaving is not deterministic).
func logClasses(lines []string, d *daemon.Daemon, probes map[string]bool, oracle bool) (ordered map[string][]string, unordered map[string][]string, dropped int) {
	_, port, _ := strings.Cut(d.Addr, ":")
	runDir := d.Opts.RunDir
	ordered, unordered = map[string][]string{}, map[string][]string{}
next:
	for _, l := range lines {
		if m := portRe.FindStringSubmatch(l); m != nil && probes[m[1]] {
			continue
		}
		if timedRecords.MatchString(l) {
			continue
		}
		th := threadOf(l)
		if oracle {
			base, _, _ := strings.Cut(th, "[")
			if _, ok := cOnlyThreads[base]; ok {
				dropped++
				continue
			}
			for _, c := range cOnlyRecords {
				if c.re.MatchString(l) && !portedRecords.MatchString(l) {
					dropped++
					continue next
				}
			}
		}
		n := normalizeLog(l, runDir, port)
		switch base, _, _ := strings.Cut(th, "["); base {
		case "", "EXIT_WATCHER":
			ordered[base] = append(ordered[base], n)
		default:
			unordered[base] = append(unordered[base], n)
		}
	}
	return ordered, unordered, dropped
}

// logWorkload sends the same requests to a daemon, each with a response the candidate already serves byte for byte:
// static files, a missing file, an unknown API command, two requests on one keep-alive connection, and a STREAM
// request with an unknown key; then a child streams a chart for a few seconds and disconnects.
func logWorkload(t *testing.T, d *daemon.Daemon) {
	t.Helper()
	requests := []string{
		"GET /index.html HTTP/1.1\r\nConnection: close\r\n\r\n",
		"GET /nonexistent HTTP/1.1\r\nConnection: close\r\n\r\n",
		"GET /api/v1/nope HTTP/1.1\r\nConnection: close\r\n\r\n",
		"GET /favicon.ico HTTP/1.1\r\n\r\nGET /nothing-here HTTP/1.1\r\nConnection: close\r\n\r\n",
		"STREAM key=5a1e0000-0000-4000-8000-00000000dead&hostname=log-child&registry_hostname=log-child" +
			"&machine_guid=5a1e0000-0000-4000-8000-0000000000cc&update_every=1&ver=1 HTTP/1.1\r\n" +
			"User-Agent: netdata/v0\r\nAccept: */*\r\n\r\n",
	}
	for _, r := range requests {
		if _, err := rawExchange(d.Addr, []byte(r), 2*time.Second); err != nil {
			t.Fatalf("parity: %v", err)
		}
	}
	conn, err := stream.Connect(d.Addr, d.StreamKey, childHost, stream.CapsLive)
	if err != nil {
		t.Fatalf("parity: %v", err)
	}
	conn.Linef("CHART 'log.test' '' 'title' 'units' 'family' 'log.test' line 1000 1 '' fixture-pusher corpus")
	conn.Linef("DIMENSION 'd1' '' absolute 1 1 ''")
	now := time.Now().Unix()
	for i := int64(3); i >= 1; i-- {
		conn.Linef("BEGIN2 'log.test' 1 %d #", now-i)
		conn.Linef("SET2 'd1' %d %d A", i, i)
		conn.Linef("END2")
	}
	if err := conn.Flush(); err != nil {
		t.Fatalf("parity: %v", err)
	}
	time.Sleep(500 * time.Millisecond)
	conn.Close()
	time.Sleep(500 * time.Millisecond)
}

// diffSequences lists the records only one side has, as a longest-common-subsequence diff keeps them in place.
func diffSequences(a, b []string) string {
	lcs := make([][]int, len(a)+1)
	for i := range lcs {
		lcs[i] = make([]int, len(b)+1)
	}
	for i := len(a) - 1; i >= 0; i-- {
		for j := len(b) - 1; j >= 0; j-- {
			if a[i] == b[j] {
				lcs[i][j] = lcs[i+1][j+1] + 1
			} else {
				lcs[i][j] = max(lcs[i+1][j], lcs[i][j+1])
			}
		}
	}
	var out strings.Builder
	i, j := 0, 0
	for i < len(a) || j < len(b) {
		switch {
		case i < len(a) && j < len(b) && a[i] == b[j]:
			i, j = i+1, j+1
		case j < len(b) && (i == len(a) || lcs[i][j+1] >= lcs[i+1][j]):
			fmt.Fprintf(&out, "  candidate only (at %d): %s\n", j, b[j])
			j++
		default:
			fmt.Fprintf(&out, "  oracle only (at %d):    %s\n", i, a[i])
			i++
		}
		if out.Len() > 8000 {
			out.WriteString("  ...\n")
			break
		}
	}
	return out.String()
}

func diffMultisets(a, b []string) string {
	count := map[string]int{}
	for _, x := range a {
		count[x]++
	}
	for _, y := range b {
		count[y]--
	}
	var out []string
	for k, n := range count {
		switch {
		case n > 0:
			out = append(out, fmt.Sprintf("  oracle only (x%d): %s", n, k))
		case n < 0:
			out = append(out, fmt.Sprintf("  candidate only (x%d): %s", -n, k))
		}
	}
	sort.Strings(out)
	return strings.Join(out, "\n")
}

// TestLogParity (check `log.parity`, logging step L6): after the same workload and a SIGTERM, both daemons' daemon
// and access logs hold the same records once the values that differ between runs are masked. The main thread and the
// shutdown watcher match record by record; worker threads (the access log's included) match as multisets.
func TestLogParity(t *testing.T) {
	compareLogs(t, "", 1)
}

// TestLogParityDebug is TestLogParity with `[logs] level = debug`, which adds the connection records and the
// debug records of startup, hosts and streaming.
func TestLogParityDebug(t *testing.T) {
	compareLogs(t, "    level = debug\n", 1)
}

// TestLogParityTiers is TestLogParityDebug with three dbengine tiers: their start threads in parallel (the registry's
// pre-population on whichever takes the lock first), each tier made ready, quiesced and stopped on its own thread.
func TestLogParityTiers(t *testing.T) {
	compareLogs(t, "    level = debug\n", 3)
}

func compareLogs(t *testing.T, logs string, tiers int) {
	p := StartPair(t, daemon.Options{WebDir: oracleWebDir(t), LogsExtra: logs, StreamMemoryMode: "ram", StorageTiers: tiers}, parentIdentity)
	for _, side := range p.Each() {
		logWorkload(t, side.Daemon)
	}
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("parity: stop %s: %v", side.Role, err)
		}
	}
	compareLogFiles(t, p)
}

// compareLogFiles compares the logs of a pair whose daemons have exited (daemon.log and access.log unless names are
// given): the main thread's and the shutdown watcher's records in order, every other thread's as a multiset, without
// the oracle's records of unported subsystems.
func compareLogFiles(t *testing.T, p *Pair, names ...string) {
	t.Helper()
	if len(names) == 0 {
		names = []string{"daemon.log", "access.log"}
	}
	oProbes := probePorts(logLines(t, p.Oracle.Opts.RunDir, "access.log"))
	cProbes := probePorts(logLines(t, p.Candidate.Opts.RunDir, "access.log"))
	for _, name := range names {
		o, c := logLines(t, p.Oracle.Opts.RunDir, name), logLines(t, p.Candidate.Opts.RunDir, name)
		if name == "daemon.log" {
			// each probe connection is one connect, one disconnect and one reception
			wo, wc := webTotals(o), webTotals(c)
			for i := range wo {
				wo[i] -= len(oProbes)
				wc[i] -= len(cProbes)
			}
			if wo != wc {
				t.Errorf("%s: web threads' connects, disconnects and receptions: oracle %v, candidate %v", name, wo, wc)
			}
		}
		oo, ou, dropped := logClasses(o, p.Oracle, oProbes, true)
		co, cu, _ := logClasses(c, p.Candidate, cProbes, false)
		t.Logf("%s: %d oracle records of unported subsystems left out", name, dropped)
		for _, class := range sortedKeys(oo, co) {
			if d := diffSequences(oo[class], co[class]); d != "" {
				t.Errorf("%s, thread %q, in order:\n%s", name, class, d)
			}
		}
		for _, class := range sortedKeys(ou, cu) {
			if d := diffMultisets(ou[class], cu[class]); d != "" {
				t.Errorf("%s, thread %q, as a multiset:\n%s", name, class, d)
			}
		}
	}
}

func sortedKeys(a, b map[string][]string) []string {
	seen := map[string]bool{}
	for k := range a {
		seen[k] = true
	}
	for k := range b {
		seen[k] = true
	}
	var keys []string
	for k := range seen {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}
