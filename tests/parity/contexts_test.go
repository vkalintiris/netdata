// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"reflect"
	"regexp"
	"strconv"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// lastTimeRe finds last_time_t members; a collected object reports the request time there.
var lastTimeRe = regexp.MustCompile(`"last_time_t":\s*(\d+)`)

// maskNow replaces last_time_t values within [from, to] (the seconds the request was in flight) with NOW.
func maskNow(b []byte, from, to int64) []byte {
	return lastTimeRe.ReplaceAllFunc(b, func(m []byte) []byte {
		v, err := strconv.ParseInt(string(lastTimeRe.FindSubmatch(m)[1]), 10, 64)
		if err == nil && v >= from && v <= to {
			return []byte(`"last_time_t":"NOW"`)
		}
		return m
	})
}

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
// /api/v1/context and /api/v3/context for that child while it is connected and after it disconnects.
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
	compare("connected", nil)
	for _, c := range conns {
		_ = c.Close()
	}
	time.Sleep(3 * time.Second)
	compare("disconnected", map[string]string{
		"contexts-rfc3339": "/api/v1/contexts?options=charts,rfc3339",
		"contexts-deep":    "/api/v1/contexts?options=deepscan,flags",
	})
}
