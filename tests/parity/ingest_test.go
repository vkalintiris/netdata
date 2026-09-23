// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"reflect"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// infoHosts returns the host members of /api/v1/info.
func infoHosts(t *testing.T, base string) map[string]any {
	t.Helper()
	resp, err := http.Get(base + "/api/v1/info")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	var all map[string]any
	if err := json.Unmarshal(body, &all); err != nil {
		t.Fatalf("%v: %s", err, body)
	}
	out := map[string]any{}
	for _, k := range []string{"hosts-available", "mirrored_hosts", "mirrored_hosts_status"} {
		out[k] = all[k]
	}
	return out
}

// TestIngestHostsListed streams one chart from a fake child into both daemons (ram mode) and compares the host
// members of /api/v1/info while the child is connected and after it disconnects.
func TestIngestHostsListed(t *testing.T) {
	p := StartPair(t, daemon.Options{StreamMemoryMode: "ram"}, parentIdentity)
	var conns []*stream.Conn
	for _, side := range p.Each() {
		conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsLive)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		conns = append(conns, conn)
		now := time.Now().Unix()
		conn.Linef("CHART 'ingest.test' '' 'title' 'units' 'family' 'ingest.test' line 1000 1 '' fixture-pusher corpus")
		conn.Linef("DIMENSION 'd1' '' absolute 1 1 ''")
		conn.Linef("CLABEL 'k' 'v' 2")
		conn.Linef("CLABEL_COMMIT")
		for i := int64(5); i >= 1; i-- {
			conn.Linef("BEGIN2 'ingest.test' 1 %d #", now-i)
			conn.Linef("SET2 'd1' %d %d A", i, i)
			conn.Linef("END2")
		}
		if err := conn.Flush(); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
	}
	time.Sleep(time.Second)
	compare := func(stage string) {
		var got [2]map[string]any
		for i, side := range p.Each() {
			got[i] = infoHosts(t, side.Daemon.BaseURL)
		}
		if !reflect.DeepEqual(got[0], got[1]) {
			t.Errorf("%s: /api/v1/info hosts differ\noracle:    %s\ncandidate: %s", stage, fmt.Sprint(got[0]), fmt.Sprint(got[1]))
		}
	}
	compare("connected")
	for _, c := range conns {
		_ = c.Close()
	}
	time.Sleep(2 * time.Second)
	compare("disconnected")
}
