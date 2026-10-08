// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"reflect"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// maskNow replaces last_time_t values within [from, to] (the seconds the request was in flight) with NOW: a collected
// object reports the request time there (maskNowKeys).
func maskNow(b []byte, from, to int64) []byte { return maskNowKeys(b, from, to, "last_time_t") }

// labelsOnly re-encodes a JSON body as parsed data: C prints labels in heap-address order, so requests showing
// labels compare as values, not bytes.
func labelsOnly(t *testing.T, b []byte) any {
	t.Helper()
	i := bytes.Index(b, []byte("\r\n\r\n"))
	if i < 0 {
		t.Fatalf("no body: %q", b)
	}
	var v any
	if err := json.Unmarshal(b[i+4:], &v); err != nil {
		t.Fatalf("%v: %q", err, b[i+4:])
	}
	return v
}

// TestContextsAPI streams charts from a fake child into both daemons (ram mode) and compares /api/v1/contexts,
// /api/v1/context and /api/v3/context for that child while it is connected and after it disconnects; while it is
// connected, also the filters with no word in them, which the oracle must answer as it does without them (D233).
func TestContextsAPI(t *testing.T) {
	p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1}, parentIdentity)
	var conns []*stream.Conn
	for _, side := range p.Each() {
		conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsLive)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		conns = append(conns, conn)
		now := time.Now().Unix()
		conn.Linef("CHART 'ingest.test' '' 'title one' 'units' 'family' 'ingest.test' line 1000 1 '' fixture-pusher corpus")
		conn.Linef("DIMENSION 'd1' '' absolute 1 1 ''")
		conn.Linef("CLABEL 'k' 'v' 2")
		conn.Linef("CLABEL_COMMIT")
		conn.Linef("CHART 'ingest.second' 'second_name' 'title two' 'units' 'family' 'ingest.test' line 900 1 '' fixture-pusher corpus")
		conn.Linef("DIMENSION 'd1' '' absolute 1 1 ''")
		conn.Linef("DIMENSION 'd2' 'dim two' absolute 1 1 ''")
		conn.Linef("CHART 'ingest.hidden' '' 'hidden' 'units' 'family' 'ingest.hidden' line 1000 1 'hidden' fixture-pusher corpus")
		conn.Linef("DIMENSION 'h' '' absolute 1 1 ''")
		for i := int64(5); i >= 1; i-- {
			for _, c := range []struct{ id, dims string }{{"ingest.test", "d1"}, {"ingest.second", "d1 d2"}, {"ingest.hidden", "h"}} {
				conn.Linef("BEGIN2 '%s' 1 %d #", c.id, now-i)
				for _, d := range bytes.Fields([]byte(c.dims)) {
					conn.Linef("SET2 '%s' %d %d A", d, i, i)
				}
				conn.Linef("END2")
			}
		}
		if err := conn.Flush(); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
	}
	// Two worker ticks: a tick between a chart's definition and its first store delays its context by one.
	time.Sleep(2500 * time.Millisecond)

	host := "/host/" + childHost.Hostname
	bytesCases := map[string]string{
		"contexts":             "/api/v1/contexts",
		"contexts-full":        "/api/v1/contexts?options=charts,dimensions,flags,deleted,hidden",
		"contexts-key":         "/api/v1/contexts?options=charts&chart_label_key=k",
		"contexts-filter":      "/api/v1/contexts?options=charts&chart_labels_filter=k:v",
		"contexts-filter-miss": "/api/v1/contexts?options=charts&chart_labels_filter=k:x",
		"contexts-dims":        "/api/v1/contexts?options=dimensions&dimensions=d2",
		"contexts-dim-name":    "/api/v1/contexts?dims=dim%20two",
		"contexts-after":       "/api/v1/contexts?after=-3600&before=-1",
		"context":              "/api/v1/context?context=ingest.test&options=charts,dimensions,flags,deleted",
		"context-hidden":       "/api/v1/context?ctx=ingest.hidden",
		"context-missing":      "/api/v1/context",
		"context-unknown":      "/api/v1/context?context=nope",
		"v3-context":           "/api/v3/context?context=ingest.test&options=instances",
	}
	valueCases := map[string]string{
		"contexts-labels": "/api/v1/contexts?options=labels,charts",
		"context-labels":  "/api/v1/context?context=ingest.test&options=labels",
	}
	// A filter with no word in it is no filter (D233; R100's 2.1): C's simple_pattern_create returns NULL for a text
	// of separators alone (web/api/v1/api_v1_contexts.c:40-49, api_v1_context.c:47-56) and the renderer tests the
	// pointer (database/contexts/api_v1_contexts.c:70-73, :123, :136-146), so the oracle answers each as it does
	// without the parameter (the second target), where a filter that matches nothing lists no context
	// (`contexts-filter-miss`) and answers 404 for one.
	sameCases := map[string][2]string{
		"contexts-key-wordless":  {"/api/v1/contexts?chart_label_key=%7C", "/api/v1/contexts"},
		"contexts-dims-wordless": {"/api/v1/contexts?dimensions=,", "/api/v1/contexts"},
		"context-key-wordless": {"/api/v1/context?context=ingest.test&chart_label_key=%7C",
			"/api/v1/context?context=ingest.test"},
	}
	compare := func(stage string, extra map[string]string) {
		cases := map[string]string{}
		for k, v := range bytesCases {
			cases[k] = v
		}
		for k, v := range extra {
			cases[k] = v
		}
		for name, path := range cases {
			t.Run(stage+"/"+name, func(t *testing.T) {
				var got [2][]byte
				for i, side := range p.Each() {
					from := time.Now().Unix()
					b, err := rawExchange(side.Daemon.Addr, []byte("GET "+host+path+" HTTP/1.1\r\n\r\n"), 2*time.Second)
					if err != nil {
						t.Fatalf("%s: %v", side.Role, err)
					}
					got[i] = maskNow(maskRaw(b), from, time.Now().Unix())
				}
				if !bytes.Equal(got[0], got[1]) {
					t.Errorf("responses differ\noracle:    %s\ncandidate: %s", got[0], got[1])
				}
			})
		}
		for name, path := range valueCases {
			t.Run(stage+"/"+name, func(t *testing.T) {
				var got [2]any
				for i, side := range p.Each() {
					from := time.Now().Unix()
					b, err := rawExchange(side.Daemon.Addr, []byte("GET "+host+path+" HTTP/1.1\r\n\r\n"), 2*time.Second)
					if err != nil {
						t.Fatalf("%s: %v", side.Role, err)
					}
					got[i] = labelsOnly(t, maskNow(maskRaw(b), from, time.Now().Unix()))
				}
				if !reflect.DeepEqual(got[0], got[1]) {
					t.Errorf("responses differ\noracle:    %v\ncandidate: %v", got[0], got[1])
				}
			})
		}
	}
	ask := func(t *testing.T, side Role, addr, path string) []byte {
		t.Helper()
		from := time.Now().Unix()
		b, err := rawExchange(addr, []byte("GET "+host+path+" HTTP/1.1\r\n\r\n"), 2*time.Second)
		if err != nil {
			t.Fatalf("%s: %v", side, err)
		}
		return maskNow(maskAnswer(b, [2]int64{from, time.Now().Unix()}), from, time.Now().Unix())
	}
	same := func(stage string) {
		for name, paths := range sameCases {
			t.Run(stage+"/"+name, func(t *testing.T) {
				// the oracle's two answers fall in one second, or are asked again (3 rounds at most)
				var with, without []byte
				for round := 0; round < 3 && (with == nil || !bytes.Equal(with, without)); round++ {
					with = ask(t, Oracle, p.Oracle.Addr, paths[0])
					without = ask(t, Oracle, p.Oracle.Addr, paths[1])
				}
				if !bytes.HasPrefix(with, []byte("HTTP/1.1 200 OK\r\n")) || !bytes.Contains(with, []byte(`"title":`)) ||
					!bytes.Equal(with, without) {
					t.Fatalf("oracle: %s is not answered as %s, a 200 with a context:\n%s\n%s", paths[0], paths[1], with,
						without)
				}
				if got := ask(t, Candidate, p.Candidate.Addr, paths[0]); !bytes.Equal(with, got) {
					t.Errorf("responses differ\noracle:    %s\ncandidate: %s", with, got)
				}
			})
		}
	}
	compare("connected", nil)
	same("connected")
	for _, c := range conns {
		_ = c.Close()
	}
	time.Sleep(3 * time.Second)
	compare("disconnected", map[string]string{
		"contexts-rfc3339": "/api/v1/contexts?options=charts,rfc3339",
		"contexts-deep":    "/api/v1/contexts?options=deepscan,flags",
	})
}
