// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"
)

// testDashNormNIStream pins the machinery of `api.v2-node-instances-stream` (nodes_stream_test.go): its parsers of
// C's texts, the rewrite that keeps a body's layout, the cut of the `stream` objects out of niIngestRender's reach,
// and the family on C's recorded answers of each stage, with named wrong candidates.
func testDashNormNIStream(t *testing.T) {
	// C's duration texts (duration_snprintf(..., "us", true)): off, then the parts from the largest unit down
	for text, want := range map[string]int64{
		"off": 0, "1us": 1, "456ms 789us": 456789, "2m 3s 456ms 789us": 123456789, "1h 1s": 3601e6, "2d 3h": 183600e6,
		"9s 990ms": 9990000,
	} {
		if got, ok := niDuration(text); !ok || got != want {
			t.Errorf("niDuration(%q) = %d, %v, want %d", text, got, ok, want)
		}
	}
	for _, text := range []string{"", "0s", "1s 2m", "1s 1s", "1s  2ms", " 1s", "1x", "1.5s", "-1s", "01s", "1mo", "off 1s"} {
		if got, ok := niDuration(text); ok {
			t.Errorf("niDuration(%q) = %d, took a text C does not write", text, got)
		}
	}
	// a parent's local time: two fraction digits or none, Z or an offset
	for text, want := range map[string]int64{
		"2026-10-09T14:40:01.23Z":      1791556801230000,
		"2026-10-09T20:10:01.23+05:30": 1791556801230000,
		"2026-10-09T14:40:01Z":         1791556801000000,
		"2026-10-09T10:40:01.05-04:00": 1791556801050000,
	} {
		if got, ok := niLocalTime(text); !ok || got != want {
			t.Errorf("niLocalTime(%q) = %d, %v, want %d", text, got, ok, want)
		}
	}
	for _, text := range []string{"2026-10-09T14:40:01.234Z", "2026-10-09T14:40:01.2Z", "2026-10-09 14:40:01Z",
		"2026-10-09T14:40:01", "2026-10-09T14:40:01+0530", "1791643201"} {
		if got, ok := niLocalTime(text); ok {
			t.Errorf("niLocalTime(%q) = %d, took a text C does not write", text, got)
		}
	}
	for text, want := range map[string]string{
		"2026-10-09T20:10:01.23+05:30": "9999-99-99T99:99:99.99+05:30",
		"2026-10-09T14:40:01Z":         "9999-99-99T99:99:99Z",
		"2026-10-09T14:40:01.234Z":     "2026-10-09T14:40:01.234Z",
	} {
		if got := niTimeShape(text); got != want {
			t.Errorf("niTimeShape(%q) = %q, want %q", text, got, want)
		}
	}
	// jsonRewrite keeps every byte between the values it replaces, in a minified and in a pretty body
	for _, body := range []string{`{"a":1,"b":{"c":[2,{"d":"x"}]}}`, "{\n    \"a\":1,\n    \"b\":{\n        \"c\":[\n" +
		"            2,\n            {\n                \"d\":\"x\"\n            }\n        ]\n    }\n}\n"} {
		got := string(jsonRewrite([]byte(body), nil, func(path []string, value []byte) ([]byte, bool) {
			switch strings.Join(path, ".") {
			case "a":
				return []byte(`"A"`), true
			case "b.c.[1].d":
				return []byte(`"D"`), true
			}
			return nil, false
		}))
		want := strings.Replace(strings.Replace(body, `1`, `"A"`, 1), `"x"`, `"D"`, 1)
		if got != want {
			t.Errorf("jsonRewrite: %q, want %q", got, want)
		}
	}
	// the cut: a render outside the `stream` objects never reaches into them, which their own render rewrites
	outside := niOutsideStream(func(_ int, _ [2]int64, b []byte) []byte {
		return []byte(strings.ReplaceAll(string(b), "1", "R"))
	}, func(_ int, _ [2]int64, whole []byte, path []string, obj []byte) []byte {
		if !strings.Contains(string(whole), `"now":1`) || !niIsStream(path) {
			return []byte(`"no body"`)
		}
		return []byte(strings.ReplaceAll(string(obj), "1", "S"))
	})
	body := `{"nodes":[{"instances":[{"ingest":{"id":1},"stream":{"id":1,"x":[1]}}]},` +
		`{"instances":[{"stream":{"id":1}}],"stream":{"id":1}}],"agents":[{"now":1}]}`
	want := `{"nodes":[{"instances":[{"ingest":{"id":R},"stream":{"id":S,"x":[S]}}]},` +
		`{"instances":[{"stream":{"id":S}}],"stream":{"id":R}}],"agents":[{"now":R}]}`
	if got := string(outside(0, [2]int64{}, []byte(body))); got != want {
		t.Errorf("niOutsideStream: %s\nwant %s", got, want)
	}

	// the windows' words: a start up to the agent's readiness only for a time taken while it booted (a parent's), the
	// first window that holds the second, none for a window of zeros
	side := niStreamSide{niSide: niSide{started: [2]int64{100, 105}}, ready: 108, connected: [2]int64{120, 125},
		closed: [2]int64{130, 135}, probed: [2]int64{140, 145}}
	for _, c := range []struct {
		sec  int64
		boot bool
		want string
	}{{99, true, ""}, {100, false, "START"}, {105, false, "START"}, {107, false, ""}, {107, true, "START"},
		{108, true, "START"}, {109, true, ""}, {119, false, ""}, {120, false, "CONNECTED"}, {125, true, "CONNECTED"},
		{126, false, ""}, {130, false, "CLOSED"}, {135, false, "CLOSED"}, {140, false, "PROBED"}, {145, false, "PROBED"},
		{146, false, ""}, {0, false, ""}} {
		if got := side.niStreamWord(c.sec, c.boot); got != c.want {
			t.Errorf("niStreamWord(%d, %v) = %q, want %q", c.sec, c.boot, got, c.want)
		}
	}
	if got := (niStreamSide{}).niStreamWord(0, true); got != "" {
		t.Errorf("niStreamWord of no window = %q", got)
	}
	// the first collection's window: from the launch to niStreamFirstSlack before the second the side was seen
	// connected, else banned; none without either; and its render of a number in it, not of one outside or of a date
	for name, c := range map[string]struct {
		side niStreamSide
		want [2]int64
	}{
		"out":   {niStreamSide{niSide: niSide{started: [2]int64{100, 105}}, connected: [2]int64{118, 125}}, [2]int64{100, 120}},
		"ban":   {niStreamSide{niSide: niSide{started: [2]int64{100, 105}}, probed: [2]int64{140, 145}}, [2]int64{100, 140}},
		"never": {niStreamSide{niSide: niSide{started: [2]int64{100, 105}}}, [2]int64{}},
	} {
		if got := c.side.collected(); got != c.want {
			t.Errorf("collected, %s: %v, want %v", name, got, c.want)
		}
	}
	out := niStreamSide{niSide: niSide{started: [2]int64{100, 105}}, connected: [2]int64{118, 125}}
	for in, want := range map[string]string{
		`{"first_time":100,"x":1}`: `{"first_time":"FIRST","x":1}`,
		`{"first_time": 120}`:      `{"first_time": "FIRST"}`,
		`{"first_time":121}`:       `{"first_time":121}`,
		`{"first_time":99}`:        `{"first_time":99}`,
		`{"first_time":0}`:         `{"first_time":0}`,
		`{"first_time":"x"}`:       `{"first_time":"x"}`,
		`{"first_time_t":110}`:     `{"first_time_t":110}`,
	} {
		if got := string(niFirstRender(out, []byte(in))); got != want {
			t.Errorf("niFirstRender(%s) = %s, want %s", in, got, want)
		}
	}
	if got := string(niFirstRender(niStreamSide{}, []byte(`{"first_time":110}`))); got != `{"first_time":110}` {
		t.Errorf("niFirstRender without a window: %s", got)
	}
	// an end cut at its port, a TLS end too
	for end, want := range map[string][4]string{
		"[127.0.0.1]:40758":     {"[127.0.0.1]:", "40758", "", "ok"},
		"[127.0.0.1]:40758:SSL": {"[127.0.0.1]:", "40758", ":SSL", "ok"},
		"[::1]:443":             {"[::1]:", "443", "", "ok"},
		"[not connected]:0":     {"[not connected]:", "0", "", "ok"},
		"127.0.0.1:40758":       {"", "", "", ""},
	} {
		b, p, a, ok := niPortOf(end)
		if got := [4]string{b, p, a, map[bool]string{true: "ok"}[ok]}; got != want {
			t.Errorf("niPortOf(%q) = %q, want %q", end, got, want)
		}
	}
	// each recorded row: C's answer passes the guard, C's pair shows no difference; then the named wrong candidates
	slow := niStreamRecorded["never/never/v3-ni"].sides[1].started[1] + 3
	niStreamCheckRecorded(t, niStreamCases(t), map[string]int64{
		"never: the parents made two seconds after the start window, on a slow boot": slow,
		"never: the parents made after the agent was ready, on a slow boot":          slow,
	})
	niStreamGuards(t)
}

// niStreamPaths are the paths of localhost's instance in a node-instance answer (at), its `stream` (st), the
// destination (dst) and the scripted parent's item (par), and where names a path as Compare reports it.
func niStreamPaths() (at, st, dst, par func(keys ...string) []string, where func([]string) string) {
	at = func(keys ...string) []string { return append([]string{"nodes", "[0]", "instances", "[0]"}, keys...) }
	st = func(keys ...string) []string { return at(append([]string{"stream"}, keys...)...) }
	dst = func(keys ...string) []string { return st(append([]string{"destination"}, keys...)...) }
	par = func(keys ...string) []string { return dst(append([]string{"parents", "[0]"}, keys...)...) }
	where = func(path []string) string { return "$." + strings.ReplaceAll(strings.Join(path, "."), ".[", "[") }
	return
}

// niStreamRecord is one recorded row of TestNodeInstancesStream (niStreamRecorded).
type niStreamRecord struct {
	stage, tz, second string
	sides             [2]niStreamSide
	flight            [2][2]int64
	body              [2]string
}

// niStreamRecordedRow is the recorded row at key with its request and family as niStreamRows makes them for the
// recorded stage, its agents' TZ written UTC where it was the harness's own (the run's box was UTC: the pins hold
// whatever box they run on).
func niStreamRecordedRow(t *testing.T, key string) (niStreamRecord, niStreamRow) {
	t.Helper()
	rec, ok := niStreamRecorded[key]
	if !ok {
		t.Fatalf("harness: no recorded row %s", key)
	}
	return rec, niStreamRowOf(t, rec, key)
}

// niStreamRowOf is the row of rec at key (`<pair>/<stage>/<row>`) as niStreamRows makes it from rec's values.
func niStreamRowOf(t *testing.T, rec niStreamRecord, key string) niStreamRow {
	t.Helper()
	pair, _, _ := strings.Cut(key, "/")
	name := key[strings.LastIndex(key, "/")+1:]
	r := &niStreamRun{Stage: rec.stage, Sides: rec.sides, pair: pair, tz: rec.tz, second: rec.second}
	if r.tz == "" {
		r.tz = "UTC"
	}
	for _, row := range niStreamRows(r) {
		if row.req.name == name {
			return row
		}
	}
	t.Fatalf("harness: the stage %s asks no row %s", rec.stage, name)
	return niStreamRow{}
}

// niStreamNormDiffs are the paths where two answers of a row differ as its family compares them, each side
// normalised with its own flight (v2Round): the values after the masks, then `<layout>` and `<escapes>` when those
// differ.
func niStreamNormDiffs(t *testing.T, fam v2Family, target string, flight [2][2]int64, o, c string) string {
	t.Helper()
	var body [2][]byte
	var doc [2]Value
	for i, b := range []string{o, c} {
		body[i] = fam.normalise(i, fam.v2Clock(target, flight[i]), flight[i], []byte(b))
		v, err := ParseJSON(body[i])
		if err != nil {
			t.Fatalf("%v: %s", err, body[i])
		}
		doc[i] = v
	}
	var out []string
	for _, d := range Compare(ApplyMasks(doc[0], fam.masks), ApplyMasks(doc[1], fam.masks), fam.unordered...) {
		out = append(out, d.Path)
	}
	if v2Layouts(body, fam.layoutByCount()) != "" {
		out = append(out, "<layout>")
	}
	if v2Escapes(body, doc, fam) != "" {
		out = append(out, "<escapes>")
	}
	return strings.Join(out, " ")
}

// niStreamSwap is text with old replaced by new, where old occurs exactly once (a named wrong candidate must change
// what it names).
func niStreamSwap(t *testing.T, text, old, new string) string {
	t.Helper()
	if n := strings.Count(text, old); n != 1 {
		t.Fatalf("harness: %q occurs %d times in the recorded answer", old, n)
	}
	return strings.Replace(text, old, new, 1)
}

// niStreamCase is one candidate of a recorded row: the recorded oracle against it shows differences at want.
type niStreamCase struct {
	key, candidate, want string
}

// niStreamCheckRecorded pins each recorded row: the oracle's answer passes the row's guard, the C pair shows no
// difference, and each named wrong candidate is reported where cases say; a case named in ready is judged as if the
// candidate's side was ready in that second (a slow boot).
func niStreamCheckRecorded(t *testing.T, cases map[string]niStreamCase, ready map[string]int64) {
	t.Helper()
	for key := range niStreamRecorded {
		rec, row := niStreamRecordedRow(t, key)
		v, err := ParseJSON(row.fam.normalise(0, row.fam.v2Clock(row.req.target, rec.flight[0]), rec.flight[0],
			[]byte(rec.body[0])))
		if err != nil {
			t.Fatalf("%s: %v", key, err)
		}
		if err := row.req.guard(v); err != nil {
			t.Errorf("%s: the guard refuses C's answer: %v", key, err)
		}
		if got := niStreamNormDiffs(t, row.fam, row.req.target, rec.flight, rec.body[0], rec.body[1]); got != "" {
			t.Errorf("%s: C's pair differs at %q", key, got)
		}
	}
	for name, c := range cases {
		rec, row := niStreamRecordedRow(t, c.key)
		if second, ok := ready[name]; ok {
			rec.sides[1].ready = second
			row = niStreamRowOf(t, rec, c.key)
		}
		if got := niStreamNormDiffs(t, row.fam, row.req.target, rec.flight, rec.body[0], c.candidate); got != c.want {
			t.Errorf("%s, %s: differences at %q, want %q", c.key, name, got, c.want)
		}
	}
}

// niStreamEdit is body with the value at path (from the document's root) written as value: a named wrong candidate
// made from a recorded answer. With add, the value is kept and add is written after it (a member more, `,"key":v`).
// With drop, the member at path is cut with the separator before it (not the first member of its object).
func niStreamEdit(t *testing.T, body string, path []string, value, add string, drop bool) string {
	t.Helper()
	done := false
	out := jsonRewrite([]byte(body), nil, func(at []string, b []byte) ([]byte, bool) {
		if drop && slices.Equal(at, path[:len(path)-1]) {
			items, err := jsonItems(b)
			if err != nil {
				return nil, false
			}
			for k, it := range items {
				if it.key == path[len(path)-1] && k > 0 {
					done = true
					return slices.Concat(b[:it.from], b[it.end:]), true
				}
			}
			return nil, false
		}
		if drop || !slices.Equal(at, path) {
			return nil, false
		}
		done = true
		if add != "" {
			return append(slices.Clone(b), add...), true
		}
		return []byte(value), true
	})
	if !done {
		t.Fatalf("harness: no %s in the recorded answer", strings.Join(path, "."))
	}
	return string(out)
}

// niStreamValue is the text of the value at path in body.
func niStreamValue(t *testing.T, body string, path ...string) string {
	t.Helper()
	got := ""
	jsonRewrite([]byte(body), nil, func(at []string, b []byte) ([]byte, bool) {
		if slices.Equal(at, path) {
			got = string(b)
			return b, true
		}
		return nil, false
	})
	if got == "" {
		t.Fatalf("harness: no %s in the recorded answer", strings.Join(path, "."))
	}
	return got
}

// niStreamCases are the named wrong candidates of the recorded rows: each made from the candidate's recorded answer,
// each reported where its want says (T5's planted bugs of commit 11 by their id where one is the same mistake).
func niStreamCases(t *testing.T) map[string]niStreamCase {
	t.Helper()
	at, st, dst, par, where := niStreamPaths()
	rec := func(key string) niStreamRecord {
		r, ok := niStreamRecorded[key]
		if !ok {
			t.Fatalf("harness: no recorded row %s", key)
		}
		return r
	}
	const nv, dt, tz, cn, dn, bn = "never/never/v3-ni", "never/never/v3-ni-rfc3339", "never-tz/never/v3-ni",
		"out/connected/v3-ni", "out/denied/v3-ni", "ban/banned/v3-ni"
	never, dated, zoned, conn, denied := rec(nv).body[1], rec(dt).body[1], rec(tz).body[1], rec(cn).body[1], rec(dn).body[1]
	banned := rec(bn).body[1]
	second := func(keys ...string) []string { return dst(append([]string{"parents", "[1]"}, keys...)...) }
	members := func(p []string) string { return where(p) + ".<members>" }
	edit := func(body string, path []string, value string) string {
		return niStreamEdit(t, body, path, value, "", false)
	}
	add := func(body string, path []string, more string) string {
		return niStreamEdit(t, body, path, "", more, false)
	}
	drop := func(body string, path []string) string { return niStreamEdit(t, body, path, "", "", true) }
	// the candidate's own seconds and ports
	cside, dside := rec(cn).sides[1], rec(dn).sides[1]
	sec := func(n int64) string { return strconv.FormatInt(n, 10) }
	// the rfc3339 row's since as the number it stands for
	utcSince, err := time.Parse(time.RFC3339, strings.Trim(niStreamValue(t, dated, st("since")...), `"`))
	if err != nil {
		t.Fatal(err)
	}
	// the TZ row's parent's since, the same moment written in UTC
	zonedSince, err := time.Parse(time.RFC3339Nano, strings.Trim(niStreamValue(t, zoned, par("since")...), `"`))
	if err != nil {
		t.Fatal(err)
	}
	connSince := niStreamValue(t, conn, par("since")...)
	// a first collection written wrong the same way in the database and in the stream path (DB-FIRST holds)
	first := func(body, value string) string {
		return edit(edit(body, at("db", "first_time"), value), dst("streaming_path", "[0]", "first_time_t"), value)
	}
	bside := rec(bn).sides[1]
	// the rfc3339 row's localhost start and last time as the numbers they stand for
	datedSecond := func(path []string) string {
		d, err := time.Parse(time.RFC3339, strings.Trim(niStreamValue(t, dated, path...), `"`))
		if err != nil {
			t.Fatal(err)
		}
		return sec(d.Unix())
	}
	connLocal := niStreamValue(t, conn, dst("local")...)
	return map[string]niStreamCase{
		// a sender that never started
		"never: attempts without the one in progress (m59)": {nv, edit(never, par("attempts"), "0"),
			where(par("attempts"))},
		"never: the path written stream_path (m58)": {nv, niStreamSwap(t, never, `"streaming_path":`, `"stream_path":`),
			members(dst())},
		"never: no stream, as a host without a sender": {nv, drop(never, st()), members(at()) + " <layout>"},
		"never: hops 0": {nv, edit(never, st("hops"), "0"), where(st("hops"))},
		"never: a since of 0, no fallback to the start (m37)": {nv, edit(never, st("since"), "0"),
			where(st("since")) + " " + where(st("age"))},
		"never: since a second before the launch": {nv, edit(never, st("since"), sec(rec(nv).sides[1].started[0]-1)),
			where(st("since")) + " " + where(st("age"))},
		"never: the reason CONNECTED": {nv, edit(never, st("reason"), `"CONNECTED"`), where(st("reason"))},
		"never: the parent ranked": {nv, add(never, par("last_handshake"), `,"batch":1,"order":1,"random":false`),
			members(par()) + " <layout>"},
		"never: the parent postponed": {nv, add(never, par("last_handshake"), `,"next_check":"2026-10-09T15:00:00Z",`+
			`"next_in":"1s"`), members(par()) + " <layout>"},
		"never: the parent's age not to now": {nv, edit(never, par("age"), `"9s 1ms"`), where(par("age"))},
		// a slow boot (niStreamSlowBoot): the list of parents made after the start window, by the second the agent was
		// ready, or after it
		"never: the parents made two seconds after the start window, on a slow boot": {nv, edit(never, par("since"),
			`"`+time.Unix(rec(nv).sides[1].started[1]+2, 230000000).UTC().Format("2006-01-02T15:04:05.00Z07:00")+`"`),
			where(par("age"))},
		"never: the parents made after the agent was ready, on a slow boot": {nv, edit(never, par("since"),
			`"`+time.Unix(rec(nv).sides[1].started[1]+4, 230000000).UTC().Format("2006-01-02T15:04:05.00Z07:00")+`"`),
			where(par("since")) + " " + where(par("age"))},
		"never: the parent's age a second short": {nv, edit(never, par("age"),
			niStreamShift(t, niStreamValue(t, never, par("age")...), -1e6)), where(par("age"))},
		"never: the parent's age two seconds long": {nv, edit(never, par("age"),
			niStreamShift(t, niStreamValue(t, never, par("age")...), 2e6)), where(par("age"))},
		"never: last_time ten seconds behind": {nv, edit(never, at("db", "last_time"),
			niStreamPlus(t, niStreamValue(t, never, at("db", "last_time")...), -10)), where(at("db", "last_time"))},
		"never: the parent's reason CONNECTING": {nv, edit(never, par("last_handshake"), `"CONNECTING"`),
			where(par("last_handshake"))},
		"never: the parent skipped": {nv, edit(never, par("skipped"), "true"), where(par("skipped"))},
		"never: the path's since 0": {nv, edit(never, dst("streaming_path", "[0]", "since"), "0"),
			where(dst("streaming_path", "[0]", "since"))},
		// rfc3339
		"rfc3339: the stream's since a number (m56)": {dt, edit(dated, st("since"), sec(utcSince.Unix())),
			where(st("since"))},
		"rfc3339: localhost's start a number": {dt, edit(dated, at("ingest", "since"), datedSecond(at("ingest", "since"))),
			where(at("ingest", "since"))},
		"rfc3339: localhost's last time a number": {dt, edit(dated, at("db", "last_time"),
			datedSecond(at("db", "last_time"))), where(at("db", "last_time"))},
		"rfc3339: the cloud status' age off by one": {dt, edit(dated, []string{"agents", "[0]", "cloud", "age"},
			niStreamPlus(t, niStreamValue(t, dated, "agents", "[0]", "cloud", "age"), 1)), "$.agents[0].cloud.age"},
		"rfc3339: the stream's since a local time": {dt, edit(dated, st("since"),
			`"`+utcSince.In(time.FixedZone("", 3600)).Format(time.RFC3339)+`"`), where(st("since")) + " " + where(st("age"))},
		// TZ=Asia/Kolkata: the parents' times are local
		"TZ: the parent's since in UTC": {tz, edit(zoned, par("since"),
			`"`+zonedSince.UTC().Format("2006-01-02T15:04:05.00Z07:00")+`"`), where(par("since"))},
		"TZ: the parent's since with three fraction digits": {tz, edit(zoned, par("since"),
			`"`+zonedSince.Format("2006-01-02T15:04:05.000Z07:00")+`"`), where(par("since")) + " " + where(par("age"))},
		// connected
		"connected: an online stream says a reason (m55)": {cn, add(conn, st("age"), `,"reason":"CONNECTED"`),
			members(st()) + " <layout>"},
		"connected: attempts without the one in progress (m59)": {cn, edit(conn, par("attempts"), "1"),
			where(par("attempts"))},
		"connected: the place printed only when random (m60)": {cn, drop(drop(drop(conn, par("batch")), par("order")),
			par("random")), members(par()) + " <layout>"},
		"connected: the path written stream_path (m58)": {cn, niStreamSwap(t, conn, `"streaming_path":`, `"stream_path":`),
			members(dst())},
		"connected: the ends swapped": {cn, edit(edit(conn, dst("local"), niStreamValue(t, conn, dst("remote")...)),
			dst("remote"), connLocal), where(dst("local")) + " " + where(dst("remote"))},
		"connected: the local end the listening port": {cn, edit(conn, dst("local"), `"[127.0.0.1]:`+cside.listen+`"`),
			where(dst("local"))},
		"connected: since the agent's start": {cn, edit(conn, st("since"), sec(cside.started[0])),
			where(st("since")) + " " + where(st("age"))},
		"connected: id 2":        {cn, edit(conn, st("id"), "2"), where(st("id"))},
		"connected: hops 0":      {cn, edit(conn, st("hops"), "0"), where(st("hops"))},
		"connected: replicating": {cn, edit(conn, st("status"), `"replicating"`), where(st("status"))},
		"connected: completion 0": {cn, edit(conn, st("replication", "completion"), "0"),
			where(st("replication", "completion"))},
		"connected: another name negotiated": {cn, edit(conn, dst("capabilities"),
			strings.Replace(niStreamNegotiated, `"FLOATBASELINE"`, `"MLMODELS"`, 1)), where(dst("capabilities", "[15]"))},
		"connected: the offered names, not the negotiated": {cn, edit(conn, dst("capabilities"), niStreamOurs),
			niStreamItems(where(dst("capabilities")), 16) + " " + where(dst("capabilities")) + ".length <layout>"},
		"connected: compression on": {cn, edit(conn, dst("traffic", "compression"), "true"),
			where(dst("traffic", "compression"))},
		"connected: no data sent": {cn, edit(conn, dst("traffic", "data"), "0"), where(dst("traffic", "data"))},
		"connected: no replication answer sent": {cn, edit(conn, dst("traffic", "replication"), "0"),
			where(dst("traffic", "replication"))},
		"connected: a byte of metadata more": {cn, edit(conn, dst("traffic", "metadata"),
			niStreamPlus(t, niStreamValue(t, conn, dst("traffic", "metadata")...), 1)), where(dst("traffic", "metadata"))},
		"connected: function bytes": {cn, edit(conn, dst("traffic", "functions"), "1"), where(dst("traffic", "functions"))},
		"connected: the parent still postponed": {cn, add(conn, par("last_handshake"), `,"next_check":`+
			`"2026-10-09T15:00:00.00Z","next_in":"1s"`), members(par()) + " <layout>"},
		"connected: the parent's reason CONNECTED": {cn, edit(conn, par("last_handshake"), `"CONNECTED"`),
			where(par("last_handshake"))},
		"connected: the parent's since the agent's start": {cn, edit(conn, par("since"),
			`"`+time.Unix(cside.started[0], 230000000).UTC().Format("2006-01-02T15:04:05.00Z07:00")+`"`),
			where(par("since")) + " " + where(par("age"))},
		"connected: the parent's info":          {cn, edit(conn, par("info"), "true"), where(par("info"))},
		"connected: the parent drawn at random": {cn, edit(conn, par("random"), "true"), where(par("random"))},
		"connected: the path's first time not the database's": {cn, edit(conn, dst("streaming_path", "[0]", "first_time_t"),
			niStreamPlus(t, niStreamValue(t, conn, at("db", "first_time")...), 1)),
			where(dst("streaming_path", "[0]", "first_time_t"))},
		"connected: the first collection at 0": {cn, first(conn, "0"),
			where(at("db", "first_time")) + " " + where(dst("streaming_path", "[0]", "first_time_t"))},
		"connected: the first collection in the second it was seen connected": {cn,
			first(conn, sec(cside.connected[1])), where(at("db", "first_time"))},
		"connected: the first collection four seconds before it was seen connected": {cn,
			first(conn, sec(cside.connected[1]-4)), where(at("db", "first_time"))},
		"connected: the first collection a second before the launch": {cn, first(conn, sec(cside.started[0]-1)),
			where(at("db", "first_time"))},
		"banned: the first collection in the second it was seen banned": {bn, first(banned, sec(bside.probed[1])),
			where(at("db", "first_time"))},
		"connected: the agent not counted sending": {cn, edit(conn, []string{"agents", "[0]", "nodes"},
			`{"total":1,"receiving":0,"sending":0,"archived":0}`), "$.agents[0].nodes.sending <layout>"},
		// refused after the close
		"denied: a refused attempt counted (m45)": {dn, edit(denied, st("id"), "2"), where(st("id"))},
		"denied: the ends kept after the disconnect (m47)": {dn, edit(denied, dst("local"), connLocal),
			where(dst("local"))},
		"denied: since stamped at the disconnect (m49)": {dn, edit(denied, st("since"), sec(dside.closed[0])),
			where(st("since")) + " " + where(st("age"))},
		"denied: the disconnect's reason, not the refusal's (m38)": {dn, edit(denied, st("reason"),
			`"DISCONNECT SOCKET ERROR"`), where(st("reason"))},
		"denied: the negotiated names kept": {dn, edit(denied, dst("capabilities"), niStreamNegotiated),
			where(dst("capabilities")) + ".length <layout>"},
		"denied: hops the ingestion's": {dn, edit(denied, st("hops"), "0"), where(st("hops"))},
		"denied: the counters zeroed at the disconnect": {dn, edit(denied, dst("traffic", "data"), "0"),
			where(dst("traffic", "data"))},
		"denied: the refused attempt not counted": {dn, edit(denied, par("attempts"), "2"), where(par("attempts"))},
		"denied: the parent's since the connection's pass": {dn, edit(denied, par("since"), connSince),
			where(par("since")) + " " + where(par("age"))},
		"denied: the parent's place kept": {dn, add(denied, par("next_in"), `,"batch":1,"order":1,"random":false`),
			members(par()) + " <layout>"},
		"denied: the parent not skipped": {dn, edit(denied, par("skipped"), "false"), where(par("skipped"))},
		"denied: the parent not postponed": {dn, drop(drop(denied, par("next_check")), par("next_in")),
			members(par()) + " <layout>"},
		"denied: next_in not to next_check": {dn, edit(denied, par("next_in"), `"1s"`), where(par("next_in"))},
		"denied: next_in a second long": {dn, edit(denied, par("next_in"),
			niStreamShift(t, niStreamValue(t, denied, par("next_in")...), 1e6)), where(par("next_in"))},
		"denied: postponed for five minutes": {dn, edit(edit(denied, par("next_check"), `"`+time.UnixMicro(
			rec(dn).flight[1][1]*1e6+300e6+230000).UTC().Format("2006-01-02T15:04:05.00Z07:00")+`"`), par("next_in"),
			`"5m"`), where(par("next_check"))},
		"denied: next_check before the request": {dn, edit(denied, par("next_check"), `"`+time.UnixMicro(
			rec(dn).flight[1][1]*1e6-30e6+230000).UTC().Format("2006-01-02T15:04:05.00Z07:00")+`"`),
			where(par("next_check")) + " " + where(par("next_in"))},
		"denied: the parent's reason SOCKET CONNECTED": {dn, edit(denied, par("last_handshake"), `"SOCKET CONNECTED"`),
			where(par("last_handshake"))},
		"denied: the agent still counted sending": {dn, edit(denied, []string{"agents", "[0]", "nodes"},
			`{"total":1,"receiving":0,"sending":1,"archived":0}`), "$.agents[0].nodes.sending <layout>"},
		// two parents banned at their probes
		"banned: a session's ban said erroneous (m61)": {bn, edit(banned, second("ban"), `"it is erroneous"`),
			where(second("ban"))},
		"banned: the permanent ban said our parent": {bn, edit(banned, par("ban"), `"it is our parent"`),
			where(par("ban"))},
		"banned: the bans in the other order": {bn, edit(edit(banned, par("ban"), `"it is our parent"`), second("ban"),
			`"it is the localhost"`), where(par("ban")) + " " + where(second("ban"))},
		"banned: a banned parent's last handshake printed": {bn, add(banned, second("age"),
			`,"last_handshake":"ALREADY CONNECTED"`), members(second()) + " <layout>"},
		"banned: the reason NEVER CONNECTED":    {bn, edit(banned, st("reason"), `"NEVER CONNECTED"`), where(st("reason"))},
		"banned: a probe counted as an attempt": {bn, edit(banned, second("attempts"), "2"), where(second("attempts"))},
		"banned: the parents in the other order": {bn, edit(edit(banned, par("destination"),
			niStreamValue(t, banned, second("destination")...)), second("destination"),
			niStreamValue(t, banned, par("destination")...)), where(par("destination")) + " " + where(second("destination"))},
	}
}

// niDurationText writes us microseconds as C's duration text (duration_snprintf(..., "us", true)), the inverse of
// niDuration for the units it reads.
func niDurationText(us int64) string {
	if us == 0 {
		return "off"
	}
	var parts []string
	for _, u := range []struct {
		name string
		n    int64
	}{{"d", 86400e6}, {"h", 3600e6}, {"m", 60e6}, {"s", 1e6}, {"ms", 1e3}, {"us", 1}} {
		if us >= u.n {
			parts = append(parts, strconv.FormatInt(us/u.n, 10)+u.name)
			us %= u.n
		}
	}
	return strings.Join(parts, " ")
}

// niStreamShift is a recorded duration text (quoted) moved by us microseconds, quoted.
func niStreamShift(t *testing.T, text string, us int64) string {
	t.Helper()
	d, ok := niDuration(strings.Trim(text, `"`))
	if !ok {
		t.Fatalf("harness: %s is no duration C writes", text)
	}
	return strconv.Quote(niDurationText(d + us))
}

// niStreamPlus is the number text n plus d.
func niStreamPlus(t *testing.T, n string, d int64) string {
	t.Helper()
	x, err := strconv.ParseInt(n, 10, 64)
	if err != nil {
		t.Fatalf("harness: %q is no number", n)
	}
	return strconv.FormatInt(x+d, 10)
}

// niStreamItems are the paths of an array's first n items as Compare names them, separated by spaces.
func niStreamItems(where string, n int) string {
	var out []string
	for i := range n {
		out = append(out, where+"["+strconv.Itoa(i)+"]")
	}
	return strings.Join(out, " ")
}

// niStreamGuards pins the stages' guards: each refuses C's answer made wrong where a row would otherwise compare a
// wrong oracle (a stage the runner did not reach).
func niStreamGuards(t *testing.T) {
	t.Helper()
	at, st, dst, par, _ := niStreamPaths()
	for name, c := range map[string]struct {
		key  string
		edit func(string) string
	}{
		"never: a sender that was tried": {"never/never/v3-ni", func(b string) string {
			return niStreamEdit(t, b, par("attempts"), "2", "", false)
		}},
		"never: a sender online": {"never/never/v3-ni", func(b string) string {
			return niStreamEdit(t, b, st("status"), `"online"`, "", false)
		}},
		"never-tz: a parent's time in UTC": {"never-tz/never/v3-ni", func(b string) string {
			v := strings.Trim(niStreamValue(t, b, par("since")...), `"`)
			ts, _ := time.Parse(time.RFC3339Nano, v)
			return niStreamEdit(t, b, par("since"), `"`+ts.UTC().Format("2006-01-02T15:04:05.00Z07:00")+`"`, "", false)
		}},
		"connected: still replicating": {"out/connected/v3-ni", func(b string) string {
			return niStreamEdit(t, b, st("status"), `"replicating"`, "", false)
		}},
		"connected: not counted yet": {"out/connected/v3-ni", func(b string) string {
			return niStreamEdit(t, b, st("id"), "0", "", false)
		}},
		"connected: the parent still postponed": {"out/connected/v3-ni", func(b string) string {
			return niStreamEdit(t, b, par("last_handshake"), "", `,"next_check":"2026-10-09T15:00:00.00Z","next_in":"1s"`,
				false)
		}},
		"denied: the disconnect's reason": {"out/denied/v3-ni", func(b string) string {
			return niStreamEdit(t, b, st("reason"), `"DISCONNECT SOCKET ERROR"`, "", false)
		}},
		"denied: the place not reset yet": {"out/denied/v3-ni", func(b string) string {
			return niStreamEdit(t, b, par("skipped"), "false", "", false)
		}},
		"denied: a second refusal": {"out/denied/v3-ni", func(b string) string {
			return niStreamEdit(t, b, par("attempts"), "4", "", false)
		}},
		"denied: the ends kept": {"out/denied/v3-ni", func(b string) string {
			return niStreamEdit(t, b, dst("remote"), `"[127.0.0.1]:1"`, "", false)
		}},
		"connected: the first collection in the second it was seen connected": {"out/connected/v3-ni",
			func(b string) string {
				return niStreamEdit(t, b, at("db", "first_time"),
					strconv.FormatInt(niStreamRecorded["out/connected/v3-ni"].sides[0].connected[1], 10), "", false)
			}},
		"rfc3339: localhost's start a number": {"never/never/v3-ni-rfc3339", func(b string) string {
			d, _ := time.Parse(time.RFC3339, strings.Trim(niStreamValue(t, b, at("ingest", "since")...), `"`))
			return niStreamEdit(t, b, at("ingest", "since"), strconv.FormatInt(d.Unix(), 10), "", false)
		}},
		"banned: the session's ban said erroneous": {"ban/banned/v3-ni", func(b string) string {
			return niStreamEdit(t, b, dst("parents", "[1]", "ban"), `"it is erroneous"`, "", false)
		}},
	} {
		rec, row := niStreamRecordedRow(t, c.key)
		v, err := ParseJSON(row.fam.normalise(0, row.fam.v2Clock(row.req.target, rec.flight[0]), rec.flight[0],
			[]byte(c.edit(rec.body[0]))))
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if err := row.req.guard(v); err == nil {
			t.Errorf("the guard of %s takes %s", c.key, name)
		}
	}
}
