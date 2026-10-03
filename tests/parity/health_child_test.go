// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/notify"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// healthChild is the health checks' streaming child.
var healthChild = stream.HostInfo{Hostname: "health-child", MachineGUID: "5a1e0000-0000-4000-8000-0000000000c1"}

// healthChildConf is one template on the child's context: the last value of `a`, WARNING above 50.
const healthChildConf = `# the child check's alert on hchild.ctx
template: hch_calc
      on: hchild.ctx
    calc: $a
   every: 1s
    warn: $this > 50
   units: things
    info: the child's last value of a
`

// fanoutChild is one streaming child of both agents at once: a single ticker writes each wall-clock second's block
// (`BEGIN2 <chart> 1 <second>`, a SET2 per dimension, END2) to both connections, so both parents store the same value
// at the same second (a child per side, each on its own ticker, can be a second apart at a boundary).
type fanoutChild struct {
	conns [2]*stream.Conn
	mu    sync.Mutex
	chart string
	dims  []string
	// values are what the next blocks carry; nil until the chart is defined
	values map[string]int64
	stop   chan struct{}
	done   chan struct{}
	err    error
}

// startFanoutChild connects the child to both agents; nothing is defined or collected until define.
func startFanoutChild(t *testing.T, p *Pair, host stream.HostInfo) *fanoutChild {
	t.Helper()
	c := &fanoutChild{stop: make(chan struct{}), done: make(chan struct{})}
	for i, s := range p.Each() {
		conn, err := stream.Connect(s.Daemon.Addr, s.Daemon.StreamKey, host, stream.CapsLive)
		if err != nil {
			t.Fatalf("%s: %s: %v", s.Role, host.Hostname, err)
		}
		c.conns[i] = conn
	}
	go c.run()
	t.Cleanup(c.close)
	return c
}

// run writes one block per whole second, 50 ms into it, to both connections.
func (c *fanoutChild) run() {
	defer close(c.done)
	for {
		next := time.Now().Truncate(time.Second).Add(time.Second + 50*time.Millisecond)
		select {
		case <-c.stop:
			return
		case <-time.After(time.Until(next)):
		}
		c.mu.Lock()
		if c.values != nil {
			sec := next.Unix()
			for _, conn := range c.conns {
				conn.Begin2(c.chart, 1, sec)
				for _, d := range c.dims {
					if v, ok := c.values[d]; ok {
						conn.Set2(d, strconv.FormatInt(v, 10), stream.FlagNotAnomalous)
					}
				}
				conn.End2()
			}
			for _, conn := range c.conns {
				if err := conn.Flush(); err != nil && c.err == nil {
					c.err = err
				}
			}
		}
		c.mu.Unlock()
	}
}

// define sends the chart's definition to both agents and starts collecting `values`, at the middle of a second: the
// first block is the next whole second's on both.
func (c *fanoutChild) define(chart, context string, dims []string, values map[string]int64) {
	healthMidSecond()
	c.mu.Lock()
	defer c.mu.Unlock()
	c.chart, c.dims, c.values = chart, dims, values
	for _, conn := range c.conns {
		conn.Linef("CHART '%s' '' 'title' 'units' 'family' '%s' line 1000 1 '' parity fanout", chart, context)
		for _, d := range dims {
			conn.Linef("DIMENSION '%s' '' absolute 1 1 ''", d)
		}
		_ = conn.Flush()
	}
}

// set switches the values at the middle of a second: both agents get them from the next whole second on.
func (c *fanoutChild) set(values map[string]int64) {
	healthMidSecond()
	c.mu.Lock()
	defer c.mu.Unlock()
	c.values = values
}

// failed is the first write error on either connection.
func (c *fanoutChild) failed() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.err
}

// close stops the ticker and closes both connections (the child disconnects from both agents at once).
func (c *fanoutChild) close() {
	select {
	case <-c.stop:
		return
	default:
	}
	close(c.stop)
	<-c.done
	for _, conn := range c.conns {
		_ = conn.Close()
	}
}

// TestHealthChild (check `health.child`, M9 commit 0, D183): a streaming child of both agents (fanoutChild) whose
// chart a template of the parent matches: the parent evaluates the child's stored data itself. The child's section
// in stream.conf turns its health on (the key's `health enabled by default = no` is the template's) with no postpone
// on connect. The child's value goes 10, 70, 10; at each phase the child's `/api/v1/alarms?all` is compared, then its
// alert log's transitions, the notifier's transcripts (the calls name the child: its hostname and machine GUID), and,
// after the child disconnects, its alert log again (C removes a disconnected child's alerts, stream-receiver.c:
// 1482-1505).
func TestHealthChild(t *testing.T) {
	base := "/host/" + healthChild.Hostname
	runHealthCases(t, map[string]healthCase{
		"basic": {
			conf:   healthChildConf,
			stream: fmt.Sprintf("\n[%s]\n    type = machine\n    health enabled = yes\n    postpone alerts on connect = 0\n", healthChild.MachineGUID),
			play: func(t *testing.T, h *healthPair) {
				// the child's ids count from its own seeds
				var cn [2]*healthNorm
				for i, s := range h.p.Each() {
					cn[i] = newHealthNorm(s.Daemon)
				}
				child := startFanoutChild(t, h.p, healthChild)
				// the child's host exists on both before its chart does; C runs no health on a child that collected
				// nothing yet (database/rrdhost-status.c:176-185), so its alerts are linked once its data comes
				time.Sleep(2 * time.Second)
				child.define("hchild.values", "hchild.ctx", []string{"a"}, map[string]int64{"a": 10})
				h.settle(t, cn, base)
				all := func(i int) string { return h.getAs(cn[i], i, base+"/api/v1/alarms?all") }
				transcript := func(i int) string { return strings.Join(cn[i].calls(t, h.p.Each()[i].Daemon), "\n") }
				log := func(i int) string { return h.transitionsAs(cn[i], i, base+"/api/v1/alarm_log") }
				for k, phase := range []struct {
					value  int64
					status string
				}{{10, "CLEAR"}, {70, "WARNING"}, {10, "CLEAR"}} {
					if k > 0 {
						time.Sleep(healthCalcHold)
						child.set(map[string]int64{"a": phase.value})
					}
					h.compareNow(t, fmt.Sprintf("phase %d: the child's /api/v1/alarms?all", k), all, healthAll(phase.status, "hch_calc"))
				}
				sequence := func(want ...string) func(string) error {
					return func(oracle string) error {
						if got := healthSequence(strings.Split(oracle, "\n"), "hch_calc"); !slices.Equal(got, want) {
							return fmt.Errorf("hch_calc went through %v, want %v", got, want)
						}
						return nil
					}
				}
				h.compareNow(t, "the child's alert log", log, sequence("REMOVED", "UNINITIALIZED", "CLEAR", "WARNING", "CLEAR"))
				h.compareNow(t, "the notifier's calls", transcript, func(oracle string) error {
					for _, w := range []string{
						fmt.Sprintf("call 1: argv[%d]=%s\n", healthArgHost, healthChild.Hostname),
						fmt.Sprintf("call 1: argv[%d]=WARNING\n", notify.ArgStatus),
						fmt.Sprintf("call 1: argv[%d]=%s\n", healthArgGUID, healthChild.MachineGUID),
						fmt.Sprintf("call 2: argv[%d]=CLEAR\n", notify.ArgStatus),
					} {
						if !strings.Contains(oracle, w) {
							return fmt.Errorf("no %q", strings.TrimSpace(w))
						}
					}
					if strings.Contains(oracle, "call 3: ") {
						return fmt.Errorf("more than 2 calls")
					}
					return nil
				})
				if err := child.failed(); err != nil {
					t.Fatalf("harness: the child's connection: %v", err)
				}
				child.close()
				h.compareNow(t, "the child's alert log after it disconnected", log,
					sequence("REMOVED", "UNINITIALIZED", "CLEAR", "WARNING", "CLEAR", "REMOVED"))
			},
		},
	})
}
