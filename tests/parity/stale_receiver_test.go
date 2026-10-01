// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// staleHost is the stub child of `stream.stale-receiver`.
var staleHost = stream.HostInfo{Hostname: "parity-stale", MachineGUID: "5a1e0000-0000-4000-8000-00000000c501"}

// staleAgeRe is the age a refused connection's record gives the old one.
var staleAgeRe = regexp.MustCompile(`last used \d+ secs ago`)

// staleRequest is the answer to a STREAM handshake of the stub host under a hostname, for a request the parent
// refuses (it answers and closes).
func staleRequest(d *daemon.Daemon, hostname string) []byte {
	req := fmt.Sprintf("STREAM key=%s&hostname=%s&registry_hostname=%s&machine_guid=%s"+
		"&update_every=1&os=linux&timezone=Etc/UTC&abbrev_timezone=UTC&utc_offset=0&hops=1&ver=%d"+
		"&NETDATA_PROTOCOL_VERSION=1.1 HTTP/1.1\r\nUser-Agent: parity/1.0\r\n\r\n",
		d.StreamKey, hostname, hostname, staleHost.MachineGUID, stream.CapsLive)
	b, _ := rawExchange(d.Addr, []byte(req), 5*time.Second)
	return maskRaw(b)
}

// TestStreamStaleReceiver (check `stream.stale-receiver`; review R49 B1): a second connection of a host that already
// streams. While the first sent something within 30 s it is refused (ALREADY); after 30 s quiet, one under another
// hostname is denied, and one under the same hostname stops the stale receiver and is accepted. The stopped receiver
// leaves, as every stopped one, with SIGNALED TO STOP: C forces the stopper's reason only into the host's ingest status.
func TestStreamStaleReceiver(t *testing.T) {
	p := StartPair(t, daemon.Options{StorageTiers: 1, PulseOff: true}, parentIdentity)
	var first [2]*stream.Conn
	var quiet time.Time
	for i, side := range p.Each() {
		c, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, staleHost, stream.CapsLive)
		if err != nil {
			t.Fatalf("%s: the first connection: %v", side.Role, err)
		}
		t.Cleanup(func() { _ = c.Close() })
		c.Linef("CHART 'stale.c1' '' 'title' 'units' 'family' 'stale.c1' line 1000 1 '' stale corpus")
		c.Linef("DIMENSION 'd' '' absolute 1 1 ''")
		c.Linef("BEGIN2 'stale.c1' 1 %d #", time.Now().Unix())
		c.Linef("SET2 'd' 1 1 A")
		c.Linef("END2")
		if err := c.Flush(); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		first[i], quiet = c, time.Now()
	}
	for _, side := range p.Each() {
		if _, b, ok := waitHop(side.Daemon.Addr, staleHost.MachineGUID, true, 20*time.Second); !ok {
			t.Fatalf("%s: the stub is not online: %.300s", side.Role, httpBody(b))
		}
	}
	compare := func(step, want string, got [2][]byte) {
		t.Helper()
		if !strings.Contains(string(got[0]), want) {
			t.Errorf("%s: the oracle answered %q, want %q", step, got[0], want)
		}
		if string(got[0]) != string(got[1]) {
			t.Errorf("%s: answers differ\noracle:    %q\ncandidate: %q", step, got[0], got[1])
		}
	}

	// the first one sent its block within 30 s: refused
	var got [2][]byte
	for i, side := range p.Each() {
		got[i] = staleRequest(side.Daemon, staleHost.Hostname)
	}
	compare("working", stream.RejectAlreadyStreaming, got)

	// 30 s quiet: another hostname is denied, the stale receiver kept
	time.Sleep(time.Until(quiet.Add(32 * time.Second)))
	for i, side := range p.Each() {
		got[i] = staleRequest(side.Daemon, "parity-stale-other")
	}
	compare("another hostname", stream.RejectNotPermitted, got)

	// the same hostname: the stale receiver stopped, the new connection accepted, the old one closed
	var closed [2]bool
	for i, side := range p.Each() {
		c, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, staleHost, stream.CapsLive)
		if err != nil {
			t.Errorf("%s: the connection after 30 s quiet: %v", side.Role, err)
			continue
		}
		t.Cleanup(func() { _ = c.Close() })
		_, err = first[i].ReadLine(time.Now().Add(5 * time.Second))
		closed[i] = err != nil && !strings.Contains(err.Error(), "timeout")
	}
	if !closed[0] || closed[0] != closed[1] {
		t.Errorf("the stale connection closed: oracle %v, candidate %v", closed[0], closed[1])
	}
	time.Sleep(2 * time.Second)

	// the records of the host's connections, as sets
	var recs [2][]string
	for i, side := range p.Each() {
		for _, l := range parentRecords(t, side.Daemon, "STREAM RCV", nil) {
			if !strings.Contains(l, staleHost.Hostname) {
				continue
			}
			l = anyDstPortRe.ReplaceAllString(l, " dst_port=P")
			l = staleAgeRe.ReplaceAllString(l, "last used N secs ago")
			recs[i] = append(recs[i], mlCapableRe.ReplaceAllString(l, "ml_capable=M"))
		}
		slices.Sort(recs[i])
		recs[i] = slices.Compact(recs[i])
	}
	t.Logf("oracle records:\n%s", strings.Join(recs[0], "\n"))
	for _, want := range []string{`reason=\"DISCONNECTED SIGNALED TO STOP\"`, "stopped previous stale receiver"} {
		if !slices.ContainsFunc(recs[0], func(l string) bool { return strings.Contains(l, want) }) {
			t.Errorf("the oracle has no record with %s", want)
		}
	}
	diffLines(t, "stale receiver records", recs[0], recs[1])
}
