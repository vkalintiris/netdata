// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"regexp"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// confPending lists the netdata.conf keys the oracle creates that the candidate does not read yet, each with the
// subsystem that will read it. A pending key is left out of the comparison on both sides; it must exist in the
// oracle's dump, and the candidate may show it only as a key it does not use (a file key). Remove an entry when its
// subsystem lands: the check then compares it.
var confPending = map[string]string{}

func pending(reason, section string, keys ...string) {
	for _, k := range keys {
		confPending["["+section+"] "+k] = reason
	}
}

func init() {
	pending("cloud/ACLK", "cloud", "query threads")
	pending("ml", "ml", "enabled", "training window", "min training window", "max training vectors",
		"max samples to smooth", "train every", "number of models per dimension", "delete models older than",
		"num samples to diff", "num samples to lag", "maximum number of k-means iterations",
		"dimension anomaly score threshold", "host anomaly rate threshold", "anomaly detection grouping method",
		"anomaly detection grouping duration", "num training threads", "flush models batch size",
		"dimension anomaly rate suppression window", "dimension anomaly rate suppression threshold",
		"enable statistics charts", "hosts to skip from training", "charts to skip from training",
		"stream anomaly detection charts")
	pending("registry", "registry", "netdata management api key file")
	pending("internal collectors", "plugins", "proc", "diskspace", "cgroups", "tc", "idlejitter", "timex", "profile")
	pending("statsd", "plugins", "statsd")
	pending("statsd", "statsd", "update every (flushInterval)", "udp messages to process at once",
		"create private charts for metrics matching", "max private charts hard limit", "set charts as obsolete after",
		"decimal detail", "disconnect idle tcp clients after", "private charts hidden",
		"histograms and timers percentile (percentThreshold)", "dictionaries max unique dimensions",
		"add dimension for number of events received", "gaps on gauges (deleteGauges)",
		"gaps on counters (deleteCounters)", "gaps on meters (deleteMeters)", "gaps on sets (deleteSets)",
		"gaps on histograms (deleteHistograms)", "gaps on timers (deleteTimers)",
		"gaps on dictionaries (deleteDictionaries)", "statsd server max TCP sockets")
}

// confEntry is one key of a dump: its annotation lines and its value line, blank lines dropped.
type confEntry struct {
	key    string
	block  []string
	unused bool // a file key the daemon does not read
}

type confSection struct {
	name    string
	notUsed bool
	entries []confEntry
}

type confDump struct {
	header   string
	sections []confSection
}

// parseConfDump splits a /netdata.conf body (inicfg_generate()) into sections and entries.
func parseConfDump(body string) confDump {
	var d confDump
	var cur *confSection
	notUsed := false
	var block []string
	var header strings.Builder
	for _, line := range strings.Split(body, "\n") {
		switch {
		case strings.HasPrefix(line, "# section '"):
			notUsed = true
			continue
		case strings.HasPrefix(line, "[") && strings.HasSuffix(line, "]"):
			d.sections = append(d.sections, confSection{name: line[1 : len(line)-1], notUsed: notUsed})
			cur = &d.sections[len(d.sections)-1]
			notUsed, block = false, nil
			continue
		case cur == nil:
			header.WriteString(line + "\n")
			continue
		case strings.TrimSpace(line) == "":
			continue
		}
		block = append(block, line)
		if strings.HasPrefix(line, "\t#|") {
			continue
		}
		v := strings.TrimPrefix(strings.TrimPrefix(line, "\t"), "# ")
		key, _, _ := strings.Cut(v, " = ")
		unused := cur.notUsed
		for _, b := range block {
			unused = unused || strings.Contains(b, "found in the config file, but is not used")
		}
		cur.entries = append(cur.entries, confEntry{key: key, block: block, unused: unused})
		block = nil
	}
	d.header = header.String()
	return d
}

// normalizeConf replaces what differs between the two daemons of a pair by design: run directories and ports.
func normalizeConf(body []byte, d *daemon.Daemon) string {
	s := string(body)
	s = strings.ReplaceAll(s, d.Opts.RunDir, "<rundir>")
	s = strings.ReplaceAll(s, ":"+strconv.Itoa(d.Opts.Port), ":<port>")
	return s
}

// TestNetdataConf compares /netdata.conf: every key both daemons read must print identically, in the same sections
// and order, with the same annotations; keys only the oracle reads must be listed in confPending. The candidate must
// be built with the oracle's compile-time paths (RECIPES.md "Building the candidate for parity checks").
func TestNetdataConf(t *testing.T) {
	compareNetdataConf(t, daemon.Options{})
}

// TestNetdataConfStandaloneProfile: the node profile, not an enabled stream API key, doubles the web server threads.
func TestNetdataConfStandaloneProfile(t *testing.T) {
	compareNetdataConf(t, daemon.Options{GlobalExtra: "    profile = standalone\n"})
}

// TestNetdataConfListenerPort: with NETDATA_LISTENER_PORT set (as in container images) the plugins thread seeds
// `[plugins] freeipmi = no` before its first scan (`plugins_d.c:309-310`), so the key comes before the plugins found.
func TestNetdataConfListenerPort(t *testing.T) {
	p := compareNetdataConf(t, daemon.Options{Env: []string{"NETDATA_LISTENER_PORT=19999"}})
	b, err := rawExchange(p.Oracle.Addr, []byte("GET /netdata.conf HTTP/1.1\r\n\r\n"), 5*time.Second)
	// read with a default, neither loaded nor changed: C dumps it commented (`inicfg_conf_file.c:444-450`)
	if err != nil || !bytes.Contains(b, []byte("\t# freeipmi = no\n")) {
		t.Errorf("the oracle's dump has no freeipmi key: %v", err)
	}
}

// TestNetdataConfProfiles: `[global] profile = child` and `iot` while stream.conf enables an API key (which the
// detection would make a parent): each dump shows the profile's defaults (M7 10f, D122.11).
func TestNetdataConfProfiles(t *testing.T) {
	for _, profile := range []string{"child", "iot"} {
		t.Run(profile, func(t *testing.T) {
			p := compareNetdataConf(t, daemon.Options{GlobalExtra: "    profile = " + profile + "\n"})
			b, err := rawExchange(p.Oracle.Addr, []byte("GET /netdata.conf HTTP/1.1\r\n\r\n"), 5*time.Second)
			if err != nil || !bytes.Contains(b, []byte("\tprofile = "+profile+"\n")) {
				t.Errorf("the oracle's dump does not hold its profile %s: %v", profile, err)
			}
		})
	}
}

// dbClampRecords are the main thread's records about the [db] keys.
var dbClampRecords = regexp.MustCompile(`(?i)msg="[^"]*(dbengine|dbegnine|tier|page cache|extent|pages per|backfill|storage|disk space)`)

// TestNetdataConfDbClamps gives both daemons out-of-range [db] values (check `api.netdata-conf`, S6a commit 2): the
// dump shows C's clamps and write-backs, and the main thread's warnings and errors about them come in C's order.
// Run A: a page cache under its minimum, an extent cache over the int range, no pages per extent, an unknown backfill,
// a tier 1 grouping of 1, tiers 0 and 1 under the minimum quota, 9 tiers; run B: too many pages per extent.
func TestNetdataConfDbClamps(t *testing.T) {
	runs := map[string]daemon.Options{
		"A": {StorageTiers: 9, TierRetentionMB: [3]int{10, 10}, TierGrouping: [3]int{0, 1},
			DBExtra: "    dbengine page cache size = 4MiB\n    dbengine extent cache size = 3PiB\n" +
				"    dbengine pages per extent = 0\n    dbengine tier backfill = bogus\n"},
		"B": {StorageTiers: 1, DBExtra: "    dbengine pages per extent = 110\n"},
	}
	for name, opts := range runs {
		t.Run(name, func(t *testing.T) {
			p := compareNetdataConf(t, opts)
			var got [2][]string
			for i, side := range p.Each() {
				if err := side.Daemon.Stop(); err != nil {
					t.Fatalf("stop %s: %v", side.Role, err)
				}
			next:
				for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
					// on the oracle, the records of subsystems not ported yet, as the log checks drop them
					for _, c := range cOnlyRecords {
						if side.Role == Oracle && c.re.MatchString(l) && !portedRecords.MatchString(l) {
							continue next
						}
					}
					if threadOf(l) == "" && (strings.Contains(l, "level=warning") || strings.Contains(l, "level=error")) &&
						dbClampRecords.MatchString(l) {
						got[i] = append(got[i], normalizeLog(l, side.Daemon.Opts.RunDir, strconv.Itoa(side.Daemon.Opts.Port)))
					}
				}
			}
			if d := diffSequences(got[0], got[1]); d != "" {
				t.Errorf("the main thread's warnings and errors differ:\n%s", d)
			}
			if len(got[0]) == 0 {
				t.Error("the oracle wrote no warning or error about the clamps")
			}
		})
	}
}

func compareNetdataConf(t *testing.T, opts daemon.Options) *Pair {
	p := StartPair(t, opts, parentIdentity)
	compareNetdataConfOn(t, p)
	return p
}

// compareNetdataConfOn compares a running pair's /netdata.conf dumps (compareNetdataConf's rules).
func compareNetdataConfOn(t *testing.T, p *Pair) {
	t.Helper()
	var dumps [2]confDump
	var heads [2][]byte
	for i, side := range p.Each() {
		b, err := rawExchange(side.Daemon.Addr, []byte("GET /netdata.conf HTTP/1.1\r\n\r\n"), 5*time.Second)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		head, body, ok := bytes.Cut(b, []byte("\r\n\r\n"))
		if !ok || !bytes.HasPrefix(head, []byte("HTTP/1.1 200 OK\r\n")) {
			t.Fatalf("%s: no 200 answer:\n%s", side.Role, head)
		}
		// The bodies differ in length while keys are pending.
		heads[i] = contentLengthRe.ReplaceAll(maskRaw(head), []byte("Content-Length: <masked>"))
		dumps[i] = parseConfDump(normalizeConf(body, side.Daemon))
	}
	if !bytes.Equal(heads[0], heads[1]) {
		t.Errorf("headers differ\n%s", firstDifference(heads[0], heads[1]))
	}
	oracle, candidate := dumps[0], dumps[1]
	if oracle.header != candidate.header {
		t.Errorf("dump headers differ\n%s", firstLineDifference(oracle.header, candidate.header))
	}

	seen := map[string]bool{}
	// the reminder is for a candidate that is still porting the keys; the oracle against itself reads them all
	bins := binaries(t)
	selfRun := bins[0] == bins[1]
	strip := func(d confDump, side Role) []confSection {
		var out []confSection
		for _, s := range d.sections {
			kept := confSection{name: s.name, notUsed: s.notUsed}
			for _, e := range s.entries {
				id := "[" + s.name + "] " + e.key
				if _, isPending := confPending[id]; isPending {
					if side == Oracle {
						seen[id] = true
					} else if !e.unused && !selfRun {
						t.Errorf("%s is read by the candidate now: remove it from confPending", id)
					}
					continue
				}
				kept.entries = append(kept.entries, e)
			}
			if len(kept.entries) > 0 {
				out = append(out, kept)
			}
		}
		return out
	}
	want, got := strip(oracle, Oracle), strip(candidate, Candidate)
	for id, reason := range confPending {
		if !seen[id] {
			t.Errorf("confPending lists %s (%s), which the oracle does not create", id, reason)
		}
	}

	render := func(ss []confSection) string {
		var b strings.Builder
		for _, s := range ss {
			if s.notUsed {
				fmt.Fprintf(&b, "# section '%s' is not used.\n", s.name)
			}
			fmt.Fprintf(&b, "[%s]\n", s.name)
			for _, e := range s.entries {
				b.WriteString(strings.Join(e.block, "\n") + "\n")
			}
		}
		return b.String()
	}
	if w, g := render(want), render(got); w != g {
		t.Errorf("dumps differ outside confPending\n%s", firstLineDifference(w, g))
	}
	pendingBySubsystem := map[string]int{}
	for _, reason := range confPending {
		pendingBySubsystem[reason]++
	}
	t.Logf("compared %d sections; %d keys pending: %v", len(want), len(confPending), pendingBySubsystem)
}

// firstLineDifference shows the first differing line of two texts, with the section it belongs to.
func firstLineDifference(a, b string) string {
	la, lb := strings.Split(a, "\n"), strings.Split(b, "\n")
	section := ""
	for i := 0; i < max(len(la), len(lb)); i++ {
		x, y := "<end>", "<end>"
		if i < len(la) {
			x = la[i]
		}
		if i < len(lb) {
			y = lb[i]
		}
		if x != y {
			return fmt.Sprintf("line %d, in %s\noracle:    %q\ncandidate: %q", i+1, section, x, y)
		}
		if strings.HasPrefix(x, "[") {
			section = x
		}
	}
	return "no difference"
}

// TestNetdataConfParserSeesEveryKey guards the dump parser on a fixed dump.
func TestNetdataConfParserSeesEveryKey(t *testing.T) {
	body := "# netdata configuration\n\n[global]\n\t#| >>> [global].hostname <<<\n" +
		"\t#| datatype: text, default value: box\n\thostname = parent\n\n\t# run as user = netdata\n\n" +
		"[db]\n\t#| >>> [db].db <<<\n\t#| found in the config file, but is not used\n\tdb = dbengine\n\n" +
		"# section 'ml' is not used.\n[ml]\n\tenabled = no\n"
	d := parseConfDump(body)
	var got []string
	for _, s := range d.sections {
		for _, e := range s.entries {
			got = append(got, fmt.Sprintf("%s/%s/%v/%d", s.name, e.key, e.unused, len(e.block)))
		}
	}
	want := "global/hostname/false/3 global/run as user/false/1 db/db/true/3 ml/enabled/true/1"
	if strings.Join(got, " ") != want {
		t.Fatalf("parsed %q, want %q", strings.Join(got, " "), want)
	}
	if d.header != "# netdata configuration\n\n" {
		t.Fatalf("header %q", d.header)
	}
}
