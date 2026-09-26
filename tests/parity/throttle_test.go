// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bufio"
	"net"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// TestWebListenerThrottle gives the one web thread a small share of sockets: once its connections reach it, the thread
// stops accepting ("DISABLING listening sockets"), and a connection that closes lets the waiting one in ("ENABLING").
// Each daemon gets the same sequence: connections that keep their answered keep-alive request open, until one gets no
// answer; then one answered connection closes and the waiting one must be answered, then the rest close.
func TestWebListenerThrottle(t *testing.T) {
	extra := "    web server threads = 1\n    web server max sockets = 5\n"
	p := StartPair(t, daemon.Options{WebDir: oracleWebDir(t), WebExtra: extra, LogsExtra: "    level = debug\n",
		StreamMemoryMode: "ram", StorageTiers: 1}, parentIdentity)
	type outcome struct {
		answered   int
		waitedThen bool
	}
	var got [2]outcome
	for i, side := range p.Each() {
		var open []net.Conn
		var waiting net.Conn
		var waitingReader *bufio.Reader
		for n := 0; n < 8; n++ {
			c, err := net.Dial("tcp", side.Daemon.Addr)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			if _, err := c.Write([]byte("GET /index.html HTTP/1.1\r\nHost: x\r\nConnection: keep-alive\r\n\r\n")); err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			r := bufio.NewReader(c)
			_ = c.SetReadDeadline(time.Now().Add(2 * time.Second))
			line, err := r.ReadString('\n')
			if err != nil {
				waiting, waitingReader = c, r
				break
			}
			if !strings.HasPrefix(line, "HTTP/1.1 200") {
				t.Fatalf("%s: %q", side.Role, line)
			}
			got[i].answered++
			open = append(open, c)
		}
		// one slot frees: the waiting connection is accepted, which fills the share again
		if waiting != nil && len(open) > 0 {
			open[0].Close()
			open = open[1:]
			_ = waiting.SetReadDeadline(time.Now().Add(5 * time.Second))
			line, err := waitingReader.ReadString('\n')
			got[i].waitedThen = err == nil && strings.HasPrefix(line, "HTTP/1.1 200")
			open = append(open, waiting)
		}
		for _, c := range open {
			c.Close()
		}
	}
	if got[0] != got[1] {
		t.Errorf("oracle %+v, candidate %+v", got[0], got[1])
	}
	if got[0].answered == 8 || !got[0].waitedThen {
		t.Errorf("the throttle was not reached: %+v", got[0])
	}
	for _, side := range p.Each() {
		if err := side.Daemon.Stop(); err != nil {
			t.Fatalf("parity: stop %s: %v", side.Role, err)
		}
	}
	var records [2][]string
	for i, side := range p.Each() {
		for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
			if strings.Contains(l, "listening sockets (used TCP sockets") {
				records[i] = append(records[i], normalizeLog(l, side.Daemon.Opts.RunDir, ""))
			}
		}
	}
	if strings.Join(records[0], "\n") != strings.Join(records[1], "\n") {
		t.Errorf("records differ\noracle:\n  %s\ncandidate:\n  %s", strings.Join(records[0], "\n  "),
			strings.Join(records[1], "\n  "))
	}
	if len(records[0]) == 0 {
		t.Errorf("no ENABLING/DISABLING records")
	}
}
