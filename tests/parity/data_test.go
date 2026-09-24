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
	// The detailed tree prints now as a collected metric's last entry.
	lastEntryRe = regexp.MustCompile(`"(le|last_entry)":(\d+)`)
)

// maskNowEntries replaces last entries within [from, to] (the seconds the request was in flight) with NOW.
func maskNowEntries(b []byte, from, to int64) []byte {
	return lastEntryRe.ReplaceAllFunc(b, func(m []byte) []byte {
		sub := lastEntryRe.FindSubmatch(m)
		v, err := strconv.ParseInt(string(sub[2]), 10, 64)
		if err == nil && v >= from && v <= to {
			return []byte(`"` + string(sub[1]) + `":"NOW"`)
		}
		return m
	})
}

func maskTimings(b []byte) []byte {
	b = contentLengthRe.ReplaceAll(b, []byte("Content-Length: <masked>"))
	b = v2ClockRe.ReplaceAll(b, []byte(`"$1":"<masked>"`))
	return timingsRe.ReplaceAllFunc(b, func(m []byte) []byte {
		name, _, _ := bytes.Cut(m, []byte(":"))
		// A fresh slice: appending to name would write over the source after the match.
		return append(append([]byte{}, name...), ":<masked>"...)
	})
}

// streamDataFixture sends a child with two charts of one context: q.a (update every 1 s: a with gaps, b with
// negatives, resets and anomalous samples, z always zero, inc incremental, h hidden) and q.two (update every 2 s),
// ending at base+60.
func streamDataFixture(t *testing.T, conn *stream.Conn, base int64) {
	t.Helper()
	conn.Linef("CHART 'q.a' 'q_a_name' 'title a' 'units' 'fam' 'q.ctx' line 1000 1 '' fixture-pusher corpus")
	conn.Linef("DIMENSION 'a' 'alpha' absolute 1 1 ''")
	conn.Linef("DIMENSION 'b' '' absolute 1 1 ''")
	conn.Linef("DIMENSION 'z' '' absolute 1 1 ''")
	conn.Linef("DIMENSION 'inc' '' incremental 1 1 ''")
	conn.Linef("DIMENSION 'h' '' absolute 1 1 'hidden'")
	conn.Linef("CLABEL 'k' 'v1' 2")
	conn.Linef("CLABEL_COMMIT")
	conn.Linef("CHART 'q.two' '' 'title two' 'units' 'fam' 'q.ctx' line 1001 2 '' fixture-pusher corpus")
	conn.Linef("DIMENSION 'a' '' absolute 1 1 ''")
	conn.Linef("DIMENSION 'b' '' absolute 1 1 ''")
	conn.Linef("CLABEL 'k' 'v2' 2")
	conn.Linef("CLABEL_COMMIT")
	for i := int64(1); i <= 60; i++ {
		conn.Linef("BEGIN2 'q.a' 1 %d #", base+i)
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
			conn.Linef("BEGIN2 'q.two' 2 %d #", base+i)
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
	base := time.Now().Unix()/60*60 - 120
	for _, side := range p.Each() {
		conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsLive)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		t.Cleanup(func() { _ = conn.Close() })
		streamDataFixture(t, conn, base)
	}
	time.Sleep(2500 * time.Millisecond)

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
	// v2/v3 walk every host: scope them to the child, the parent's own charts differ by design.
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
				from := time.Now().Unix()
				b, err := rawExchange(side.Daemon.Addr, []byte("GET "+host+path+" HTTP/1.1\r\n\r\n"), 2*time.Second)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				got[i] = maskNowEntries(maskTimings(maskRaw(b)), from, time.Now().Unix())
			}
			if !bytes.Equal(got[0], got[1]) {
				t.Errorf("responses differ\n%s", firstDifference(got[0], got[1]))
			}
		})
	}
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
