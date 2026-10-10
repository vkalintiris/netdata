// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"regexp"
	"strconv"
	"strings"
	"testing"
	"time"
)

// niRecordedPair is one recorded row of C against C (dash_norm_ni_data_test.go): its target, each side's own values
// of the family that asked it (niSide), the seconds each side's request was in flight, and both bodies.
type niRecordedPair struct {
	target string
	sides  [2]niSide
	flight [2][2]int64
	body   [2]string
}

// niJudged is what compareV2 makes of a recorded pair's flights with the bodies o and c, under the family fam and the
// row's guard: each body normalised by the family in its side's own seconds (the clock v2Clock gives the target),
// then, as v2Round judges them, each side's clock and tiers against its own flight, the oracle's guard, the clocks'
// shapes, the values under the family's masks, the layouts and the strings' escapes. The heads and the run directory
// are not here (the recorded bodies hold none). Empty when nothing is wrong; else each problem, in that order: `oracle
// clock` or `candidate clock`, `guard: ` and the guard's error, `clock shapes`, a value's path, `layout`, `escapes`.
func niJudged(t *testing.T, fam v2Family, guard func(Value) error, r niRecordedPair, o, c string) []string {
	t.Helper()
	var body [2][]byte
	var doc [2]Value
	var out []string
	for i, b := range []string{o, c} {
		clock := fam.v2Clock(r.target, r.flight[i])
		body[i] = fam.normalise(i, clock, r.flight[i], []byte(b))
		v, err := ParseJSON(body[i])
		if err != nil {
			t.Fatalf("%v: %s", err, body[i])
		}
		doc[i] = v
		for _, err := range []error{v2NowInFlight(v, clock), v2TiersInFlight(v, r.flight[i])} {
			if err != nil {
				out = append(out, []string{"oracle", "candidate"}[i]+" clock")
			}
		}
	}
	if guard != nil {
		if err := guard(doc[0]); err != nil {
			out = append(out, "guard: "+err.Error())
		}
	}
	if o, c := clockShapes(doc[0]), clockShapes(doc[1]); strings.Join(o, " ") != strings.Join(c, " ") {
		out = append(out, "clock shapes")
	}
	for _, d := range Compare(ApplyMasks(doc[0], fam.masks), ApplyMasks(doc[1], fam.masks), fam.unordered...) {
		out = append(out, d.Path)
	}
	if v2Layouts(body, fam.layoutByCount()) != "" {
		out = append(out, "layout")
	}
	if v2Escapes(body, doc, fam) != "" {
		out = append(out, "escapes")
	}
	return out
}

// niPlant is body with the one match of pattern replaced by what with makes of the match's groups (g[0] the whole
// match); `~` in pattern stands for the white space C's pretty JSON may put between two tokens. A pattern that does not
// match exactly once fails the pin: a plant that changed nothing would prove nothing.
func niPlant(t *testing.T, body, pattern string, with func(g []string) string) string {
	t.Helper()
	re := regexp.MustCompile(strings.ReplaceAll(pattern, "~", `\s*`))
	if n := len(re.FindAllStringIndex(body, -1)); n != 1 {
		t.Fatalf("harness: %s matches %d times, not once", pattern, n)
	}
	return re.ReplaceAllStringFunc(body, func(m string) string { return with(re.FindStringSubmatch(m)) })
}

// niRows are the rows the recorded pairs are of, by name: check `api.v2-node-instances`' (the gone row of the
// recorded fixture's base) and `sqlite.archived-hosts`' one.
func niRows() map[string]v2Req {
	rows := map[string]v2Req{}
	for _, r := range nodeInstancesRows() {
		rows[r.name] = r
	}
	rows["gone"] = niGoneRow(niRecordedBase)
	rows[niArchivedRow.name] = niArchivedRow
	return rows
}

// testDashNormNI pins SA-A's rows of `api.v2-node-instances` and `sqlite.archived-hosts` (H39): the render's date
// forms and gone window, the forms' guards, the gone row's facts, the archived row's family and facts, on the pairs C
// answered C against C (dash_norm_ni_data_test.go), and the helpers that make each side's windows.
func testDashNormNI(t *testing.T) {
	testDashNormNIRender(t)
	testDashNormNIRecorded(t)
	testDashNormNIPlanted(t)
	testDashNormNIGone(t)
}

// testDashNormNIRecorded pins that each recorded pair is of a row as it is (its target), that the row's guard takes
// C's answer and the family finds no difference between C's two, and that each row of SA-A's has its pair.
func testDashNormNIRecorded(t *testing.T) {
	rows := niRows()
	for _, name := range []string{"v3-ni-rfc3339", "v2-ni-long", "v3-ni-window", "gone", "archived"} {
		if _, ok := niRecordedPairs[name]; !ok {
			t.Errorf("no recorded pair of %s", name)
		}
	}
	for name, rec := range niRecordedPairs {
		r, ok := rows[name]
		if !ok {
			t.Errorf("the recorded pair %s is of no row", name)
			continue
		}
		if r.target != rec.target {
			t.Errorf("%s asks %s, its pair was recorded for %s", name, r.target, rec.target)
		}
		if got := niJudged(t, nodeInstancesFamily(rec.sides), r.guard, rec, rec.body[0], rec.body[1]); got != nil {
			t.Errorf("%s: C's recorded pair: %q", name, got)
		}
	}
	// the gone windows hold each side's detach and nothing before the close
	gone := niRecordedPairs["gone"]
	for i, side := range gone.sides {
		if side.gone[0] <= side.opened[1] || side.gone[1] < side.gone[0] || side.gone[1] >= gone.flight[i][0] {
			t.Errorf("gone, side %d: the gone window %v is not after the connection %v and before the request %v", i,
				side.gone, side.opened, gone.flight[i])
		}
	}
}

// testDashNormNIRender pins the render's pieces one by one.
func testDashNormNIRender(t *testing.T) {
	// C's date of a second, and nothing else
	for name, c := range map[string]struct {
		in   string
		want int64
		ok   bool
	}{
		"C's date":         {"2026-10-09T14:40:12Z", 1791556812, true},
		"a fraction":       {"2026-10-09T14:40:12.00Z", 0, false},
		"an offset":        {"2026-10-09T14:40:12+00:00", 0, false},
		"a local offset":   {"2026-10-09T17:40:12+03:00", 0, false},
		"a space":          {"2026-10-09 14:40:12Z", 0, false},
		"no zone":          {"2026-10-09T14:40:12", 0, false},
		"a short year":     {"26-10-09T14:40:12Z", 0, false},
		"a number":         {"1791556812", 0, false},
		"a word":           {"NOW", 0, false},
		"lower case":       {"2026-10-09t14:40:12z", 0, false},
		"a day past":       {"2026-10-32T14:40:12Z", 0, false},
		"a padded date":    {" 2026-10-09T14:40:12Z", 0, false},
		"an unpadded hour": {"2026-10-09T4:40:12Z", 0, false},
	} {
		if got, ok := niDate(c.in); got != c.want || ok != c.ok {
			t.Errorf("niDate, %s: %d %v, want %d %v", name, got, ok, c.want, c.ok)
		}
	}
	for name, c := range map[string]struct {
		in       string
		second   int64
		date, ok bool
	}{
		"a number":       {"1791556812", 1791556812, false, true},
		"a date":         {`"2026-10-09T14:40:12Z"`, 1791556812, true, true},
		"a date not C's": {`"2026-10-09T14:40:12.5Z"`, 0, true, false},
		"null":           {"null", 0, false, false},
		"a quote alone":  {`"`, 0, false, false},
		"a negative":     {"-1", -1, false, true},
	} {
		if second, date, ok := niSecond([]byte(c.in)); second != c.second || date != c.date || ok != c.ok {
			t.Errorf("niSecond, %s: %d %v %v, want %d %v %v", name, second, date, ok, c.second, c.date, c.ok)
		}
	}
	if niDateWord("START", false) != `"START"` || niDateWord("START", true) != `"rfc3339:START"` ||
		niShort.word("NOW") != `"NOW"` || niLong.word("NOW") != `"NOW"` || niDated.word("NOW") != `"rfc3339:NOW"` {
		t.Errorf("the words: %s %s %s %s %s", niDateWord("START", false), niDateWord("START", true),
			niShort.word("NOW"), niLong.word("NOW"), niDated.word("NOW"))
	}
	// the body's clock: a number or a date, the first `now`
	for name, c := range map[string]struct {
		in      string
		n       int64
		clocked bool
	}{
		"a number":           {`{"agents":[{"now":1791556812,"ai":0}]}`, 1791556812, true},
		"a date":             {`{"agents":[{"now":"2026-10-09T14:40:12Z","ai":0}]}`, 1791556812, true},
		"pretty":             {"{\n    \"now\": 1791556812,\n", 1791556812, true},
		"none":               {`{"agents":[{"ai":0}]}`, 0, false},
		"the first":          {`{"now":1791556812,"now":1791556813}`, 1791556812, true},
		"a word":             {`{"now":"NOW"}`, 0, false},
		"a date with offset": {`{"now":"2026-10-09T17:40:12+03:00"}`, 1791556812, true},
	} {
		if n, clocked := niNow([]byte(c.in)); n != c.n || clocked != c.clocked {
			t.Errorf("niNow, %s: %d %v, want %d %v", name, n, clocked, c.n, c.clocked)
		}
	}
	// a last time as a date reads NOW where it is the body's clock, and only there
	for name, c := range map[string]struct {
		in      string
		n       int64
		clocked bool
		want    string
	}{
		"the clock":        {`"last_time":"2026-10-09T14:40:12Z"`, 1791556812, true, `"last_time":"rfc3339:NOW"`},
		"a second before":  {`"last_time":"2026-10-09T14:40:11Z"`, 1791556812, true, ""},
		"no clock":         {`"last_time":"2026-10-09T14:40:12Z"`, 1791556812, false, ""},
		"a number":         {`"last_time":1791556812`, 1791556812, true, ""},
		"not C's date":     {`"last_time":"2026-10-09T14:40:12.00Z"`, 1791556812, true, ""},
		"the first time":   {`"first_time":"2026-10-09T14:40:12Z"`, 1791556812, true, ""},
		"a space kept":     {`"last_time": "2026-10-09T14:40:12Z"`, 1791556812, true, `"last_time": "rfc3339:NOW"`},
		"the family's NOW": {`"last_time":"NOW"`, 1791556812, true, ""},
		"null":             {`"last_time":null`, 1791556812, true, ""},
		"two, one the clock": {`"last_time":"2026-10-09T14:40:12Z","last_time":"2026-10-09T14:40:10Z"`, 1791556812, true,
			`"last_time":"rfc3339:NOW","last_time":"2026-10-09T14:40:10Z"`},
	} {
		want := c.want
		if want == "" {
			want = c.in
		}
		if got := string(niLastDate([]byte(c.in), c.n, c.clocked)); got != want {
			t.Errorf("niLastDate, %s: %s, want %s", name, got, want)
		}
	}
	// a side's words: each window by itself, the gone window only once set
	side := niSide{started: [2]int64{100, 105}, opened: [2]int64{110, 110}, gone: [2]int64{120, 121}}
	for since, want := range map[int64]string{99: "", 100: "START", 105: "START", 106: "", 109: "", 110: "CONNECTED",
		111: "", 119: "", 120: "GONE", 121: "GONE", 122: "", 0: ""} {
		if got := side.word(since); got != want {
			t.Errorf("niSide.word(%d): %q, want %q", since, got, want)
		}
	}
	unset := niSide{started: [2]int64{100, 105}, opened: [2]int64{110, 110}}
	for _, since := range []int64{0, 120} {
		if got := unset.word(since); got != "" {
			t.Errorf("niSide.word(%d) without a gone window: %q", since, got)
		}
	}
	// a pair without the fixture child (the archived hosts'): its start alone
	alone := niSide{started: [2]int64{100, 105}}
	for since, want := range map[int64]string{0: "", 100: "START", 1: ""} {
		if got := alone.word(since); got != want {
			t.Errorf("niSide.word(%d) without the child: %q, want %q", since, got, want)
		}
	}
}

// niWith are the plants of testDashNormNIPlanted: what one does to a recorded body (r's, of side i).
type niWith func(t *testing.T, r niRecordedPair, i int, body string) string

// niSecs is a plant that writes, for the match's group 1 kept, a number of seconds after it: the second the group 2
// date or number holds, moved by delta.
func niSecs(delta int64) func(g []string) string {
	return func(g []string) string {
		n, _, ok := niSecond([]byte(g[2]))
		if !ok {
			n, _, _ = niSecond([]byte(`"` + g[2] + `"`))
		}
		return g[1] + strconv.FormatInt(n+delta, 10)
	}
}

// niTo is a plant that writes text in place of the match.
func niTo(text string) func(g []string) string { return func([]string) string { return text } }

// niPlants are named wrong answers, each of one row: planted on the candidate's body (or, with oracle set, on the
// oracle's, where the row's guard must refuse it), and what niJudged reports then, each a path or a problem's name.
// The best wrong candidates are the forms a port slips on: a time written as a number under `rfc3339`, a short key
// under `long-json-keys`, the wall clock under a window, a gone child that still reads connected, an archived host
// with a child's members.
func niPlants() map[string]struct {
	row    string
	oracle bool
	plant  niWith
	want   string
} {
	at := func(pattern string, with func(g []string) string) niWith {
		return func(t *testing.T, _ niRecordedPair, _ int, body string) string {
			return niPlant(t, body, pattern, with)
		}
	}
	// the body's clock (niNow) moved by delta, in place of the match's group 2 after its group 1
	now := func(pattern string, delta int64) niWith {
		return func(t *testing.T, _ niRecordedPair, _ int, body string) string {
			n, _ := niNow([]byte(body))
			return niPlant(t, body, pattern, func(g []string) string {
				return g[1] + strconv.FormatInt(n+delta, 10)
			})
		}
	}
	// the side's wall clock (the end of its request's flight)
	wall := func(pattern string) niWith {
		return func(t *testing.T, r niRecordedPair, i int, body string) string {
			return niPlant(t, body, pattern, func(g []string) string {
				return g[1] + strconv.FormatInt(r.flight[i][1], 10)
			})
		}
	}
	const (
		childDated = `("mode":"ram",~"first_time":"[^"]*",~"last_time":)"([^"]*)"`
		localDated = `("mode":"dbengine",~"first_time":null,~"last_time":)"([^"]*)"`
		goneIngest = `"type":"archived",~"status":"offline",~"since":\d+,~"age":\d+,~"metrics":0,~"instances":0,~"contexts":0`
	)
	return map[string]struct {
		row    string
		oracle bool
		plant  niWith
		want   string
	}{
		// rfc3339: each time C writes as a date, written as the number of the same second
		"rfc3339: the child's last time as a number": {row: "v3-ni-rfc3339", plant: now(childDated, 0),
			want: "$.nodes[1].instances[0].db.last_time"},
		"rfc3339: localhost's last time as a number": {row: "v3-ni-rfc3339", plant: now(localDated, 0),
			want: "$.nodes[0].instances[0].db.last_time"},
		"rfc3339: the child's start as a number": {row: "v3-ni-rfc3339",
			plant: at(`("type":"child",~"status":"online",~"since":)"([^"]*)"`, niSecs(0)),
			want:  "$.nodes[1].instances[0].ingest.since"},
		"rfc3339: localhost's start as a number": {row: "v3-ni-rfc3339",
			plant: at(`("type":"localhost",~"status":"initializing",~"since":)"([^"]*)"`, niSecs(0)),
			want:  "$.nodes[0].instances[0].ingest.since"},
		"rfc3339: the child's first time as a number": {row: "v3-ni-rfc3339",
			plant: at(`("first_time":)"([^"]*)"`, niSecs(0)), want: "$.nodes[1].instances[0].db.first_time"},
		"rfc3339: localhost's first time 0, not null": {row: "v3-ni-rfc3339",
			plant: at(`"first_time":null`, niTo(`"first_time":0`)), want: "$.nodes[0].instances[0].db.first_time"},
		"rfc3339: a start with a fraction": {row: "v3-ni-rfc3339",
			plant: at(`("type":"child",~"status":"online",~"since":"[^"]*)Z"`, func(g []string) string {
				return g[1] + `.00Z"`
			}), want: "$.nodes[1].instances[0].ingest.since $.nodes[1].instances[0].ingest.age"},
		"rfc3339: a start in local time": {row: "v3-ni-rfc3339",
			plant: at(`("type":"child",~"status":"online",~"since":)"([^"]*)"`, func(g []string) string {
				s, _ := niDate(g[2])
				return g[1] + `"` + time.Unix(s, 0).In(time.FixedZone("", 3*3600)).Format(time.RFC3339) + `"`
			}), want: "$.nodes[1].instances[0].ingest.since $.nodes[1].instances[0].ingest.age"},
		"rfc3339: the agent's clock as a number": {row: "v3-ni-rfc3339",
			plant: at(`("now":)"([^"]*)"`, niSecs(0)), want: "clock shapes"},
		"rfc3339: the guard, the agent's clock as a number": {row: "v3-ni-rfc3339", oracle: true,
			plant: at(`("now":)"([^"]*)"`, niSecs(0)), want: "guard: agents.[0].now is {now}, want a date"},
		"rfc3339: the guard, the child's start as a number": {row: "v3-ni-rfc3339", oracle: true,
			plant: at(`("type":"child",~"status":"online",~"since":)"([^"]*)"`, niSecs(0)),
			want:  `guard: nodes.[1].instances.[0].ingest.since is "CONNECTED", want "rfc3339:CONNECTED"`},
		// long-json-keys: a short name where C writes the long one, and the agent's long where C keeps the short
		"long: a node's short index": {row: "v2-ni-long", plant: at(`"nodes_array_index":1`, niTo(`"ni":1`)),
			want: "$.nodes[1].<members>"},
		"long: localhost's status by its short names": {row: "v2-ni-long",
			plant: at(`"instances":\[\{"status":\{"agents_array_index":0,"code":200,"msg":""\},"db":\{"status":"initializing"`,
				niTo(`"instances":[{"st":{"ai":0,"code":200,"msg":""},"db":{"status":"initializing"`)),
			want: "$.nodes[0].instances[0].<members>"},
		"long: the agent's long names": {row: "v2-ni-long", plant: at(`"agents":\[\{"mg":`,
			niTo(`"agents":[{"machine_guid":`)), want: "$.agents[0].<members>"},
		"long: the guard, the child's short names": {row: "v2-ni-long", oracle: true,
			plant: at(`"machine_guid":"5a1e0000-0000-4000-8000-0000000000bb"`,
				niTo(`"mg":"5a1e0000-0000-4000-8000-0000000000bb"`)),
			want: "guard: no nodes.[1].machine_guid, want \"5a1e0000-0000-4000-8000-0000000000bb\""},
		// a window: the walk's clock is the wall's less one; a time of the wall clock is refused
		"window: the child's last time on the wall clock": {row: "v3-ni-window",
			plant: wall(`("mode":"ram",~"first_time":\d+,~"last_time":)(\d+)`),
			want:  "$.nodes[1].instances[0].db.last_time"},
		"window: the agent's clock on the wall": {row: "v3-ni-window", plant: wall(`("now":)(\d+)`),
			want: "candidate clock $.nodes[0].instances[0].ingest.age $.nodes[1].instances[0].ingest.age " +
				"$.agents[0].cloud.age"},
		"window: the guard, the child's last time on the wall clock": {row: "v3-ni-window", oracle: true,
			plant: wall(`("mode":"ram",~"first_time":\d+,~"last_time":)(\d+)`),
			want:  "guard: nodes.[1].instances.[0].db.last_time is {wall}, want \"NOW\""},
		// gone: what a child that is still connected writes, each alone
		"gone: a source": {row: "gone", plant: at(goneIngest, func(g []string) string {
			return g[0] + `,"source":{"local":"[127.0.0.1]:1","remote":"[127.0.0.1]:2","capabilities":[]}`
		}), want: "$.nodes[0].instances[0].ingest.<members> layout"},
		"gone: a reason": {row: "gone", plant: at(goneIngest, func(g []string) string {
			return g[0] + `,"reason":"DISCONNECTED SOCKET CLOSED BY REMOTE END"`
		}), want: "$.nodes[0].instances[0].ingest.<members> layout"},
		"gone: since its connection": {row: "gone", plant: func(t *testing.T, r niRecordedPair, i int, body string) string {
			n, _ := niNow([]byte(body))
			opened := r.sides[i].opened[1]
			return niPlant(t, body, `"since":\d+,(~)"age":\d+,(~)"metrics":0`, func(g []string) string {
				return `"since":` + strconv.FormatInt(opened, 10) + `,` + g[1] + `"age":` +
					strconv.FormatInt(n-opened, 10) + `,` + g[2] + `"metrics":0`
			})
		}, want: "$.nodes[0].instances[0].ingest.since"},
		"gone: since a second after its window": {row: "gone",
			plant: func(t *testing.T, r niRecordedPair, i int, body string) string {
				n, _ := niNow([]byte(body))
				late := r.sides[i].gone[1] + 1
				return niPlant(t, body, `"since":\d+,(~)"age":\d+,(~)"metrics":0`, func(g []string) string {
					return `"since":` + strconv.FormatInt(late, 10) + `,` + g[1] + `"age":` +
						strconv.FormatInt(n-late, 10) + `,` + g[2] + `"metrics":0`
				})
			}, want: "$.nodes[0].instances[0].ingest.since"},
		"gone: since a second before its window": {row: "gone",
			plant: func(t *testing.T, r niRecordedPair, i int, body string) string {
				n, _ := niNow([]byte(body))
				early := r.sides[i].gone[0] - 1
				return niPlant(t, body, `"since":\d+,(~)"age":\d+,(~)"metrics":0`, func(g []string) string {
					return `"since":` + strconv.FormatInt(early, 10) + `,` + g[1] + `"age":` +
						strconv.FormatInt(n-early, 10) + `,` + g[2] + `"metrics":0`
				})
			}, want: "$.nodes[0].instances[0].ingest.since"},
		"gone: the collected counts": {row: "gone",
			plant: at(`("since":\d+,~"age":\d+,~)"metrics":0,(~)"instances":0,(~)"contexts":0`, func(g []string) string {
				return g[1] + `"metrics":7,` + g[2] + `"instances":2,` + g[3] + `"contexts":1`
			}),
			want: "$.nodes[0].instances[0].ingest.metrics $.nodes[0].instances[0].ingest.instances " +
				"$.nodes[0].instances[0].ingest.contexts"},
		"gone: no functions": {row: "gone", plant: at(`"functions":\{~\},~`, niTo("")),
			want: "$.nodes[0].instances[0].<members> layout"},
		"gone: a child's type": {row: "gone", plant: at(`"type":"archived"`, niTo(`"type":"child"`)),
			want: "$.nodes[0].instances[0].ingest.type"},
		"gone: live": {row: "gone", plant: at(`"liveness":"stale"`, niTo(`"liveness":"live"`)),
			want: "$.nodes[0].instances[0].db.liveness"},
		"gone: the last time now": {row: "gone", plant: now(`("first_time":\d+,~"last_time":)(\d+)`, 0),
			want: "$.nodes[0].instances[0].db.last_time"},
		"gone: the agent still receiving": {row: "gone", plant: at(`"receiving":0,(~)"sending":0,(~)"archived":1`,
			func(g []string) string { return `"receiving":1,` + g[1] + `"sending":0,` + g[2] + `"archived":0` }),
			want: "$.agents[0].nodes.receiving $.agents[0].nodes.archived"},
		"gone: the guard, a source": {row: "gone", oracle: true, plant: at(goneIngest, func(g []string) string {
			return g[0] + `,"source":{}`
		}), want: "guard: nodes.[0].instances.[0].ingest has members [id hops type status since age metrics instances " +
			"contexts source], want [id hops type status since age metrics instances contexts]"},
		"gone: the guard, collected counts": {row: "gone", oracle: true,
			plant: at(`("since":\d+,~"age":\d+,~)"metrics":0`, func(g []string) string { return g[1] + `"metrics":7` }),
			want:  "guard: nodes.[0].instances.[0].ingest.metrics is 7, want 0"},
		"gone: the guard, since its connection": {row: "gone", oracle: true,
			plant: func(t *testing.T, r niRecordedPair, i int, body string) string {
				n, _ := niNow([]byte(body))
				opened := r.sides[i].opened[1]
				return niPlant(t, body, `"since":\d+,~"age":\d+,~"metrics":0`, niTo(`"since":`+
					strconv.FormatInt(opened, 10)+`,"age":`+strconv.FormatInt(n-opened, 10)+`,"metrics":0`))
			}, want: `guard: nodes.[0].instances.[0].ingest.since is "CONNECTED", want "GONE"`},
		"gone: the guard, a function": {row: "gone", oracle: true, plant: at(`"functions":\{~\}`,
			niTo(`"functions":{"f":{}}`)), want: `guard: nodes.[0].instances.[0].functions is {"f":{}}, want {}`},
		"gone: the guard, no functions": {row: "gone", oracle: true, plant: at(`"functions":\{~\},~`, niTo("")),
			want: "guard: nodes.[0].instances.[0] has members [st db ingest ml health capabilities dyncfg], want [st db " +
				"ingest ml health functions capabilities dyncfg]"},
		"gone: the guard, the last time a second earlier": {row: "gone", oracle: true,
			plant: at(`("first_time":\d+,~"last_time":)(\d+)`, niSecs(-1)),
			want:  "guard: nodes.[0].instances.[0].db.last_time is {base+59}, want {base+60}"},
		"gone: the guard, the agent still receiving": {row: "gone", oracle: true,
			plant: at(`"receiving":0,~"sending":0,~"archived":1`, niTo(`"receiving":1,"sending":0,"archived":0`)),
			want:  `guard: agents.[0].nodes is {"total":2,"receiving":1,"sending":0,"archived":0}, want {"total":2,"receiving":0,"sending":0,"archived":1}`},
		// archived: what a host that had a receiver, or a registry, or a sender, writes
		"archived: functions": {row: "archived",
			plant: at(`("nm":"parity-parent",(?s:.*?)"health":\{~"status":"disabled"~\},~)"capabilities"`,
				func(g []string) string { return g[1] + `"functions":{},"capabilities"` }),
			want: "$.nodes[0].instances[0].<members> layout"},
		"archived: a stream": {row: "archived", plant: at(`("nm":"parity-child",(?s:.*?))("ml":\{)`,
			func(g []string) string { return g[1] + `"stream":{"id":0,"hops":2,"status":"offline"},` + g[2] }),
			want: "$.nodes[1].instances[0].<members> layout"},
		"archived: since 0": {row: "archived",
			plant: func(t *testing.T, _ niRecordedPair, _ int, body string) string {
				n, _ := niNow([]byte(body))
				return niPlant(t, body, `("nm":"parity-parent",(?s:.*?)"status":"archived",~)"since":\d+,(~)"age":\d+`,
					func(g []string) string { return g[1] + `"since":0,` + g[2] + `"age":` + strconv.FormatInt(n, 10) })
			}, want: "$.nodes[0].instances[0].ingest.since"},
		"archived: since now": {row: "archived",
			plant: func(t *testing.T, _ niRecordedPair, _ int, body string) string {
				n, _ := niNow([]byte(body))
				return niPlant(t, body, `("nm":"parity-parent",(?s:.*?)"status":"archived",~)"since":\d+,(~)"age":\d+`,
					func(g []string) string { return g[1] + `"since":` + strconv.FormatInt(n, 10) + `,` + g[2] + `"age":0` })
			}, want: "$.nodes[0].instances[0].ingest.since"},
		"archived: offline": {row: "archived", plant: at(`("nm":"parity-child",(?s:.*?))"status":"archived"`,
			func(g []string) string { return g[1] + `"status":"offline"` }),
			want: "$.nodes[1].instances[0].ingest.status"},
		"archived: the last time now": {row: "archived",
			plant: now(`("nm":"parity-parent",(?s:.*?)"first_time":0,~"last_time":)(0)`, 0),
			want:  "$.nodes[0].instances[0].db.last_time"},
		"archived: the agent receiving them": {row: "archived", plant: at(`"receiving":0,(~)"sending":0,(~)"archived":2`,
			func(g []string) string { return `"receiving":2,` + g[1] + `"sending":0,` + g[2] + `"archived":0` }),
			want: "$.agents[0].nodes.receiving $.agents[0].nodes.archived"},
		"archived: the guard, functions": {row: "archived", oracle: true,
			plant: at(`("nm":"parity-parent",(?s:.*?)"health":\{~"status":"disabled"~\},~)"capabilities"`,
				func(g []string) string { return g[1] + `"functions":{},"capabilities"` }),
			want: "guard: nodes.[0].instances.[0] has members [st db ingest ml health functions capabilities dyncfg], " +
				"want [st db ingest ml health capabilities dyncfg]"},
		"archived: the guard, since 0": {row: "archived", oracle: true,
			plant: func(t *testing.T, _ niRecordedPair, _ int, body string) string {
				n, _ := niNow([]byte(body))
				return niPlant(t, body, `("nm":"parity-parent",(?s:.*?)"status":"archived",~)"since":\d+,(~)"age":\d+`,
					func(g []string) string { return g[1] + `"since":0,` + g[2] + `"age":` + strconv.FormatInt(n, 10) })
			}, want: `guard: nodes.[0].instances.[0].ingest.since is 0, want "START"`},
		"archived: the guard, a connection": {row: "archived", oracle: true,
			plant: at(`("nm":"parity-child",(?s:.*?)"ingest":\{~)"id":0`, func(g []string) string { return g[1] + `"id":1` }),
			want:  "guard: nodes.[1].instances.[0].ingest.id is 1, want 0"},
		"archived: the guard, the contexts' version": {row: "archived", oracle: true,
			plant: at(`"contexts_hard_hash":0`, niTo(`"contexts_hard_hash":13`)),
			want:  "guard: versions.contexts_hard_hash is 13, want 0"},
	}
}

// testDashNormNIPlanted pins that each named wrong answer is reported, at the paths it changes and nowhere else, and
// that each row's guard refuses the oracle's wrong answers.
func testDashNormNIPlanted(t *testing.T) {
	rows := niRows()
	for name, c := range niPlants() {
		rec, ok := niRecordedPairs[c.row]
		if !ok {
			t.Fatalf("%s: no recorded pair of %s", name, c.row)
		}
		o, cand := rec.body[0], rec.body[1]
		if c.oracle {
			o = c.plant(t, rec, 0, o)
		} else {
			cand = c.plant(t, rec, 1, cand)
		}
		problems := niJudged(t, nodeInstancesFamily(rec.sides), rows[c.row].guard, rec, o, cand)
		got := strings.Join(problems, " ")
		if c.oracle {
			// the guard's error alone: the comparison then reports the plant as well
			got = ""
			for _, p := range problems {
				if strings.HasPrefix(p, "guard: ") {
					got = p
				}
			}
		}
		// the recorded run's own values: the oracle's clock and wall clock, the fixture's base
		n, _ := niNow([]byte(rec.body[0]))
		want := strings.NewReplacer("{now}", strconv.FormatInt(n, 10), "{wall}", strconv.FormatInt(rec.flight[0][1], 10),
			"{base+59}", strconv.FormatInt(niRecordedBase+59, 10), "{base+60}", strconv.FormatInt(niRecordedBase+60, 10)).
			Replace(c.want)
		if got != want {
			t.Errorf("%s: %q, want %q", name, got, want)
		}
	}
}

// testDashNormNIGone pins niGone's two pieces that need no agent: the poll's judge, and the windows read from two
// stub agents.
func testDashNormNIGone(t *testing.T) {
	ok := func(body string) []byte {
		return []byte("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n" + body)
	}
	child := `{"nodes":[{"mg":"x","nm":"parity-child","ni":0,"instances":[{"ingest":{"since":1791558630,"age":1}}]}]}`
	for name, c := range map[string]struct {
		answer []byte
		since  int64
		ok     bool
	}{
		"C's answer":           {ok(child), 1791558630, true},
		"another node first":   {ok(`{"nodes":[{"nm":"parity-child2","instances":[{"ingest":{"since":1}}]},` + child[10:]), 1791558630, true},
		"a longer name alone":  {ok(strings.Replace(child, "parity-child", "parity-child2", 1)), 0, false},
		"no instance":          {ok(`{"nodes":[{"nm":"parity-child","instances":[]}]}`), 0, false},
		"a date":               {ok(strings.Replace(child, `1791558630`, `"2026-10-09T15:10:30Z"`, 1)), 0, false},
		"a 404":                {[]byte("HTTP/1.1 404 Not Found\r\n\r\nnot found"), 0, false},
		"a body that is cut":   {ok(child[:40]), 0, false},
		"no status line":       {[]byte(""), 0, false},
		"a name that is a key": {ok(strings.Replace(child, `"nm":"parity-child"`, `"nm":1`, 1)), 0, false},
	} {
		if since, ok := niGoneSince(c.answer, "parity-child"); since != c.since || ok != c.ok {
			t.Errorf("niGoneSince, %s: %d %v, want %d %v", name, since, ok, c.since, c.ok)
		}
	}

	// two stub agents: the oracle's child stamped at the close's own second at once, the candidate's after a moment,
	// before which its ingestion still starts at the connection
	from := time.Now().Unix()
	stampAt := time.Now().Add(700 * time.Millisecond)
	body := func(since int64) string {
		return `{"nodes":[{"nm":"parity-child","instances":[{"ingest":{"since":` + strconv.FormatInt(since, 10) + `}}]}]}`
	}
	p := &Pair{
		Oracle: dashNormStub(t, stubAnswer{body: func(int64) string { return body(from) }}.raw),
		Candidate: dashNormStub(t, stubAnswer{body: func(int64) string {
			if time.Now().Before(stampAt) {
				return body(from - 5)
			}
			return body(from)
		}}.raw),
	}
	gone := niGoneWindows(t, p, childHost, from)
	end := time.Now().Unix()
	if gone[0][0] != from || gone[0][1] < from || gone[0][1] > end || gone[1][0] != from ||
		gone[1][1] < stampAt.Unix() || gone[1][1] > end {
		t.Errorf("niGoneWindows: %v, want each from %d, the oracle's to its first answer, the candidate's to %d or "+
			"later, both by %d", gone, from, stampAt.Unix(), end)
	}
}
