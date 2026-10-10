// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strconv"
	"strings"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// dashNormInfoTailExchange is one recorded step of TestInfoV1Tail's pair: each side's raw answer (0 the oracle, 1 the
// candidate) and the seconds it was in flight.
type dashNormInfoTailExchange struct {
	raw    [2]string
	flight [2][2]int64
}

// dashNormInfoTailRust is the list the Rust agent prints at commit 14 on TestInfoV1Tail's pair: BuildInfo::analytics()
// at 553b778338 (daemon/src/buildinfo/tests.rs, the_analytics_join: these eight names, and `|StreamParent` for an
// agent whose stream.conf enables an API key, as the harness's does).
const dashNormInfoTailRust = "Stream Compression|allocator|dbengine|Native HTTPS|TLS Host Verification|zlib|JSON-C|" +
	"libcrypto|StreamParent"

// dashNormInfoTailValueOnly are the named slots C gives a value and never a status (buildinfo.c:1326, :1546, :1556,
// :1560): C never prints their names. A slot's JSON is its value where it has one, else its status (:1610-1613);
// every other named slot gets a value only with its status (:1228-1238, :1249-1253, :1259-1264, :1298-1302), so a
// string there says that it holds.
var dashNormInfoTailValueOnly = []string{"libs.stacktraces", "runtime.profile", "runtime.mem-total",
	"runtime.mem-available"}

// dashNormInfoTailRuntime are the two slots set from the agent's own stream.conf (buildinfo.c:1548-1549): the pair's in
// /api/v1/info, the installed one's in a -W buildinfojson.
var dashNormInfoTailRuntime = []string{"runtime.parent", "runtime.child"}

// dashNormInfoTailSet tells whether a named slot of a build info (its JSON values, buildinfoSlotsOf) has its status
// set: `true`, or a value but for dashNormInfoTailValueOnly's.
func dashNormInfoTailSet(values map[string]string, slot string) bool {
	v := values[slot]
	return v == "true" || strings.HasPrefix(v, `"`) && !slices.Contains(dashNormInfoTailValueOnly, slot)
}

// dashNormInfoTailWith is raw with its body replaced by body (its Content-Length made the new body's: fnsLength).
func dashNormInfoTailWith(raw, body string) string {
	head, _, _ := strings.Cut(raw, "\r\n\r\n")
	return fnsLength(head, body)
}

// dashNormInfoTailBody is a raw answer's body (httpBody).
func dashNormInfoTailBody(raw string) string { return string(httpBody([]byte(raw))) }

// dashNormInfoTailRound judges a recorded step as v2Round judges a live one (v2Judge), the network aside.
func dashNormInfoTailRound(x dashNormInfoTailExchange, req v2Req, fam v2Family) v2Answer {
	var a v2Answer
	for i := range a.raw {
		a.raw[i] = []byte(x.raw[i])
	}
	a.flight = x.flight
	v2Judge(&a, req, fam, [2]string{string(Oracle), string(Candidate)}, [2]string{})
	return a
}

// dashNormInfoTailParse parses a JSON text of the recorded data.
func dashNormInfoTailParse(t *testing.T, s string) Value {
	t.Helper()
	v, err := ParseJSON([]byte(s))
	if err != nil {
		t.Fatalf("%v: %s", err, s)
	}
	return v
}

// dashNormInfoTailProblems holds problems to want: as many, each holding its want's text.
func dashNormInfoTailProblems(problems, want []string) bool {
	if len(problems) != len(want) {
		return false
	}
	for i := range want {
		if !strings.Contains(problems[i], want[i]) {
			return false
		}
	}
	return true
}

// testDashNormInfoTail pins TestInfoV1Tail's family, guard and hello (check `api.v1-info-tail`, D251 F2 A) on the
// answers two C agents gave on its pair, and the table of C's analytics names on C's and the Rust build k117's -W
// buildinfojson, all recorded by SA-B's probe p2 (dash_norm_info_tail_data_test.go).
func testDashNormInfoTail(t *testing.T) {
	rec := dashNormInfoTailRecorded
	info := rec["info"]
	oBody := dashNormInfoTailBody(info.raw[0])
	cList, err := dashNormInfoTailList(oBody)
	if err != nil {
		t.Fatal(err)
	}

	t.Run("pair", func(t *testing.T) {
		// what is asked: what the probe asked
		if got := string(v2Request(infoTailReq)); got != "GET /api/v1/info HTTP/1.1\r\nHost: localhost\r\n"+
			"Connection: close\r\n\r\n" || infoTailReq.from != "" || infoTailReq.status != "200" {
			t.Errorf("the request is %q (from %q, status %q)", got, infoTailReq.from, infoTailReq.status)
		}
		// the recorded pair, judged as v2Round judges a live one: no difference, and the check finds none (C against C)
		for _, step := range []string{"info", "info2"} {
			a := dashNormInfoTailRound(rec[step], infoTailReq, infoTailFamily)
			if a.problems != nil {
				t.Errorf("%s: the recorded pair: %q", step, a.problems)
				continue
			}
			if p := infoBuildinfoJudge(a.doc[0], a.doc[1], true); p != nil {
				t.Errorf("%s: the recorded pair's buildinfo: %q", step, p)
			}
		}
		// the same pair judged as if the candidate were another binary: each listed name C prints is reported (the
		// allowance is a tripwire, as buildinfoDiffs' is in cli.buildinfo)
		a := dashNormInfoTailRound(info, infoTailReq, infoTailFamily)
		var want []string
		for _, name := range strings.Split(cList, "|") {
			for _, n := range infoBuildinfoNames {
				if _, listed := buildinfoDiffs[n.slot]; n.name == name && listed {
					want = append(want, name+" ("+n.slot+"): the candidate now prints it as C does: remove it from "+
						"buildinfoDiffs")
				}
			}
		}
		want = append(want, "buildinfo: the candidate prints "+strconv.Quote(cList)+", want "+
			strconv.Quote(dashNormInfoTailRust))
		if p := infoBuildinfoJudge(a.doc[0], a.doc[1], false); len(want) != 21 || !dashNormInfoTailProblems(p, want) {
			t.Errorf("the recorded pair as another binary's: %q, want %d problems %q", p, len(want), want)
		}
		// asked at once after readiness, before the 10th second, the oracle's answer fails the guard at its
		// exporting connectors, still null (the one difference, analytics.c:1350)
		a = dashNormInfoTailRound(rec["info0"], infoTailReq, infoTailFamily)
		if !slices.Equal(a.problems, []string{`oracle: exporting-connectors is null, want "": ` +
			truncateBytes([]byte(dashNormInfoTailBody(rec["info0"].raw[0])))}) {
			t.Errorf("the answer before the 10th second: %q", a.problems)
		}
		if a := dashNormInfoTailRound(rec["info0"], v2Req{name: "info", target: "/api/v1/info", status: "200"},
			infoTailFamily); a.problems != nil {
			t.Errorf("the pair before the 10th second, no guard: %q", a.problems)
		}
	})

	t.Run("hello", func(t *testing.T) {
		for _, step := range []string{"hello", "hello2"} {
			for i, raw := range rec[step].raw {
				if err := infoHelloAnswered([]byte(raw)); err != nil {
					t.Errorf("%s %d: %v", step, i, err)
				}
			}
		}
		ok := rec["hello"].raw[0]
		_, rest, _ := strings.Cut(ok, "\r\n")
		for name, raw := range map[string]string{
			"refused (the dashboard ACL)": "HTTP/1.1 451 Unavailable For Legal Reasons\r\n" + rest,
			"another code starting 200":   "HTTP/1.1 2000 OK\r\n" + rest,
			"HTTP/1.0":                    "HTTP/1.0 200 OK\r\n" + rest,
			"nothing":                     "",
		} {
			if err := infoHelloAnswered([]byte(raw)); err == nil {
				t.Errorf("hello, %s: accepted", name)
			}
		}
		// what is asked, and of whom: each side once, by its own address
		if got := string(v2Request(infoHelloReq)); got != "GET /api/v1/registry?action=hello HTTP/1.1\r\n"+
			"Host: localhost\r\nConnection: close\r\n\r\n" || infoHelloReq.from != "" || infoHelloReq.status != "200" {
			t.Errorf("the hello is %q (from %q, status %q)", got, infoHelloReq.from, infoHelloReq.status)
		}
		answer := func(raw string) func() string { return func() string { return raw } }
		refused := "HTTP/1.1 451 Unavailable For Legal Reasons\r\n" + rest
		gone := dashNormStub(t, answer(ok))
		gone.Addr = "127.0.0.1:1"
		for name, c := range map[string]struct {
			o, c *daemon.Daemon
			want []string
		}{
			"C's answers": {dashNormStub(t, answer(ok)), dashNormStub(t, answer(rec["hello"].raw[1])), nil},
			"the candidate refuses": {dashNormStub(t, answer(ok)), dashNormStub(t, answer(refused)),
				[]string{"hello: candidate: answered \"HTTP/1.1 451"}},
			"the oracle refuses": {dashNormStub(t, answer(refused)), dashNormStub(t, answer(ok)),
				[]string{"hello: oracle: answered \"HTTP/1.1 451"}},
			"both refuse": {dashNormStub(t, answer(refused)), dashNormStub(t, answer(refused)),
				[]string{"hello: oracle: answered", "hello: candidate: answered"}},
			"the candidate closes": {dashNormStub(t, answer(ok)), dashNormStub(t, answer("")),
				[]string{`hello: candidate: answered ""`}},
			"no candidate": {dashNormStub(t, answer(ok)), gone, []string{"hello: candidate: dial tcp"}},
		} {
			if p := infoTailHello(&Pair{Oracle: c.o, Candidate: c.c}); !dashNormInfoTailProblems(p, c.want) {
				t.Errorf("infoTailHello, %s: %q, want %q", name, p, c.want)
			}
		}
	})

	t.Run("guard", func(t *testing.T) {
		if err := infoTailGuard(dashNormInfoTailParse(t, oBody)); err != nil {
			t.Fatalf("the guard refuses C's answer: %v", err)
		}
		if err := infoTailGuard(dashNormInfoTailParse(t, dashNormInfoTailBody(rec["info2"].raw[0]))); err != nil {
			t.Errorf("the guard refuses C's answer after a second hello: %v", err)
		}
		if len(infoTailFixed) != 16 {
			t.Errorf("infoTailFixed has %d facts, want 16", len(infoTailFixed))
		}
		// each member changed alone, then removed: the guard refuses it at that member, and of infoTailFixed's facts
		// exactly the member's own (none for the phase's members)
		fixed := []string{"cloud-enabled", "cloud-available", "agent-claimed", "aclk-available", "web-enabled",
			"stream-enabled", "stream-compression", "https-enabled", "release-channel", "exporting-enabled",
			"exporting-connectors", "allmetrics-prometheus-used", "allmetrics-shell-used", "allmetrics-json-used",
			"dashboard-used", "ml-info"}
		wrong := []struct{ member, value string }{
			{"cloud-enabled", "false"}, {"cloud-available", "false"}, {"agent-claimed", "true"},
			{"aclk-available", "true"}, {"web-enabled", "false"}, {"stream-enabled", "true"},
			{"stream-compression", "true"}, {"https-enabled", "false"}, {"release-channel", `"stable"`},
			{"exporting-enabled", "true"}, {"exporting-connectors", "null"}, {"exporting-connectors", `"JSON"`},
			{"allmetrics-prometheus-used", "1"}, {"allmetrics-shell-used", "1"}, {"allmetrics-json-used", "1"},
			{"dashboard-used", "1"}, {"ml-info", `{"enabled":true}`}, {"ml-info", `{"version":1,"enabled":false}`},
			{"collectors", `[{"plugin":"go.d","module":"x"}]`}, {"notification-methods", `"email"`},
			{"notification-methods", `""`}, {"charts-count", "1"}, {"metrics-count", "1"},
		}
		refused := make([]int, len(infoTailFixed))
		for _, w := range wrong {
			for _, value := range []string{w.value, ""} {
				v := dashNormInfoTailParse(t, oBody)
				name, wantErr := w.member+" = "+value, w.member+" is "+value+", want "
				if value == "" {
					name, wantErr = w.member+" removed", "no "+w.member+", want "
					v.Members = slices.DeleteFunc(v.Members, func(m Member) bool { return m.Key == w.member })
				} else if err := dashNormWeightsSet(&v, value, w.member); err != nil {
					t.Fatal(err)
				}
				if err := infoTailGuard(v); err == nil || !strings.HasPrefix(err.Error(), wantErr) {
					t.Errorf("guard, %s: %v, want %q", name, err, wantErr)
				}
				n := 0
				for i, f := range infoTailFixed {
					if f(v) != nil {
						n++
						refused[i]++
					}
				}
				if want := slices.Index(fixed, w.member); (want >= 0) != (n == 1) || want < 0 && n != 0 ||
					want >= 0 && infoTailFixed[want](v) == nil {
					t.Errorf("guard, %s: %d of infoTailFixed's facts refuse it, want only %s's", name, n, w.member)
				}
			}
		}
		for i, n := range refused {
			if n == 0 {
				t.Errorf("infoTailFixed's fact %d (%s) refuses no named wrong answer", i, fixed[i])
			}
		}
	})

	t.Run("table", func(t *testing.T) {
		keys, cValues := buildinfoSlotsOf(dashNormInfoTailParse(t, dashNormInfoTailBuildinfoJSON[0]))
		kKeys, kValues := buildinfoSlotsOf(dashNormInfoTailParse(t, dashNormInfoTailBuildinfoJSON[1]))
		if len(keys) != 119 || !slices.Equal(keys, kKeys) {
			t.Fatalf("the recorded build infos have %d and %d slots, want the same 119", len(keys), len(kKeys))
		}
		// 40 names, each once, each of a slot of C's, once, in C's slot order
		if len(infoBuildinfoNames) != 40 {
			t.Errorf("infoBuildinfoNames has %d names, want C's 40", len(infoBuildinfoNames))
		}
		names, at := map[string]bool{}, -1
		for _, n := range infoBuildinfoNames {
			i := slices.Index(keys, n.slot)
			if names[n.name] || i <= at {
				t.Errorf("infoBuildinfoNames: %s (%s) is named twice, is no slot of C's, or is out of C's order", n.name,
					n.slot)
			}
			names[n.name], at = true, i
			for side, values := range []map[string]string{cValues, kValues} {
				if v := values[n.slot]; v != "true" && v != "false" && !strings.HasPrefix(v, `"`) {
					t.Errorf("%s (%s) on side %d is %s, want a status or a value", n.name, n.slot, side, v)
				}
			}
		}
		// what C printed on the pair is what its build info holds, but the slots of the pair's own stream.conf; and
		// the same of k117's build info, with the pair's parent, is the Rust list
		derive := func(values map[string]string, runtime string) string {
			var out []string
			for _, n := range infoBuildinfoNames {
				if slices.Contains(dashNormInfoTailRuntime, n.slot) {
					if strings.Contains("|"+runtime+"|", "|"+n.name+"|") {
						out = append(out, n.name)
					}
				} else if dashNormInfoTailSet(values, n.slot) {
					out = append(out, n.name)
				}
			}
			return strings.Join(out, "|")
		}
		if got := derive(cValues, cList); got != cList || !strings.HasSuffix(cList, "|StreamParent") {
			t.Errorf("C's build info holds %q, C printed %q", got, cList)
		}
		if got := derive(kValues, cList); got != dashNormInfoTailRust {
			t.Errorf("k117's build info holds %q, want the Rust list %q", got, dashNormInfoTailRust)
		}
		// the allowance is buildinfoDiffs' exactly: a named slot where the two builds differ is listed, and a listed
		// one C holds k117 does not
		for _, n := range infoBuildinfoNames {
			c, k := dashNormInfoTailSet(cValues, n.slot), dashNormInfoTailSet(kValues, n.slot)
			_, listed := buildinfoDiffs[n.slot]
			// C's stacktraces slot holds a value and no status (buildinfo.c:1326): listed for its value alone
			if slices.Contains(dashNormInfoTailRuntime, n.slot) || listed && c && !k || !listed && c == k ||
				n.slot == "libs.stacktraces" && listed && !c && !k {
				continue
			}
			t.Errorf("%s (%s): C %v, k117 %v, listed in buildinfoDiffs %v", n.name, n.slot, c, k, listed)
		}
	})

	t.Run("judge", func(t *testing.T) {
		rust := dashNormInfoTailRust
		swap := strings.Replace(cList, "dbengine|Native HTTPS", "Native HTTPS|dbengine", 1)
		for name, c := range map[string]struct {
			o, c string
			same bool
			want []string
		}{
			// the Rust agent's list against C's
			"(a) the Rust list": {cList, rust, false, nil},
			"(b) the Rust list without zlib": {cList, strings.Replace(rust, "|zlib", "", 1), false,
				[]string{`buildinfo: the candidate prints "Stream Compression|allocator|dbengine|Native HTTPS|TLS ` +
					`Host Verification|JSON-C|libcrypto|StreamParent", want "` + rust + `"`}},
			"(c) the Rust list with Netdata Cloud": {cList, "Netdata Cloud|" + rust, false, []string{
				"buildinfo: Netdata Cloud (features.cloud): the candidate now prints it as C does: remove it from " +
					"buildinfoDiffs (M11 Cloud)",
				`buildinfo: the candidate prints "Netdata Cloud|` + rust + `", want "` + rust + `"`}},
			"the Rust list without StreamParent": {cList, strings.TrimSuffix(rust, "|StreamParent"), false,
				[]string{"the candidate prints"}},
			"the Rust list with StreamChild": {cList, rust + "|StreamChild", false, []string{"the candidate prints"}},
			"the Rust list in another order": {cList, strings.Replace(rust, "zlib|JSON-C", "JSON-C|zlib", 1), false,
				[]string{"the candidate prints"}},
			"the Rust list with stacktraces, which C never prints": {cList,
				strings.Replace(rust, "libcrypto", "libcrypto|stacktraces", 1), false, []string{"the candidate prints"}},
			"the Rust list twice zlib": {cList, strings.Replace(rust, "|zlib", "|zlib|zlib", 1), false,
				[]string{"the candidate prints"}},
			"nothing on the candidate": {cList, "", false, []string{`the candidate prints "", want "` + rust + `"`}},
			"an unknown name on the candidate": {cList, rust + "|Kubernetes", false,
				[]string{`buildinfo: the candidate prints "Kubernetes", no analytics name`}},
			"an unknown name on the oracle": {cList + "|Kubernetes", rust, false,
				[]string{`buildinfo: the oracle prints "Kubernetes", no analytics name`}},
			"an empty name on the candidate": {cList, "|" + rust, false,
				[]string{`buildinfo: the candidate prints "", no analytics name`}},
			"a name cut on the candidate": {cList, strings.Replace(rust, "zlib", "zli", 1), false,
				[]string{`buildinfo: the candidate prints "zli", no analytics name`}},
			// C against C: nothing is allowed
			"C against C": {cList, cList, true, nil},
			"C against C, one name dropped": {cList, strings.Replace(cList, "|zlib", "", 1), true,
				[]string{"the candidate prints"}},
			"C against C, the order swapped": {cList, swap, true, []string{"the candidate prints"}},
			"C against C, an unknown name": {cList, cList + "|Kubernetes", true,
				[]string{`the candidate prints "Kubernetes", no analytics name`}},
			"C against C, StreamChild on the candidate only": {cList, cList + "|StreamChild", true,
				[]string{"the candidate prints"}},
			"C against C, StreamChild on the oracle only": {cList + "|StreamChild", cList, true,
				[]string{"the candidate prints"}},
			"C against C, StreamParent on the oracle only": {cList, strings.TrimSuffix(cList, "|StreamParent"), true,
				[]string{"the candidate prints"}},
			"C against the Rust list": {cList, rust, true, []string{"the candidate prints"}},
			"nothing on both sides":   {"", "", true, nil},
		} {
			if p := infoBuildinfoProblems(c.o, c.c, c.same); !dashNormInfoTailProblems(p, c.want) {
				t.Errorf("infoBuildinfoProblems, %s: %q, want %q", name, p, c.want)
			}
		}
		// the parsed answers' member: a string on both sides
		o := dashNormInfoTailParse(t, oBody)
		for name, c := range map[string]struct {
			o, c string
			want string
		}{
			"the candidate's null":   {`{"buildinfo":"zlib"}`, `{"buildinfo":null}`, "the candidate's is null"},
			"the candidate's number": {`{"buildinfo":"zlib"}`, `{"buildinfo":1}`, "the candidate's is 1"},
			"no candidate's":         {`{"buildinfo":"zlib"}`, `{"build":"zlib"}`, "the candidate's is null"},
			"the oracle's null":      {`{"buildinfo":null}`, `{"buildinfo":"zlib"}`, "the oracle's is null"},
			"no oracle's":            {`{}`, `{"buildinfo":"zlib"}`, "the oracle's is null"},
		} {
			p := infoBuildinfoJudge(dashNormInfoTailParse(t, c.o), dashNormInfoTailParse(t, c.c), true)
			if len(p) != 1 || !strings.Contains(p[0], c.want) || !strings.Contains(p[0], "want a string") {
				t.Errorf("infoBuildinfoJudge, %s: %q, want %q", name, p, c.want)
			}
		}
		if p := infoBuildinfoJudge(o, o, true); p != nil {
			t.Errorf("infoBuildinfoJudge on C's answer: %q", p)
		}
	})

	t.Run("family", func(t *testing.T) {
		fam := infoTailFamily
		if len(fam.masks) != 1 || fam.masks[0].Pattern != "buildinfo" || fam.check == nil ||
			reflect.ValueOf(fam.check).Pointer() != reflect.ValueOf(infoBuildinfoCheck).Pointer() {
			t.Fatalf("infoTailFamily: masks %v, check %v: want buildinfo's mask alone and infoBuildinfoCheck", fam.masks,
				fam.check != nil)
		}
		quoted := `"buildinfo":` + strconv.Quote(cList)
		if strings.Count(oBody, quoted) != 1 {
			t.Fatalf("C's answer holds no %s", quoted)
		}
		candidate := func(member string) dashNormInfoTailExchange {
			x := info
			x.raw[1] = dashNormInfoTailWith(info.raw[1], strings.Replace(dashNormInfoTailBody(info.raw[1]), quoted,
				member, 1))
			return x
		}
		unmasked := fam
		unmasked.masks = nil
		for name, c := range map[string]struct {
			member         string
			masked, judged []string // the problems of the round with the family, and of the check after it
			plain          []string // the round's problems without the mask
		}{
			// the Rust list: the mask lets it through to the check, which takes it; without the mask it differs
			"the Rust list": {`"buildinfo":` + strconv.Quote(dashNormInfoTailRust), nil, nil,
				[]string{"$.buildinfo: oracle " + strconv.Quote(cList)}},
			// a value that is no list: the round takes it, the check does not
			"null": {`"buildinfo":null`, nil, []string{"the candidate's is null"}, []string{"$.buildinfo: oracle"}},
			// the member missing: the mask hides no absence
			"no member": {`"build-info":` + strconv.Quote(cList), []string{"$.<members>"}, nil,
				[]string{"$.<members>"}},
			// the known limit: the member's escapes are not compared (C's names have none); without the mask they are
			"an escape": {`"buildinfo":"` + strings.Replace(dashNormInfoTailRust, "|", `\u007c`, 1) + `"`, nil, nil,
				[]string{"$.buildinfo: oracle", "the strings' escapes differ"}},
		} {
			x := candidate(c.member)
			a := dashNormInfoTailRound(x, infoTailReq, fam)
			if !dashNormInfoTailProblems(a.problems, c.masked) {
				t.Errorf("infoTailFamily, %s: %q, want %q", name, a.problems, c.masked)
			}
			if a.problems == nil {
				if p := infoBuildinfoJudge(a.doc[0], a.doc[1], false); !dashNormInfoTailProblems(p, c.judged) {
					t.Errorf("infoTailFamily's check, %s: %q, want %q", name, p, c.judged)
				}
			}
			if p := dashNormInfoTailRound(x, infoTailReq, unmasked).problems; !dashNormInfoTailProblems(p, c.plain) {
				t.Errorf("infoTailFamily without its mask, %s: %q, want %q", name, p, c.plain)
			}
		}
		// the check reads sameBinary: two binaries take the Rust list, one binary takes C's own list
		dir := t.TempDir()
		for _, f := range []string{"a", "b"} {
			if err := os.WriteFile(filepath.Join(dir, f), nil, 0o644); err != nil {
				t.Fatal(err)
			}
		}
		rustDoc := dashNormInfoTailRound(candidate(`"buildinfo":`+strconv.Quote(dashNormInfoTailRust)), infoTailReq,
			fam).doc
		t.Setenv("PARITY_ORACLE", filepath.Join(dir, "a"))
		t.Setenv("PARITY_CANDIDATE", filepath.Join(dir, "b"))
		fam.check(t, "two binaries, the Rust list", rustDoc[0], rustDoc[1])
		t.Setenv("PARITY_CANDIDATE", filepath.Join(dir, "a"))
		cDoc := dashNormInfoTailRound(info, infoTailReq, fam).doc
		fam.check(t, "one binary, C's list", cDoc[0], cDoc[1])
	})
}

// dashNormInfoTailList is the `buildinfo` string of a recorded answer's body.
func dashNormInfoTailList(body string) (string, error) {
	v, err := ParseJSON([]byte(body))
	if err != nil {
		return "", err
	}
	b, err := dashMember(v, "buildinfo")
	return b.Text, err
}
