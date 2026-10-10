// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// infoIdentity splits an /api/v1/info answer into what must match byte for byte (version, uid, and the members from
// "alarms" up to "host_labels") and the host labels, compared as a map (C orders them by heap address).
func infoIdentity(t *testing.T, addr, path string) (string, map[string]any) {
	t.Helper()
	b, err := rawExchange(addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 10*time.Second)
	if err != nil {
		t.Fatalf("%s: %v", path, err)
	}
	body := httpBody(b)
	var doc struct {
		Version    string         `json:"version"`
		UID        string         `json:"uid"`
		HostLabels map[string]any `json:"host_labels"`
	}
	if err := json.Unmarshal(body, &doc); err != nil {
		t.Fatalf("%s: %v: %s", path, err, body)
	}
	from := bytes.Index(body, []byte(`"alarms":`))
	to := bytes.Index(body, []byte(`"host_labels":`))
	if from < 0 || to < from {
		t.Fatalf("%s: no alarms..host_labels members: %s", path, body)
	}
	return doc.Version + " " + doc.UID + "\n" + string(body[from:to]), doc.HostLabels
}

// contextsHostLabels are the host labels /api/v1/contexts shows for localhost.
func contextsHostLabels(t *testing.T, addr string) map[string]any {
	t.Helper()
	b, err := rawExchange(addr, []byte("GET /api/v1/contexts?options=labels HTTP/1.1\r\n\r\n"), 10*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	var doc struct {
		HostLabels map[string]any `json:"host_labels"`
	}
	if err := json.Unmarshal(httpBody(b), &doc); err != nil {
		t.Fatalf("contexts: %v", err)
	}
	return doc.HostLabels
}

// identityRecords are the main-thread records of the identity steps, in order, from a daemon's log.
func identityRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	msgRe := regexp.MustCompile(`msg="((?:[^"\\]|\\.)*)"`)
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		m := msgRe.FindStringSubmatch(l)
		if m == nil || threadOf(l) != "" {
			continue
		}
		for _, prefix := range []string{"SYSTEM INFO:", "RRDLABEL:", "Kubernetes pod label", "ACLK: proxy"} {
			if strings.HasPrefix(m[1], prefix) {
				out = append(out, m[1])
			}
		}
	}
	return out
}

func compareIdentity(t *testing.T, p *Pair, stage, path string) {
	t.Helper()
	var text [2]string
	var labels [2]map[string]any
	for i, side := range p.Each() {
		text[i], labels[i] = infoIdentity(t, side.Daemon.Addr, path)
	}
	if text[0] != text[1] {
		t.Errorf("%s %s: differs\n%s", stage, path, firstDifference([]byte(text[0]), []byte(text[1])))
	}
	if !reflect.DeepEqual(labels[0], labels[1]) {
		t.Errorf("%s %s: host labels differ\noracle:    %v\ncandidate: %v", stage, path, labels[0], labels[1])
	}
}

// TestLocalhostIdentity compares what both daemons say about localhost (system info and host labels in
// /api/v1/info and /api/v1/contexts) and about a connected child, before, while and after a child streams in (for
// `_is_parent`), with `[host labels]` that expand environment variables; then the same without any plugin scripts,
// with the records of the failures. Check `api.localhost-identity`.
func TestLocalhostIdentity(t *testing.T) {
	// a set variable, one the system-info script exports, and an unset one
	t.Setenv("SP2_TEST_VAR", "from-env")
	labels := "    sp2_label = hello\n    sp2_env = ${SP2_TEST_VAR:-fallback}\n    sp2_sys = ${NETDATA_SYSTEM_KERNEL_NAME}\n" +
		"    sp2_unset = ${SP2_UNSET_VAR}\n"
	p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, HostLabels: labels}, parentIdentity)
	compareIdentity(t, p, "standalone", "/api/v1/info")
	var contexts [2]map[string]any
	for i, side := range p.Each() {
		contexts[i] = contextsHostLabels(t, side.Daemon.Addr)
	}
	if !reflect.DeepEqual(contexts[0], contexts[1]) {
		t.Errorf("contexts host labels differ\noracle:    %v\ncandidate: %v", contexts[0], contexts[1])
	}
	if contexts[1]["_is_parent"] != "false" || contexts[1]["sp2_env"] != "from-env" || contexts[1]["sp2_sys"] != "Linux" {
		t.Errorf("candidate labels: %v", contexts[1])
	}
	var conns []*stream.Conn
	for _, side := range p.Each() {
		conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsLive)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		conns = append(conns, conn)
	}
	time.Sleep(time.Second)
	compareIdentity(t, p, "child connected", "/api/v1/info")
	compareIdentity(t, p, "child connected", "/host/"+childHost.Hostname+"/api/v1/info")
	if got := contextsHostLabels(t, p.Candidate.Addr)["_is_parent"]; got != "true" {
		t.Errorf("candidate _is_parent with a child: %v", got)
	}
	for _, c := range conns {
		_ = c.Close()
	}
	time.Sleep(2 * time.Second)
	compareIdentity(t, p, "child disconnected", "/api/v1/info")
	var records [2][]string
	for i, side := range p.Each() {
		records[i] = identityRecords(t, side.Daemon)
	}
	if !reflect.DeepEqual(records[0], records[1]) {
		t.Errorf("identity records differ\noracle:    %q\ncandidate: %q", records[0], records[1])
	}
	unset := "RRDLABEL: environment variable 'SP2_UNSET_VAR' is not set and no default provided"
	if !slices.Contains(records[1], unset) {
		t.Errorf("candidate records lack %q: %q", unset, records[1])
	}

	t.Run("no-scripts", func(t *testing.T) {
		plugins := filepath.Join(t.TempDir(), "plugins.d")
		if err := os.Mkdir(plugins, 0o755); err != nil {
			t.Fatal(err)
		}
		p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, PluginsDir: plugins}, parentIdentity)
		compareIdentity(t, p, "no scripts", "/api/v1/info")
		var records [2][]string
		for i, side := range p.Each() {
			records[i] = identityRecords(t, side.Daemon)
		}
		if !reflect.DeepEqual(records[0], records[1]) {
			t.Errorf("identity records differ\noracle:    %q\ncandidate: %q", records[0], records[1])
		}
		if len(records[1]) < 3 {
			t.Errorf("candidate failure records: %q", records[1])
		}
	})
}

// infoTailAge is how long after both agents are ready TestInfoV1Tail asks: C's ANALYTICS thread starts once the agent
// is ready (main.c:1395-1413), gathers its immutable members at its 10th second and its mutable ones after its 120th
// (analytics.c:655-697, analytics.h:10-11).
const infoTailAge = 12 * time.Second

// infoTailFamily compares `/api/v1/info` whole: its host labels as a map (C adds them from concurrent startup threads,
// `null.streamed-chart`'s rule; a label map is flat, so the layout is compared whole), and `buildinfo` by name
// (infoBuildinfoCheck, D251 F2 A): masked for the comparison of values and escapes, judged once the rest agrees.
var infoTailFamily = v2Family{
	masks:     []Mask{{Pattern: "buildinfo", Reason: "compared by name (infoBuildinfoCheck)"}},
	unordered: []string{"host_labels"},
	flat:      true,
	check:     infoBuildinfoCheck,
}

// infoBuildinfoNames are C's 40 analytics names of the build info slots, in the slots' order, each with its slot as
// -W buildinfojson names it (`section.key`): BUILD_INFO[]'s `.analytics` (daemon/buildinfo.c, the line of each name
// below), in the order of the enum BUILD_INFO_SLOT (:9-132), which analytics_build_info() walks (:1703-1718): it
// joins with `|` the names of the slots whose status holds. Two are set at runtime from stream.conf (:1548-1549):
// StreamParent (an API key is enabled) and StreamChild (the agent sends).
var infoBuildinfoNames = []struct{ name, slot string }{
	{"Netdata Cloud", "features.cloud"},                        // :488
	{"Stream Compression", "features.stream-compression"},      // :528
	{"Machine Learning", "features.ml"},                        // :552
	{"allocator", "features.allocator"},                        // :560
	{"dbengine", "databases.dbengine"},                         // :576
	{"Native HTTPS", "connectivity.native-https"},              // :632
	{"TLS Host Verification", "connectivity.tls-host-verify"},  // :640
	{"zlib", "libs.zlib"},                                      // :664
	{"protobuf", "libs.protobuf"},                              // :680
	{"JSON-C", "libs.jsonc"},                                   // :704
	{"libcap", "libs.libcap"},                                  // :712
	{"libcrypto", "libs.libcrypto"},                            // :720
	{"libyaml", "libs.libyaml"},                                // :728
	{"libmnl", "libs.libmnl"},                                  // :736
	{"stacktraces", "libs.stacktraces"},                        // :744
	{"apps", "plugins.apps"},                                   // :752
	{"cgroup Network Tracking", "plugins.cgroup-network"},      // :768
	{"MACOS-LOGS", "plugins.macos-logs"},                       // :816
	{"debugfs", "plugins.debugfs"},                             // :864
	{"CUPS", "plugins.cups"},                                   // :872
	{"EBPF", "plugins.ebpf"},                                   // :880
	{"IPMI", "plugins.freeipmi"},                               // :888
	{"NETWORK-VIEWER", "plugins.network-viewer"},               // :896
	{"SYSTEMD-JOURNAL", "plugins.systemd-journal"},             // :904
	{"WINDOWS-EVENTS", "plugins.windows-events"},               // :912
	{"NFACCT", "plugins.nfacct"},                               // :920
	{"perf", "plugins.perf"},                                   // :928
	{"slabinfo", "plugins.slabinfo"},                           // :936
	{"Xen", "plugins.xen"},                                     // :944
	{"Xen VBD Error Tracking", "plugins.xen-vbd-error"},        // :952
	{"AWS Kinesis", "exporters.kinesis"},                       // :1048
	{"GCP PubSub", "exporters.pubsub"},                         // :1056
	{"MongoDB", "exporters.mongodb"},                           // :960
	{"Prometheus Remote Write", "exporters.prom-remote-write"}, // :1040
	{"DebugTraceAlloc", "debug-n-devel.trace-allocations"},     // :1064
	{"ConfigProfile", "runtime.profile"},                       // :1080
	{"StreamParent", "runtime.parent"},                         // :1088
	{"StreamChild", "runtime.child"},                           // :1096
	{"TotalMemory", "runtime.mem-total"},                       // :1104
	{"AvailableMemory", "runtime.mem-available"},               // :1112
}

// infoBuildinfoProblems judges `/api/v1/info`'s `buildinfo` by name: o and c are the oracle's and the candidate's
// lists as printed (names joined by `|`, an empty text no name). Every name must be one of C's (infoBuildinfoNames).
// The candidate's must be the oracle's without the names whose slot is in buildinfoDiffs (the slots where the Rust
// agent says other than C's production build, D87.1), in the oracle's order; a listed name the candidate prints as
// the oracle does fails as cli.buildinfo's listed slots do: remove it from buildinfoDiffs. When the oracle is its own
// candidate (same) nothing is allowed: the lists are equal.
func infoBuildinfoProblems(o, c string, same bool) []string {
	slots := map[string]string{}
	for _, n := range infoBuildinfoNames {
		slots[n.name] = n.slot
	}
	var lists [2][]string
	var problems []string
	for i, text := range []string{o, c} {
		if text != "" {
			lists[i] = strings.Split(text, "|")
		}
		for _, name := range lists[i] {
			if _, ok := slots[name]; !ok {
				problems = append(problems, fmt.Sprintf("buildinfo: the %s prints %q, no analytics name of C's build "+
					"info (infoBuildinfoNames)", [2]Role{Oracle, Candidate}[i], name))
			}
		}
	}
	if problems != nil {
		return problems
	}
	var want []string
	for _, name := range lists[0] {
		d, listed := buildinfoDiffs[slots[name]]
		switch {
		case !listed || same:
			want = append(want, name)
		case slices.Contains(lists[1], name):
			problems = append(problems, fmt.Sprintf("buildinfo: %s (%s): the candidate now prints it as C does: remove "+
				"it from buildinfoDiffs (%s)", name, slots[name], d.closes))
		}
	}
	if !slices.Equal(want, lists[1]) {
		problems = append(problems, fmt.Sprintf("buildinfo: the candidate prints %q, want %q (the oracle's %q)", c,
			strings.Join(want, "|"), o))
	}
	return problems
}

// infoBuildinfoJudge is infoBuildinfoProblems on two parsed answers, each of which must hold `buildinfo` as a string
// (C prints it quoted, api_v1_info.c:153, once ready, :180).
func infoBuildinfoJudge(o, c Value, same bool) []string {
	var text [2]string
	for i, v := range [2]Value{o, c} {
		b, err := dashMember(v, "buildinfo")
		if err != nil || b.Kind != KindString {
			return []string{fmt.Sprintf("buildinfo: the %s's is %s, want a string (%v)", [2]Role{Oracle, Candidate}[i],
				b, err)}
		}
		text[i] = b.Text
	}
	return infoBuildinfoProblems(text[0], text[1], same)
}

// infoBuildinfoCheck is infoTailFamily's check: the two answers' `buildinfo` by name (infoBuildinfoJudge), with no
// allowance when the oracle is its own candidate (sameBinary).
func infoBuildinfoCheck(t *testing.T, name string, o, c Value) {
	t.Helper()
	for _, problem := range infoBuildinfoJudge(o, c, sameBinary(t)) {
		t.Errorf("%s: %s", name, problem)
	}
}

// infoTailFixed are the oracle's members after `functions` (api_v1_info.c:134-172) that hold in every phase of a
// standalone pair or a pair with a plugin, once past the ANALYTICS thread's 10th second, for localhost and for a
// routed child of such a pair (whose own `stream-compression` and `ml-info` are false and `{"enabled":false}` as
// localhost's: the parent gives a child no sender and ML is off):
//   - the two literal cloud flags (:136-137); unclaimed, so no claim and no Cloud link (:138-139);
//   - the web server on (:146); localhost sends nothing (:147, :149: the launcher's stream.conf says `[stream] enabled
//     = no`, and there is no sender, stream-sender-api.c:14-16); the literal https flag (:151);
//   - the release channel of a version with a `-` and no `.environment` in the run directory (:154;
//     charts2json.c:7-48);
//   - exporting off by exporting.conf's default (:157; analytics.c:559), its connectors "" once gathered at the 10th
//     second, null before (:158; analytics.c:317-325, :1350);
//   - the four hit counters 0 (:160-163): the launcher writes `.opt-out-from-anonymous-statistics`, which C reads
//     before it is ready (main.c:1131, status-file.c:1553, :1287, analytics.c:1326-1340), and C then counts no hit
//     (analytics.c:231-278);
//   - ML off for localhost (:168-172; ml_public.cc:257-264).
var infoTailFixed = dashMembers(nil,
	"cloud-enabled", "true", "cloud-available", "true", "agent-claimed", "false", "aclk-available", "false",
	"web-enabled", "true", "stream-enabled", "false", "stream-compression", "false", "https-enabled", "true",
	"release-channel", `"nightly"`, "exporting-enabled", "false", "exporting-connectors", `""`,
	"allmetrics-prometheus-used", "0", "allmetrics-shell-used", "0", "allmetrics-json-used", "0",
	"dashboard-used", "0", "ml-info", `{"enabled":false}`)

// infoTailGuard checks the oracle's members after `functions` in TestInfoV1Tail's 12-115 s phase: infoTailFixed, then
// the phase's own: no collector (localhost's charts' plugin and module pairs, api_v1_info.c:5-35: no chart with the
// pulse off and no plugin), and the mutable analytics not gathered yet (notification methods null, charts and
// metrics 0 until the 120th second: :155, :165-166; analytics.c:603-611, :1345-1395).
var infoTailGuard = dashGuard(infoTailFixed, dashMembers(nil, "collectors", "[]", "notification-methods", "null",
	"charts-count", "0", "metrics-count", "0"))

// infoTailReq is TestInfoV1Tail's request.
var infoTailReq = v2Req{name: "info", target: "/api/v1/info", status: "200", guard: infoTailGuard}

// infoHelloReq is the dashboard's hello (the registry's action=hello), which C counts as a dashboard hit unless the
// agent opted out of anonymous statistics (api_v1_registry.c:125-127; analytics.c:271-278).
var infoHelloReq = v2Req{name: "hello", target: "/api/v1/registry?action=hello", status: "200"}

// infoHelloAnswered holds when a raw answer to infoHelloReq has its status.
func infoHelloAnswered(raw []byte) error {
	if !bytes.HasPrefix(raw, []byte("HTTP/1.1 "+infoHelloReq.status+" ")) {
		return fmt.Errorf("answered %q, want %s", truncateBytes(raw), infoHelloReq.status)
	}
	return nil
}

// infoTailHello asks each side of p infoHelloReq once: the problems, each naming its side, of a side that did not
// answer it (infoHelloAnswered).
func infoTailHello(p *Pair) []string {
	var problems []string
	for _, side := range p.Each() {
		b, err := v2Exchange(side.Daemon.Addr, infoHelloReq)
		if err == nil {
			err = infoHelloAnswered(b)
		}
		if err != nil {
			problems = append(problems, fmt.Sprintf("hello: %s: %v", side.Role, err))
		}
	}
	return problems
}

// TestInfoV1Tail (check `api.v1-info-tail`, milestone 10 commit 14, D224 F7 A): `/api/v1/info` whole on a standalone
// pair (no pulse charts), asked once both agents have been ready for infoTailAge and the oracle is younger than its
// first mutable gather, after a hello to each side (the dashboard hit the opt-out keeps uncounted).
func TestInfoV1Tail(t *testing.T) {
	p := StartPair(t, daemon.Options{PulseOff: true}, parentIdentity)
	if problems := infoTailHello(p); problems != nil {
		t.Fatal(strings.Join(problems, "\n"))
	}
	time.Sleep(infoTailAge)
	compareV2(t, p, infoTailReq, infoTailFamily)
	if age := time.Since(p.Oracle.LaunchStartedAt); age > 115*time.Second {
		t.Errorf("harness: asked %s after the oracle's launch, past the 12-115 s phase", age)
	}
}
