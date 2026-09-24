// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
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
	pending("daemon status file", "global", "crash reports")
	pending("host labels", "global", "is ephemeral node", "has unstable connection")
	pending("dbengine (D4)", "db", "storage tiers", "dbengine tier 0 retention time", "dbengine tier 1 retention time",
		"dbengine tier 2 retention time", "dbengine page type", "dbengine page cache size", "dbengine extent cache size",
		"dbengine enable journal integrity check", "dbengine use all ram for caches", "dbengine out of memory protection",
		"dbengine use direct io", "dbengine journal v2 unmount time", "dbengine pages per extent",
		"dbengine tier backfill", "dbengine tier 1 update every iterations", "dbengine tier 2 update every iterations",
		"dbengine tier 0 retention size", "dbengine tier 1 retention size", "dbengine tier 2 retention size")
	pending("contexts engine (extreme cardinality)", "db", "extreme cardinality protection",
		"extreme cardinality keep instances", "extreme cardinality min ephemerality")
	pending("registry", "directories", "registry")
	pending("logging (B5)", "logs", "facility", "logs flood protection period", "logs to trigger flood protection",
		"level", "debug", "daemon", "collector", "access", "health", "debug flags")
	pending("cloud/ACLK", "cloud", "conversation log", "scope", "query threads", "proxy")
	pending("ml", "ml", "enabled", "training window", "min training window", "max training vectors",
		"max samples to smooth", "train every", "number of models per dimension", "delete models older than",
		"num samples to diff", "num samples to lag", "maximum number of k-means iterations",
		"dimension anomaly score threshold", "host anomaly rate threshold", "anomaly detection grouping method",
		"anomaly detection grouping duration", "num training threads", "flush models batch size",
		"dimension anomaly rate suppression window", "dimension anomaly rate suppression threshold",
		"enable statistics charts", "hosts to skip from training", "charts to skip from training",
		"stream anomaly detection charts")
	pending("health", "health", "enabled", "silencers file", "enable stock health configuration",
		"use summary for notifications", "default repeat warning", "default repeat critical",
		"in memory max health log entries", "health log retention", "script to execute on alarm", "enabled alarms",
		"run at least every", "postpone alarms during hibernation for", "notification execution timeout")
	pending("web TLS", "web", "ssl key", "ssl certificate", "tls version", "tls ciphers",
		"ssl skip certificate verification")
	pending("bearer tokens", "web", "bearer token protection")
	pending("registry", "registry", "enabled", "registry db file", "registry log file",
		"registry save db every new entries", "registry expire idle persons", "registry domain", "registry to announce",
		"registry hostname", "verify browser cookies support", "enable cookies SameSite and Secure", "max URL length",
		"max URL name length", "netdata management api key file")
	pending("pulse", "pulse", "extended", "update every")
	pending("pulse", "plugins", "netdata pulse")
	// go.d is a file key C does not read either; its annotation depends on the section having a used key.
	pending("plugins.d", "plugins", "enable running new plugins", "check for new plugins every", "cups", "xenstat",
		"systemd-units", "nfacct", "scripts.d", "ebpf-go", "freeipmi", "otel", "apps", "go.d", "charts.d", "python.d",
		"debugfs", "perf", "slabinfo", "ioping", "ebpf", "systemd-journal", "network-viewer")
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
	p := StartPair(t, daemon.Options{}, parentIdentity)
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
	strip := func(d confDump, side Role) []confSection {
		var out []confSection
		for _, s := range d.sections {
			kept := confSection{name: s.name, notUsed: s.notUsed}
			for _, e := range s.entries {
				id := "[" + s.name + "] " + e.key
				if _, isPending := confPending[id]; isPending {
					if side == Oracle {
						seen[id] = true
					} else if !e.unused {
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
