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
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// timingsRe finds the query timings of a json wrapper: wall-clock figures. Their width varies, so the length
// header is masked too (the bodies are compared whole).
var (
	timingsRe       = regexp.MustCompile(`"?(prep|query|output|total|cloud)_ms"?:[-+0-9.e]+`)
	contentLengthRe = regexp.MustCompile(`(?m)^Content-Length: [0-9]+`)
	// v2 wrappers: the answering agent's clock, and the context dictionary's version, which counts worker-timed
	// update events (spec §11).
	v2ClockRe = regexp.MustCompile(`"(now|contexts_hard_hash)":("[^"]*"|[0-9]+)`)
	// agentNowRe is the answering agent's clock: a number of seconds or, with `options=rfc3339`, a date
	// (api_v2_contexts_agents.c:25).
	agentNowRe = regexp.MustCompile(`"now":(\s*)([0-9]+|"[^"]*")`)
)

// agentNowIn tells whether an answering agent's clock, as agentNowRe's second group has it (a JSON number or
// string), is a second in [from, to] (v2Second).
func agentNowIn(text []byte, from, to int64) bool {
	clock, err := ParseJSON(text)
	if err != nil {
		return false
	}
	v, err := v2Second(clock)
	return err == nil && v >= from && v <= to
}

// maskAgentNow writes NOW for each `now` of b that is a second in [from, to], the seconds the request was in flight:
// the answering agent reads its clock while it answers (api_v2_contexts_agents.c:12-13, :25: the data wrapper and the
// dyncfg tree pass it no clock of their own, jsonwrap-v2.c:558, dyncfg-tree.c:162). Any other value is left to the
// comparison: a 0, a clock in milliseconds, a second outside the flight.
func maskAgentNow(b []byte, from, to int64) []byte {
	return agentNowRe.ReplaceAllFunc(b, func(m []byte) []byte {
		g := agentNowRe.FindSubmatch(m)
		if !agentNowIn(g[2], from, to) {
			return m
		}
		return []byte(`"now":` + string(g[1]) + `"NOW"`)
	})
}

// dataStrayNow takes each `now` of b that is no second in [from, to] out of maskTimings' reach: its key then reads
// `now, not a second of the request's flight`, its value as the agent wrote it. maskTimings hides a `now` whatever it
// holds (maskClock, as the checks outside the dashboard's use it); a data answer's is the answering agent's clock
// (maskAgentNow), so one the flight does not hold is left for the comparison, under a name that says why.
func dataStrayNow(b []byte, from, to int64) []byte {
	return agentNowRe.ReplaceAllFunc(b, func(m []byte) []byte {
		g := agentNowRe.FindSubmatch(m)
		if agentNowIn(g[2], from, to) {
			return m
		}
		return []byte(`"now, not a second of the request's flight":` + string(g[1]) + string(g[2]))
	})
}

// maskNowKeys writes "NOW" for each member of b named one of keys whose value is a second in [from, to], the seconds
// the request was in flight: an agent prints its clock there for what is collected (a detailed tree's last entries; a
// v2 walk's context `last_entry`, database/contexts/api_v2_contexts.c:1213, and an online host's `db.last_time`,
// rrdhost.h:617-618, from the walk's one `now`, :1374). Any other value is left to the comparison.
func maskNowKeys(b []byte, from, to int64, keys ...string) []byte {
	quoted := make([]string, len(keys))
	for i, k := range keys {
		quoted[i] = regexp.QuoteMeta(k)
	}
	re := regexp.MustCompile(`"(` + strings.Join(quoted, "|") + `)":(\d+)`)
	return re.ReplaceAllFunc(b, func(m []byte) []byte {
		sub := re.FindSubmatch(m)
		v, err := strconv.ParseInt(string(sub[2]), 10, 64)
		if err == nil && v >= from && v <= to {
			return []byte(`"` + string(sub[1]) + `":"NOW"`)
		}
		return m
	})
}

// maskNowEntries is maskNowKeys of a data answer's last entries (the detailed tree's `le` and `last_entry`).
func maskNowEntries(b []byte, from, to int64) []byte {
	return maskNowKeys(b, from, to, "le", "last_entry")
}

// dataMask hides what differs between two data answers of one fixture, given the seconds [from, to] the request was
// in flight: the head's clock and expiry read against them (maskAnswer: an absolute window's answer expires a day
// after its Date, a relative one's with it), the transaction id, the length, the contexts' version and the timings
// (maskTimings), and what holds the clock of now: the answering agent's `now`, which is masked only where it is a
// second of the flight (dataStrayNow), and the last entries.
func dataMask(b []byte, from, to int64) []byte {
	b = dataStrayNow(maskAnswer(b, [2]int64{from, to}), from, to)
	return maskNowEntries(maskTimings(b), from, to)
}

// dataAgree tells whether two masked data answers agree but for the labels' order (labelOrderOnly).
func dataAgree(a, b []byte) bool { return bytes.Equal(a, b) || labelOrderOnly(a, b) }

// dataJudge judges the oracle's masked data answer and compares the two (exactJudge); where they differ only in the
// labels' order, the candidate's counts as the oracle's.
func dataJudge(t *testing.T, got [2][]byte, want [2]string, holds []string, guard func(Value) error) {
	t.Helper()
	if labelOrderOnly(got[0], got[1]) {
		got[1] = got[0]
	}
	exactJudge(t, got, want, holds, guard)
}

// dataDashboardType is the type a browser's fetch() gives a string payload the page sends without one (the Fetch
// standard, "extract a body"): the dashboard's data POST.
const dataDashboardType = "Content-Type: text/plain;charset=UTF-8"

// dataDashboardBody is the payload the dashboard POSTs to `/api/v3/data` for a context's latest value (WEB/v3/app.*.js
// @191540, its key order; the window is the last 600 s before now). C reads no payload (api_v2_data.c:20-340).
func dataDashboardBody(context string, now int64) []byte {
	return fmt.Appendf(nil, `{"format":"json2","scope":{"contexts":[%q]},"aggregations":{"metrics":[`+
		`{"group_by":["nodes"],"group_by_label":[],"aggregation":"avg"}],"time":{"time_group":"avg",`+
		`"time_resampling":0}},"window":{"after":%d,"before":%d,"points":1}}`, context, now-600, now)
}

// maskClock hides a v2 answer's clock and its contexts' version (v2ClockRe).
func maskClock(b []byte) []byte { return v2ClockRe.ReplaceAll(b, []byte(`"$1":"<masked>"`)) }

func maskTimings(b []byte) []byte {
	b = maskClock(contentLengthRe.ReplaceAll(b, []byte("Content-Length: <masked>")))
	return timingsRe.ReplaceAllFunc(b, func(m []byte) []byte {
		name, _, _ := bytes.Cut(m, []byte(":"))
		// A fresh slice: appending to name would write over the source after the match.
		return append(append([]byte{}, name...), `:"<masked>"`...)
	})
}

// dataCharts are the words of the data fixture's CHART lines that name and describe its two charts: their type (the
// charts are <type>.a and <type>.two), the first one's name, the titles, the units, the family, the context, and
// the first one's priority (the second's is one more).
type dataCharts struct {
	typ, name, titleA, titleTwo, units, family, context string
	priority                                            int
}

// qCharts are the fixture child's charts: q.a (named q_a_name) and q.two, of context q.ctx.
var qCharts = dataCharts{typ: "q", name: "q_a_name", titleA: "title a", titleTwo: "title two", units: "units",
	family: "fam", context: "q.ctx", priority: 1000}

// streamDataFixture sends the fixture child's charts (qCharts) and their data (streamChartsFixture).
func streamDataFixture(t *testing.T, conn *stream.Conn, base int64) {
	t.Helper()
	streamChartsFixture(t, conn, base, qCharts)
}

// streamChartsFixture sends a child two charts of one context, as cs names them: <type>.a (update every 1 s: a with
// gaps, b with negatives, resets and anomalous samples, z always zero, inc incremental, h hidden) and <type>.two
// (update every 2 s), ending at base+60.
func streamChartsFixture(t *testing.T, conn *stream.Conn, base int64, cs dataCharts) {
	t.Helper()
	conn.Linef("CHART '%s.a' '%s' '%s' '%s' '%s' '%s' line %d 1 '' fixture-pusher corpus", cs.typ, cs.name, cs.titleA,
		cs.units, cs.family, cs.context, cs.priority)
	conn.Linef("DIMENSION 'a' 'alpha' absolute 1 1 ''")
	conn.Linef("DIMENSION 'b' '' absolute 1 1 ''")
	conn.Linef("DIMENSION 'z' '' absolute 1 1 ''")
	conn.Linef("DIMENSION 'inc' '' incremental 1 1 ''")
	conn.Linef("DIMENSION 'h' '' absolute 1 1 'hidden'")
	conn.Linef("CLABEL 'k' 'v1' 2")
	conn.Linef("CLABEL_COMMIT")
	conn.Linef("CHART '%s.two' '' '%s' '%s' '%s' '%s' line %d 2 '' fixture-pusher corpus", cs.typ, cs.titleTwo,
		cs.units, cs.family, cs.context, cs.priority+1)
	conn.Linef("DIMENSION 'a' '' absolute 1 1 ''")
	conn.Linef("DIMENSION 'b' '' absolute 1 1 ''")
	conn.Linef("CLABEL 'k' 'v2' 2")
	conn.Linef("CLABEL_COMMIT")
	for i := int64(1); i <= 60; i++ {
		conn.Linef("BEGIN2 '%s.a' 1 %d #", cs.typ, base+i)
		if i%9 == 0 {
			conn.Linef("SET2 'a' 0 0 E")
		} else {
			conn.Linef("SET2 'a' %d %g A", i, float64(i)*1.5)
		}
		flags := "A"
		if i%4 == 0 {
			flags = ""
		}
		if i == 20 {
			flags += "R"
		}
		if flags == "" {
			flags = "#"
		}
		conn.Linef("SET2 'b' %d %d %s", i, (i*7)%13-6, flags)
		conn.Linef("SET2 'z' 0 0 A")
		conn.Linef("SET2 'inc' %d 2.5 A", i)
		conn.Linef("SET2 'h' %d %d A", i, i)
		conn.Linef("END2")
		if i%2 == 0 {
			conn.Linef("BEGIN2 '%s.two' 2 %d #", cs.typ, base+i)
			conn.Linef("SET2 'a' %d %d A", i, 100+i)
			conn.Linef("SET2 'b' %d %d A", i, -i)
			conn.Linef("END2")
		}
	}
	if err := conn.Flush(); err != nil {
		t.Fatal(err)
	}
}

// TestDataAPI streams a fixture child into both daemons (ram, one tier) and compares /api/v1/data byte for byte
// over absolute windows: time groupings, natural and virtual points, formats, options and errors.
func TestDataAPI(t *testing.T) {
	p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1}, parentIdentity)
	base := dashBase()
	dashChild(t, p, base)

	win := fmt.Sprintf("after=%d&before=%d", base, base+60)
	chart := "/api/v1/data?chart=q.a&" + win
	ctx := "/api/v1/data?context=q.ctx&" + win
	cases := map[string]string{
		"natural":          chart,
		"natural-wrapped":  chart + "&options=jsonwrap",
		"virtual":          chart + "&points=13&options=jsonwrap",
		"aligned":          chart + "&points=7",
		"unaligned":        chart + "&points=7&options=unaligned",
		"two-natural":      "/api/v1/data?chart=q.two&" + win + "&options=jsonwrap",
		"two-virtual":      "/api/v1/data?chart=q.two&" + win + "&points=11",
		"by-name":          "/api/v1/data?chart=q_a_name&" + win + "&points=5&options=jsonwrap",
		"context":          ctx + "&points=10&options=jsonwrap",
		"context-dims":     ctx + "&points=10&dims=a&dimension=b&options=jsonwrap",
		"context-label":    ctx + "&points=4&chart_label_key=k&chart_labels_filter=k:v2&options=jsonwrap",
		"context-limit":    ctx + "&points=4&limit=2&options=jsonwrap",
		"context-card":     ctx + "&points=4&cardinality_limit=3&options=jsonwrap",
		"all-dimensions":   ctx + "&points=3&show_dimensions=1&options=jsonwrap",
		"debug":            ctx + "&points=3&options=jsonwrap,debug",
		"latest":           chart + "&points=1&group=latest&options=jsonwrap",
		"latest-anomaly":   chart + "&points=1&group=latest&options=jsonwrap,anomaly-bit",
		"abs-null2zero":    chart + "&points=9&options=abs,null2zero,flip,ms,objectrows",
		"nonzero":          chart + "&points=6&options=nonzero,jsonwrap",
		"nonzero-dims":     chart + "&points=6&dims=z|b&options=nonzero,jsonwrap",
		"percentage":       chart + "&points=6&options=percentage,jsonwrap",
		"raw":              chart + "&points=6&options=raw,jsonwrap",
		"rfc3339":          chart + "&points=4&options=rfc3339,jsonwrap",
		"anomaly-bit":      chart + "&points=6&options=anomaly-bit",
		"jwar":             chart + "&points=6&options=jsonwrap,jw-anomaly-rates",
		"minify":           chart + "&points=6&options=jsonwrap,minify",
		"resampling":       chart + "&points=6&gtime=4",
		"tier-selected":    chart + "&points=6&tier=0&options=jsonwrap",
		"countif":          chart + "&points=7&group=countif&group_options=>5",
		"csv":              chart + "&points=5&format=csv",
		"csv-seconds":      chart + "&points=5&format=csv&options=seconds,label-quotes",
		"csv-wrapped":      chart + "&points=5&format=csv&options=jsonwrap",
		"tsv":              chart + "&points=5&format=tsv&options=ms",
		"markdown":         chart + "&points=5&format=markdown",
		"html":             chart + "&points=5&format=html",
		"html-wrapped":     chart + "&points=5&format=html&options=jsonwrap",
		"ssv":              chart + "&points=5&format=ssv",
		"ssv-min2max":      chart + "&points=5&format=ssvcomma&options=min2max,jsonwrap",
		"ssv-average":      chart + "&points=5&format=ssv&options=average",
		"array":            chart + "&points=5&format=array&options=min",
		"array-wrapped":    chart + "&points=5&format=array&options=max,jsonwrap",
		"csvjsonarray":     chart + "&points=5&format=csvjsonarray",
		"csvjsonarray-ms":  chart + "&points=5&format=csvjsonarray&options=ms,jsonwrap",
		"datatable":        chart + "&points=5&format=datatable",
		"datatable-google": chart + "&points=5&format=datatable&options=google_json,jsonwrap",
		"datasource":       chart + "&points=5&format=datasource&tqx=reqId:3;sig:0",
		"datasource-old":   chart + "&points=5&tqx=out:json;sig:99999999999",
		"jsonp":            chart + "&points=5&format=jsonp&callback=cb",
		"jsonp-wrapped":    chart + "&points=5&format=jsonp&options=jsonwrap",
		"json-google":      chart + "&points=5&options=google_json",
		"json2":            chart + "&points=5&format=json2",
		"json2-long-keys":  chart + "&points=5&format=json2&options=long-json-keys,rfc3339,null2zero",
		"filename":         chart + "&points=2&filename=out.csv&format=csv&options=seconds",
		"no-target":        "/api/v1/data?" + win,
		"star-target":      "/api/v1/data?chart=*&" + win,
		"unknown-chart":    "/api/v1/data?chart=nope&" + win,
		"past-the-data":    fmt.Sprintf("/api/v1/data?chart=q.a&after=%d&before=%d", base+500, base+600),
		"cancelled":        chart + "&timeout=-1",
		"cancelled-jsonp":  chart + "&timeout=-1&format=jsonp",
		"unknown-keywords": chart + "&points=3&format=nope&group=nope&options=nope,jsonwrap&unknown=1",
		"tier-too-high":    chart + "&points=3&tier=5&options=jsonwrap",
		"points-huge":      chart + "&points=100000",
		"points-negative":  chart + "&points=-1&options=jsonwrap",
		"after-gt-before":  fmt.Sprintf("/api/v1/data?chart=q.a&after=%d&before=%d&points=3&options=jsonwrap", base+60, base),
		"overflow":         "/api/v1/data?chart=q.a&after=-9223372036854775808&before=9223372036854775807",
		"dims-negative":    chart + "&points=3&dims=!a|*&options=jsonwrap",
	}
	for _, g := range []string{"min", "max", "sum", "median", "stddev", "cv", "ses", "des", "incremental_sum",
		"percentile", "trimmed-mean", "extremes", "average", "trimmed-median10"} {
		cases["group-"+g] = chart + "&points=7&group=" + g
		cases["group-two-"+g] = "/api/v1/data?chart=q.two&" + win + "&points=4&group=" + g
	}
	// v2/v3 walk every host: scope them to the child (localhost's pulse charts are compared by `pulse.localhost-charts`).
	v3 := "/api/v3/data?scope_nodes=" + childHost.Hostname + "&scope_contexts=q.ctx&" + win
	for name, extra := range map[string]string{
		"default":            "&points=6",
		"natural":            "",
		"v2":                 "&points=6&__v2",
		"group-instance":     "&points=4&group_by=instance",
		"group-label":        "&points=4&group_by=label&group_by_label=k",
		"group-selected-sum": "&points=4&group_by=selected&aggregation=sum",
		"group-node-max":     "&points=4&group_by=node&aggregation=max",
		"group-context-min":  "&points=4&group_by=context&aggregation=min",
		"group-units":        "&points=4&group_by=units&aggregation=extremes",
		"two-pass":           "&points=4&group_by[0]=dimension&group_by[1]=node&aggregation[1]=max",
		"two-pass-label":     "&points=4&group_by[0]=instance&group_by[1]=label&group_by_label[1]=k&aggregation[1]=sum",
		"pct-of-instance":    "&points=4&group_by=percentage-of-instance",
		"aggregation-pct":    "&points=4&group_by=instance&aggregation=percentage&dimensions=a",
		"raw":                "&points=4&options=raw",
		"raw-pct":            "&points=4&options=raw&group_by=instance&aggregation=percentage&dimensions=a",
		"debug":              "&points=4&options=debug&tier=0&time_group_options=5&timeout=1000",
		"minimal":            "&points=4&options=minimal-stats",
		"details":            "&points=4&options=details",
		"details-all":        "&points=4&options=details,all-dimensions&dimensions=a",
		"long-keys":          "&points=4&options=long-json-keys,rfc3339,null2zero",
		"nonzero":            "&points=4&options=nonzero&dimensions=z|b",
		"percentage":         "&points=4&options=percentage",
		"limit":              "&points=4&limit=2",
		"limit-summaries":    "&points=4&cardinality_limit=2&options=cardinality-limit-all",
		"group-by-labels":    "&points=4&options=group-by-labels&group_by=dimension",
		"time-group-sum":     "&points=4&time_group=sum",
		"anomaly-bit":        "&points=4&options=anomaly-bit",
		"mcp-info":           "&points=2&options=mcp-info",
		"csv":                "&points=4&format=csv",
		"datatable":          "&points=4&format=datatable",
		"no-match":           "&points=4&contexts=nothing",
		"labels-filter":      "&points=4&labels=k:v2",
		"instances-filter":   "&points=4&instances=q.two",
		"unknown-keywords":   "&points=3&format=nope&time_group=nope&aggregation=nope&options=nope&unknown=1",
		"tier-too-high":      "&points=3&tier=5&options=debug",
		"points-huge":        "&points=100000&format=csv",
		"labels-negative":    "&points=3&labels=!k:v1",
		"overflow-window":    "&after=-9223372036854775808&before=9223372036854775807",
	} {
		path := v3 + extra
		if strings.HasSuffix(extra, "&__v2") {
			path = strings.Replace(strings.TrimSuffix(path, "&__v2"), "/api/v3/", "/api/v2/", 1)
		}
		cases["v3-"+name] = path
	}
	host := "/host/" + childHost.Hostname
	for name, path := range cases {
		t.Run(name, func(t *testing.T) {
			var got [2][]byte
			for i, side := range p.Each() {
				got[i] = dataAsk(t, side.Role, side.Daemon.Addr, []byte("GET "+host+path+" HTTP/1.1\r\n\r\n"))
			}
			if !dataAgree(got[0], got[1]) {
				t.Errorf("responses differ\n%s", firstDifference(got[0], got[1]))
			}
		})
	}
	// The dashboard's POST (D224 F10) with v3-default's query: C reads no payload (api_v2_data.c:20-340), so the
	// oracle answers it as that GET, whatever the body asks (one point grouped by node, the 600 s before now): the
	// query's 6 points of 10 s grouped by dimension.
	t.Run("v3-post", func(t *testing.T) {
		target := host + cases["v3-default"]
		post := rawRequest("POST", target, []string{dataDashboardType}, dataDashboardBody("q.ctx", time.Now().Unix()))
		dataSame(t, p, []byte("GET "+target+" HTTP/1.1\r\n\r\n"), post, [2]string{"HTTP/1.1 200 OK\r\n", "\n}\n"},
			[]string{`"update_every":10,`, `"grouped_by":["dimension"]`}, nil)
	})
	get := func(path string) []byte { return []byte("GET " + host + path + " HTTP/1.1\r\n\r\n") }
	// A context scope with no word in it is no scope (D233: string_to_simple_pattern returns NULL for a text of
	// separators alone, simple_pattern.h:57-59, which the query target reads as no filter, query_scope.c:115): the
	// oracle answers as it does without the parameter, the child's one context queried.
	t.Run("v2-scope-wordless", func(t *testing.T) {
		plain := "/api/v2/data?scope_nodes=" + childHost.Hostname + "&points=6&" + win
		dataSame(t, p, get(plain), get(plain+"&scope_contexts=%7C"), [2]string{"HTTP/1.1 200 OK\r\n", "\n}\n"}, nil,
			dashGuard([]dashFact{dashIs(`"q.ctx"`, "summary", "contexts", "[0]", "id"),
				dashIs(`{"sl":2,"qr":2}`, "summary", "contexts", "[0]", "is")}))
	})
	// The rows below carry what the oracle's answer must be (dataJudge): C's, in H35's probe P2.
	for _, r := range []struct {
		name, path string
		want       [2]string
		guard      func(Value) error
	}{
		// a chart pattern with no word in it is no pattern: every chart of the host answers, q.a's four visible
		// dimensions and q.two's two (web/api/v1/api_v1_data.c:139-148, :175: the request has a chart, and the query
		// target gets no instance filter), where `chart=*` is refused as no chart at all (`star-target`)
		{"chart-wordless", "/api/v1/data?chart=%7C&points=5&" + win, [2]string{"HTTP/1.1 200 OK\r\n", "\n    }"},
			dashGuard([]dashFact{dashIs(`["time","alpha","b","z","inc","a","b"]`, "labels")})},
		// a `labels=` word whose separator is escaped: C parses each word of the pattern again with the web
		// separators (database/pattern-array.c:112), and that second reading ends the value at the escaped comma, so
		// `k:v1\,x` selects the instance labelled k=v1 (q.a: one of the two selected and queried, the other excluded)
		{"v3-labels-escaped", v3 + "&points=4&labels=k:v1%5C,x", [2]string{"HTTP/1.1 200 OK\r\n", "\n}\n"},
			dashGuard([]dashFact{dashIs(`{"sl":1,"ex":1,"qr":1}`, "summary", "nodes", "[0]", "is")})},
		// a chart asked by its id is still held to `chart_label_key`: C checks the key on the single-chart path too
		// (database/contexts/query_target.c:943-972), so a key the chart has no label of leaves nothing to query
		{"chart-label-key-miss", "/api/v1/data?chart=q.a&chart_label_key=nosuchlabel&points=5&" + win,
			[2]string{"HTTP/1.1 404 Not Found\r\n", "\r\n\r\nNo metrics where matched to query."}, nil},
	} {
		t.Run(r.name, func(t *testing.T) {
			var got [2][]byte
			for i, side := range p.Each() {
				got[i] = dataAsk(t, side.Role, side.Daemon.Addr, get(r.path))
			}
			dataJudge(t, got, r.want, nil, r.guard)
		})
	}
}

// dataSame judges a request that C answers as it answers another: a POST whose payload C does not read
// (api_v2_data.c:20-340), a selector with no word in it (D233). The oracle's answer to post must be its answer to get
// asked just before or just after it; then the two answers to post are judged as a data row's (dataJudge). An answer
// whose window ends at now moves with the clock: a round whose answers fall in different seconds is asked again (3
// rounds at most).
func dataSame(t *testing.T, p *Pair, get, post []byte, want [2]string, holds []string, guard func(Value) error) {
	t.Helper()
	var got [2][]byte
	asGet, agree := false, false
	for round := 0; round < 3 && !(asGet && agree); round++ {
		before := dataAsk(t, Oracle, p.Oracle.Addr, get)
		got[0] = dataAsk(t, Oracle, p.Oracle.Addr, post)
		got[1] = dataAsk(t, Candidate, p.Candidate.Addr, post)
		after := dataAsk(t, Oracle, p.Oracle.Addr, get)
		asGet = dataAgree(got[0], before) || dataAgree(got[0], after)
		agree = dataAgree(got[0], got[1])
	}
	if !asGet {
		t.Fatalf("oracle: the request is not answered as the one it should equal: %q", truncateBytes(got[0]))
	}
	dataJudge(t, got, want, holds, guard)
}

// dataAsk sends request to one agent and returns its answer with the data masks (dataAnswer); an answer whose
// Content-Length is not its body's length is reported.
func dataAsk(t *testing.T, side Role, addr string, request []byte) []byte {
	t.Helper()
	b, length, err := dataAnswer(addr, request)
	if err != nil {
		t.Fatalf("%s: %v", side, err)
	}
	if length != nil {
		t.Errorf("%s: %v", side, length)
	}
	return b
}

// dataAnswer sends request to one agent and hands back its answer with the data masks, for the seconds the request
// was in flight (dataMask), and what is wrong with the length its head names (rawLength: the masks hide the header,
// the two sides' timings being as wide as they are).
func dataAnswer(addr string, request []byte) (masked []byte, length, err error) {
	from := time.Now().Unix()
	b, err := rawExchange(addr, request, 2*time.Second)
	if err != nil {
		return nil, nil, err
	}
	return dataMask(b, from, time.Now().Unix()), rawLength(b), nil
}

// labelOrderPaths are where the answers print labels in their order, which in C is heap-address order and differs
// between runs (D22.2): the instances' and group-by labels, the chart labels, v3's summary of label keys and v1's
// label pairs.
var labelOrderPaths = []string{"**.labels", "**.chart_labels", "summary.labels[]", "full_chart_labels[]"}

// labelOrderOnly tells whether two raw answers differ in their label order alone: the same headers, and JSON bodies
// equal but for the order at labelOrderPaths.
func labelOrderOnly(a, b []byte) bool {
	sep := []byte("\r\n\r\n")
	ha, ba, _ := bytes.Cut(a, sep)
	hb, bb, _ := bytes.Cut(b, sep)
	if !bytes.Equal(ha, hb) {
		return false
	}
	va, errA := ParseJSON(ba)
	vb, errB := ParseJSON(bb)
	return errA == nil && errB == nil && len(Compare(va, vb, labelOrderPaths...)) == 0
}

// firstDifference shows both responses around their first differing byte, in the bodies when those differ.
func firstDifference(a, b []byte) string {
	sep := []byte("\r\n\r\n")
	ha, ba, _ := bytes.Cut(a, sep)
	hb, bb, _ := bytes.Cut(b, sep)
	where := "body"
	if bytes.Equal(ba, bb) {
		where, ba, bb = "headers", ha, hb
	}
	i := 0
	for i < len(ba) && i < len(bb) && ba[i] == bb[i] {
		i++
	}
	from := max(0, i-200)
	cut := func(x []byte) string {
		return strings.ReplaceAll(string(x[from:min(len(x), i+200)]), "\r", "\\r")
	}
	return fmt.Sprintf("%s at byte %d\noracle:    %s\ncandidate: %s", where, i, cut(ba), cut(bb))
}

// TestDataGroupingWindows configures `[web] ses max tg_des_window` and `des max tg_des_window` (one of them at 1,
// which keeps the default) and compares the ses/des answers.
func TestDataGroupingWindows(t *testing.T) {
	p := StartPair(t, daemon.Options{
		StreamMemoryMode: "ram",
		StorageTiers:     1,
		WebExtra:         "    ses max tg_des_window = 3\n    des max tg_des_window = 1\n",
	}, parentIdentity)
	base := dashBase()
	dashChild(t, p, base)
	host := "/host/" + childHost.Hostname
	for _, q := range []string{"group=ses&points=60", "group=des&points=60", "group=ses&points=7", "group=ema&points=1"} {
		path := fmt.Sprintf("%s/api/v1/data?chart=q.a&after=%d&before=%d&%s", host, base, base+60, q)
		var got [2][]byte
		for i, side := range p.Each() {
			b, err := rawExchange(side.Daemon.Addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 2*time.Second)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			got[i] = maskTimings(maskRaw(b))
		}
		if !bytes.Equal(got[0], got[1]) {
			t.Errorf("%s: responses differ\n%s", q, firstDifference(got[0], got[1]))
		}
	}
}
