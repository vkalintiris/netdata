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

// healthChildBase is the prefix of the child's endpoints on a parent.
var healthChildBase = "/host/" + healthChild.Hostname

// The child's chart: one dimension on a context of its own.
const (
	healthChildChart   = "hchild.values"
	healthChildContext = "hchild.ctx"
)

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

// healthChildSection is the child's section of the parents' stream.conf (its machine GUID's: it wins over the API
// key's, whose `health enabled by default = no` is the template's; stream-conf.c:447-455), with the lines given. A
// line that turns the child's health on passes the rail on stream.conf only because the health runner names the
// recording stub as the notifier (daemon.validateStreamHealth).
func healthChildSection(lines ...string) string {
	return fmt.Sprintf("\n[%s]\n    type = machine\n    %s\n", healthChild.MachineGUID, strings.Join(lines, "\n    "))
}

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
	// since is the second of the first block that carried the current values; 0 until one did
	since int64
	stop  chan struct{}
	done  chan struct{}
	err   error
	// halted: the ticker was stopped; closed: the connections were closed
	halted sync.Once
	closed bool
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
			if c.since == 0 {
				c.since = sec
			}
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
// first block is the next whole second's on both. It returns that second.
func (c *fanoutChild) define(chart, context string, dims []string, values map[string]int64) int64 {
	first := healthMidSecondOn(0)
	c.mu.Lock()
	defer c.mu.Unlock()
	c.chart, c.dims, c.values, c.since = chart, dims, values, 0
	for _, conn := range c.conns {
		conn.Linef("CHART '%s' '' 'title' 'units' 'family' '%s' line 1000 1 '' parity fanout", chart, context)
		for _, d := range dims {
			conn.Linef("DIMENSION '%s' '' absolute 1 1 ''", d)
		}
		_ = conn.Flush()
	}
	return first
}

// set switches the values at the middle of a second: both agents get them from the next whole second on.
func (c *fanoutChild) set(values map[string]int64) {
	healthMidSecond()
	c.setNow(values)
}

// setNow switches the values where the caller stands: from the next block on. A caller at the middle of a second
// (healthPair.atRelease) has them start at the next whole second.
func (c *fanoutChild) setNow(values map[string]int64) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.values, c.since = values, 0
}

// switched waits for the first block of the current values and returns its second (0: no block within 3 s).
func (c *fanoutChild) switched() int64 {
	for end := time.Now().Add(3 * time.Second); ; time.Sleep(20 * time.Millisecond) {
		c.mu.Lock()
		since := c.since
		c.mu.Unlock()
		if since != 0 || time.Now().After(end) {
			return since
		}
	}
}

// failed is the first write error on either connection.
func (c *fanoutChild) failed() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.err
}

// close stops the ticker and closes both connections (the child disconnects from both agents at once).
func (c *fanoutChild) close() {
	c.halted.Do(func() {
		close(c.stop)
		<-c.done
	})
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return
	}
	c.closed = true
	for _, conn := range c.conns {
		_ = conn.Close()
	}
}

// halt stops the child's blocks and leaves both connections open, after a last check that every block was written:
// what a parent that stops finds is a child still connected.
func (c *fanoutChild) halt(t *testing.T) {
	t.Helper()
	if err := c.failed(); err != nil {
		t.Fatalf("harness: the child's connection: %v", err)
	}
	c.halted.Do(func() {
		close(c.stop)
		<-c.done
	})
}

// disconnect is close as a case's step: the child leaves both agents at once, after a last check that every block
// was written.
func (c *fanoutChild) disconnect(t *testing.T) {
	t.Helper()
	if err := c.failed(); err != nil {
		t.Fatalf("harness: the child's connection: %v", err)
	}
	c.close()
}

// healthChildViews are a case's views of the child: each side has a normalizer of the child's own (its ids count
// from its own seeds).
type healthChildViews struct {
	t  *testing.T
	h  *healthPair
	cn [2]*healthNorm
}

func (h *healthPair) childViews(t *testing.T) *healthChildViews {
	v := &healthChildViews{t: t, h: h}
	for i, s := range h.p.Each() {
		v.cn[i] = newHealthNorm(s.Daemon, healthChildBase)
	}
	return v
}

// all is the child's `/api/v1/alarms?all`; changes its alert log as transitions; log its whole alert log (healthPair.
// logOf); flags its alerts' flags; calls the notifier's transcript of the child's notifications (healthNorm.callsOf:
// the calls that name its machine GUID), a first status's duration as healthCallsSinceLink prints it.
func (v *healthChildViews) all(i int) string {
	return v.h.getAs(v.cn[i], i, healthChildBase+"/api/v1/alarms?all")
}
func (v *healthChildViews) changes(i int) string {
	return v.h.transitionsAs(v.cn[i], i, healthChildBase+"/api/v1/alarm_log")
}
func (v *healthChildViews) log(i int) string {
	return v.h.logOf(v.cn[i], i, healthChildBase+"/api/v1/alarm_log")
}
func (v *healthChildViews) flags(i int) string { return v.h.flagsOf(v.cn[i], i, healthChildBase) }
func (v *healthChildViews) calls(i int) string {
	return healthCallsSinceLink(strings.Join(v.cn[i].callsOf(v.t, v.h.p.Each()[i].Daemon, healthChild.MachineGUID), "\n"))
}

// statuses is the child's `/api/v1/alarms?all` as each alert's status alone (`name: STATUS`, in the answer's order),
// after the answer's status line and the host's `status` member: a view without an id, for a moment at which the
// child's alert log has stored no entry yet (the ids' bases are not known).
func (v *healthChildViews) statuses(i int) string {
	r := healthGet(v.h.p.Each()[i].Daemon, healthChildBase+"/api/v1/alarms?all")
	out := []string{healthView(r, ""), "status " + strconv.FormatBool(strings.Contains(string(r.Body), "\n\t\"status\": true,"))}
	for _, m := range healthAlarmStatusRe.FindAllStringSubmatch(string(r.Body), -1) {
		out = append(out, m[1]+": "+m[2])
	}
	return strings.Join(out, "\n")
}

// processed is healthPair.processed for the child's alert log.
func (v *healthChildViews) processed(what, name, status string) {
	v.t.Helper()
	v.h.processedAs(v.t, v.cn, healthChildBase, what, name, status)
}

// settle waits for the child's alert log (healthPair.settle) and bounds its ids' seeds to the chart's first second.
func (v *healthChildViews) settle(first int64) {
	v.t.Helper()
	v.h.settle(v.t, v.cn, healthChildBase)
	v.h.bases(v.t, v.cn, first)
}

// healthHostOff is a guard on a view of a host's `/api/v1/alarms?all`: the host's health is off and no alert is
// listed (health_json.c:278-296: a child that disconnected, or one whose section turns its health off).
func healthHostOff(view string) error {
	if err := healthWant(map[string]string{})(view); err != nil {
		return err
	}
	if !strings.Contains(view, "\n\t\"status\": false,") || !strings.Contains(view, "\"alarms\": {\n\n\t}") {
		return fmt.Errorf("the host's health is not off, or its alarms are not empty")
	}
	return nil
}

// healthHostOn is a guard on a view of a host's `/api/v1/alarms?all`: the host's health runs.
func healthHostOn(view string) error {
	if !strings.Contains(view, "\n\t\"status\": true,") {
		return fmt.Errorf("the host's health is not on")
	}
	return nil
}

// healthSequenceIs is a guard on a view of an alert log's transitions: the alert went through the statuses.
func healthSequenceIs(name string, want ...string) func(string) error {
	return func(view string) error {
		if got := healthSequence(strings.Split(view, "\n"), name); !slices.Equal(got, want) {
			return fmt.Errorf("%s went through %v, want %v", name, got, want)
		}
		return nil
	}
}

// healthHostCalls is a guard on the transcript of one host's notifications: one call per status, in turn, each for
// the alert and naming the host (its hostname in argument 2, its machine GUID in argument 28), and no other call.
func healthHostCalls(hostname, guid, name string, statuses ...string) func(string) error {
	return func(transcript string) error {
		calls := healthCallsOf(transcript)
		if len(calls) != len(statuses) {
			return fmt.Errorf("%d calls, want %d (%v)", len(calls), len(statuses), statuses)
		}
		for k, c := range calls {
			if err := c.want(map[int]string{healthArgHost: hostname, notify.ArgName: name, notify.ArgStatus: statuses[k], healthArgGUID: guid}); err != nil {
				return fmt.Errorf("call %d: %w", k+1, err)
			}
		}
		return nil
	}
}

// healthChildCalls is healthHostCalls for the child's alert.
func healthChildCalls(name string, statuses ...string) func(string) error {
	return healthHostCalls(healthChild.Hostname, healthChild.MachineGUID, name, statuses...)
}

// The records a postponed child leaves at debug level: the receiver's when the child connects
// (stream-receiver.c:1419-1428; written on the web thread that took the child's request, with the request's fields)
// and HEALTH's at the first pass past the delay (health_event_loop.c:420-422).
const (
	healthRecPostponing = "]: Postponing health checks for "
	healthRecResuming   = "]: Resuming health checks after delay."
)

// healthChildPostpone is how long the `postpone` case's child is postponed. It is longer than a first store: the
// child's alert is linked at its first pass, which comes with its first data (three seconds after it connected), and
// the link's entry is stored by the metadata thread's own round, up to seven seconds later
// (healthStoreBound); HEALTH passes the host by until then (health_event_loop.c:396-401). A delay that ends before
// that store would leave the first evaluation at each side's round; this one ends after it on both.
const healthChildPostpone = 15

// TestHealthChild (check `health.child`, M9 commit 0, D183; the cases but `basic`'s first half: M9 commit 9, D214): a
// streaming child of both agents (fanoutChild) whose chart a template of the parent matches: the parent evaluates the
// child's stored data itself. The child's section in stream.conf sets its health (the key's `health enabled by
// default = no` is the template's). Cases:
//   - `basic`: health on, no postpone on connect. The child's value goes 10, 70, 10; at each phase the child's
//     `/api/v1/alarms?all` is compared, then its alert log's transitions and the notifier's transcript (the calls name
//     the child: its hostname and machine GUID). Then the child disconnects: C removes its alerts, each with a REMOVED
//     entry, turns the host's health off (stream-receiver.c:1482-1505, rrdcalc.c:772-797) and notifies nothing (a
//     status below CLEAR never is, health_notifications.c:392). It connects again, at 70 then 10: the alert is linked
//     again with the id of the first connection, and its statuses are notified; and it disconnects a second time;
//   - `auto`: `health enabled = auto` is `yes` to a parent (stream-receiver-connection.c:179): `basic` up to its
//     first disconnect;
//   - `off`: `health enabled = no`: the child's host has no health, no alert, an empty alert log, and no call;
//   - `postpone`: `postpone alerts on connect`: the alert is linked at the child's first pass and not evaluated before
//     the delay ended (stream-receiver.c:1419-1428, health_event_loop.c:416-425), with the two debug records;
//   - `two-hosts`: an alert on localhost's own chart and one on the child's, raised in one second: a call for each,
//     naming its host; then the silencers' `hosts=` selector takes the child's alert alone; the alerts rows of two
//     hosts and of a node selector at the first 70, the transitions of the two hosts at the end (D234 F3);
//   - `half-off`: localhost's alarm beside a child whose health is off: the alerts rows of a host whose health never
//     ran beside one with alerts;
//   - `dyncfg`: with the child connected, a rule on the child's context is added through localhost's DynCfg template,
//     then its job disabled: C applies a DynCfg change to every host whose health ran (health_prototypes.c:681-702,
//     health_dyncfg.c:608-628);
//   - `restart`: both parents stop with the child connected and its alert raised, and start again on their own
//     directories; the child connects again: what the parent makes of the child's alert log it finds.
func TestHealthChild(t *testing.T) {
	runHealthCases(t, map[string]healthCase{
		"basic":     healthChildBasicCase("yes", true),
		"auto":      healthChildBasicCase("auto", false),
		"off":       healthChildOffCase(),
		"postpone":  healthChildPostponeCase(),
		"two-hosts": healthChildTwoHostsCase(),
		"half-off":  healthChildHalfOffCase(),
		"dyncfg":    healthChildDynCfgCase(),
		"restart":   healthChildRestartCase(),
	})
}

// healthChildBasicCase is `basic` (the whole case: `again`) and `auto` (up to the first disconnect), with the
// section's `health enabled` as given.
func healthChildBasicCase(enabled string, again bool) healthCase {
	const name = "hch_calc"
	first := []string{"REMOVED", "UNINITIALIZED", "CLEAR", "WARNING", "CLEAR"}
	return healthCase{
		conf:   healthChildConf,
		stream: healthChildSection("health enabled = "+enabled, "postpone alerts on connect = 0"),
		play: func(t *testing.T, h *healthPair) {
			v := h.childViews(t)
			child := startFanoutChild(t, h.p, healthChild)
			// the child's host exists on both before its chart does; C runs no health on a child that collected
			// nothing yet (it counts as replicating, database/rrdhost-status.c:176-185), so its alerts are linked
			// once its data comes
			time.Sleep(2 * time.Second)
			v.settle(child.define(healthChildChart, healthChildContext, []string{"a"}, map[string]int64{"a": 10}))
			var ids map[string]string
			for k, phase := range []struct {
				value  int64
				status string
			}{{10, "CLEAR"}, {70, "WARNING"}, {10, "CLEAR"}} {
				if k > 0 {
					time.Sleep(healthCalcHold)
					child.set(map[string]int64{"a": phase.value})
				}
				ids = healthAlarmIDs(h.compareNow(t, fmt.Sprintf("phase %d: the child's /api/v1/alarms?all", k), v.all,
					healthBoth(healthAll(phase.status, name), healthHostOn)))
			}
			h.compareNow(t, "the child's alert log", v.changes, healthSequenceIs(name, first...))
			// two calls, whenever they are read: for WARNING and for the return to CLEAR
			v.processed("phase 2", name, "CLEAR")
			h.compareNow(t, "the notifier's calls", v.calls, healthChildCalls(name, "WARNING", "CLEAR"))

			// the child leaves: its alert is removed, its host's health is off, and nothing is notified
			time.Sleep(healthUnlinkHold)
			child.disconnect(t)
			h.compareNow(t, "the child's alert log after it disconnected", v.changes, healthSequenceIs(name, append(first, "REMOVED")...))
			h.compareNow(t, "the child's /api/v1/alarms?all after it disconnected", v.all, healthHostOff)
			h.compareNow(t, "the notifier's calls after the child disconnected", v.calls, healthChildCalls(name, "WARNING", "CLEAR"))
			if !again {
				return
			}

			// it comes back at 70: the alert is linked again with its id and goes to WARNING, which is notified (the
			// status it was last notified for is CLEAR)
			child = startFanoutChild(t, h.p, healthChild)
			time.Sleep(2 * time.Second)
			child.define(healthChildChart, healthChildContext, []string{"a"}, map[string]int64{"a": 70})
			h.compareNow(t, "connected again: the child's /api/v1/alarms?all", v.all,
				healthBoth(healthAll("WARNING", name), healthHostOn, healthIDsAre(ids)))
			v.processed("connected again", name, "WARNING")
			h.compareNow(t, "connected again: the notifier's calls", v.calls, healthChildCalls(name, "WARNING", "CLEAR", "WARNING"))
			time.Sleep(healthCalcHold)
			child.set(map[string]int64{"a": 10})
			h.compareNow(t, "connected again, at 10: the child's /api/v1/alarms?all", v.all, healthBoth(healthAll("CLEAR", name), healthIDsAre(ids)))
			v.processed("connected again, at 10", name, "CLEAR")
			second := append(slices.Clone(first), "REMOVED", "UNINITIALIZED", "WARNING", "CLEAR")
			h.compareNow(t, "connected again: the child's alert log", v.changes, healthSequenceIs(name, second...))
			h.compareNow(t, "connected again: the notifier's calls at 10", v.calls, healthChildCalls(name, "WARNING", "CLEAR", "WARNING", "CLEAR"))
			// the whole log: the ids, the event ids and the links between the entries of the two connections
			h.compareNow(t, "connected again: the child's /api/v1/alarm_log", v.log, healthLogEntries(len(second)-1))

			// and leaves a second time
			time.Sleep(healthUnlinkHold)
			child.disconnect(t)
			h.compareNow(t, "the child's alert log after it disconnected again", v.changes, healthSequenceIs(name, append(second, "REMOVED")...))
			h.compareNow(t, "the child's /api/v1/alarms?all after it disconnected again", v.all, healthHostOff)
			h.compareNow(t, "the notifier's calls after the child disconnected again", v.calls,
				healthChildCalls(name, "WARNING", "CLEAR", "WARNING", "CLEAR"))
		},
	}
}

// healthChildOffCase is `off`: the child's section turns its health off. Its chart is collected at a value the
// template would raise; the parent runs no health on the host, before and after it disconnected.
func healthChildOffCase() healthCase {
	return healthCase{
		conf:   healthChildConf,
		stream: healthChildSection("health enabled = no", "postpone alerts on connect = 0"),
		play: func(t *testing.T, h *healthPair) {
			v := h.childViews(t)
			child := startFanoutChild(t, h.p, healthChild)
			time.Sleep(2 * time.Second)
			child.define(healthChildChart, healthChildContext, []string{"a"}, map[string]int64{"a": 70})
			// the chart is there, and no alert of it
			chart := func(i int) string {
				return h.plainMember(i, healthChildBase+"/api/v1/chart?chart="+healthChildChart, "alarms")
			}
			log := func(i int) string { return h.getAs(v.cn[i], i, healthChildBase+"/api/v1/alarm_log") }
			h.compareNow(t, "the alerts of the child's chart", chart, healthIsText("HTTP 200, application/json; charset=utf-8\n{}"))
			// two passes of a health that ran would have raised the alert by now
			time.Sleep(healthCalcHold)
			for _, at := range []string{"connected", "disconnected"} {
				if at == "disconnected" {
					child.disconnect(t)
					time.Sleep(healthCalcHold)
				}
				h.compareNow(t, at+": the child's /api/v1/alarms?all", v.all, healthHostOff)
				h.compareNow(t, at+": the child's /api/v1/alarm_log", log, healthIs(healthLogEmpty))
				h.compareNow(t, at+": the notifier's calls", v.calls, healthCallsWant(0))
			}
		},
	}
}

// healthChildPostponeCase is `postpone`: the child's section postpones its health checks for healthChildPostpone
// seconds after each connection. Three seconds after its first data the alert is linked and has no status; it gets
// one when the delay ended, not before. Both agents then stop, and the two records are compared: the receiver's,
// with the delay, and HEALTH's, once.
func healthChildPostponeCase() healthCase {
	const name = "hch_calc"
	return healthCase{
		conf:   healthChildConf,
		stream: healthChildSection("health enabled = yes", fmt.Sprintf("postpone alerts on connect = %d", healthChildPostpone)),
		logs:   healthLogsDebug,
		play: func(t *testing.T, h *healthPair) {
			v := h.childViews(t)
			connected := time.Now()
			child := startFanoutChild(t, h.p, healthChild)
			time.Sleep(2 * time.Second)
			first := child.define(healthChildChart, healthChildContext, []string{"a"}, map[string]int64{"a": 10})
			// three seconds of data: the alert is linked, and postponed
			time.Sleep(time.Until(time.Unix(first+3, 0)))
			h.compareNow(t, "postponed: the child's alerts", v.statuses, func(view string) error {
				if !strings.HasSuffix(view, "\nstatus true\n"+name+": UNINITIALIZED") {
					return fmt.Errorf("the child's one alert is not UNINITIALIZED on a host whose health is on")
				}
				if since := time.Since(connected); since > (healthChildPostpone-1)*time.Second {
					return fmt.Errorf("harness: read %v after the child connected: the delay may have ended", since)
				}
				return nil
			})
			v.settle(first)
			// past the delay
			h.compareNow(t, "after the delay: the child's /api/v1/alarms?all", v.all, healthAll("CLEAR", name))
			h.compareNow(t, "after the delay: the child's alert log", v.changes, healthSequenceIs(name, "REMOVED", "UNINITIALIZED", "CLEAR"))
			// the first status is the delay's end, not the first pass's
			entries, err := h.entriesAs(v.cn[0], 0, healthChildBase+"/api/v1/alarm_log")
			if err != nil {
				t.Fatalf("oracle: %v", err)
			}
			if k := slices.IndexFunc(entries, func(e healthEntry) bool { return e.Status == "CLEAR" }); k < 0 ||
				entries[k].When < connected.Unix()+healthChildPostpone-1 {
				t.Fatalf("oracle: the alert's first status is not %d s after the child connected (%d): %+v", healthChildPostpone, connected.Unix(), entries)
			}
			h.compareNow(t, "after the delay: the notifier's calls", v.calls, healthCallsWant(0))
			time.Sleep(healthUnlinkHold)
			child.disconnect(t)
			h.compareNow(t, "the child's alert log after it disconnected", v.changes, healthSequenceIs(name, "REMOVED", "UNINITIALIZED", "CLEAR", "REMOVED"))
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareLines(t, "the records of the postponement", func(i int) []string {
				d := h.p.Each()[i].Daemon
				var out []string
				for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
					if strings.Contains(l, healthRecPostponing) || strings.Contains(l, healthRecResuming) {
						out = append(out, healthConnRe.ReplaceAllString(normalizeLog(l, d.Opts.RunDir, ""), " conn=N"))
					}
				}
				return out
			}, healthRecordsCount(2, map[string]int{
				// the receiver's, on the web thread that took the child's request, with the request's fields
				fmt.Sprintf("level=debug … thread=WEB[n] … request=\"key=K&hostname=%[1]s& … msg=\"STREAM RCV '%[1]s' [from [localhost]:P]: "+
					"Postponing health checks for %[2]d seconds, because it was just connected.\"", healthChild.Hostname, healthChildPostpone): 1,
				"level=debug … thread=HEALTH msg=\"[" + healthChild.Hostname + healthRecResuming + `"`: 1,
			}))
		},
	}
}

// The `two-hosts` case's alert on localhost's own chart, beside the child's template (healthChildConf).
const (
	healthTwoHostsChart = "hloc.values"
	healthTwoHostsConf  = healthChildConf + `
 alarm: hloc_calc
    on: hloc.values
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: localhost's last value of a
`
)

// healthParentRelink is how many entries each of localhost's alerts logs when the agent becomes a parent, or stops
// being one: the first child that connects and the last that leaves change localhost's `_is_parent` label
// (rrdhost-labels.c:57-66, from stream-receiver-connection.c:781 and stream-receiver.c:773), which raises localhost's
// label recheck, and HEALTH's next pass unlinks every alert of localhost, links it again and runs it
// (health_event_loop.c:211-256): the unlink, the link and the status.
const healthParentRelink = 3

// healthChildTwoHostsCase is `two-hosts`: localhost's chart (the fake plugin's) and the child's switch their values
// at the same second (healthPair.atRelease), 10, 70, 10, 70. At 70 each host's alert is notified with its own
// host's name and machine GUID. Then the management API silences by host (`hosts=health-child`,
// health_silencers.c:461-487): the child's alert is silenced and localhost's is not, so the next CLEAR and WARNING
// are notified for localhost alone. Each host's calls are read with the host's own normalizer; the order of two
// hosts' calls in one pass is not compared (the two alerts may meet their values in different passes).
func healthChildTwoHostsCase() healthCase {
	const local, name = "hloc_calc", "hch_calc"
	values := []map[string]int64{{"a": 10}, {"a": 70}, {"a": 10}, {"a": 70}}
	silencers := healthSilencersJSON(false, "SILENCE", healthSel{"hosts", healthChild.Hostname})
	return healthCase{
		conf:   healthTwoHostsConf,
		stream: healthChildSection("health enabled = yes", "postpone alerts on connect = 0"),
		sc:     healthValues(healthTwoHostsChart, "hloc.ctx", []string{"a"}, values...),
		play: func(t *testing.T, h *healthPair) {
			v := h.childViews(t)
			h.create(t)
			h.compareNow(t, "before the child: the transitions", h.changes, healthChangesOf(local, 4))
			// two more passes of localhost's alert before the child connects: the unlink its connection brings carries
			// the value of the pass before the last, null after one pass alone (health.reload `child` has the sighting)
			time.Sleep(healthUnlinkHold)
			child := startFanoutChild(t, h.p, healthChild)
			time.Sleep(2 * time.Second)
			v.settle(child.define(healthChildChart, healthChildContext, []string{"a"}, values[0]))
			// localhost became a parent: its alert was linked again
			h.compareNow(t, "the child connected: the transitions", h.changes, healthBoth(healthChangesOf(local, 4+healthParentRelink),
				healthReloaded(local, "CLEAR", "CLEAR")))
			mine := func(i int) string {
				return healthCallsSinceLink(strings.Join(h.n[i].callsOf(t, h.p.Each()[i].Daemon, parentIdentity.MachineGUID), "\n"))
			}
			localCalls := func(statuses ...string) func(string) error {
				return healthHostCalls(parentIdentity.Hostname, parentIdentity.MachineGUID, local, statuses...)
			}
			// both switches both hosts' values to phase k at one second
			both := func(k int) {
				t.Helper()
				h.atRelease = func() { child.setNow(values[k]) }
				sec := h.release(t, fmt.Sprintf("p%d", k), k, healthCalcHold)
				h.atRelease = nil
				if got := child.switched(); got != sec {
					t.Fatalf("harness: the child switched to phase %d at second %d, localhost's plugin at %d", k, got, sec)
				}
			}
			h.compareNow(t, "at 10: /api/v1/alarms?all", h.all, healthAll("CLEAR", local))
			h.compareNow(t, "at 10: the child's /api/v1/alarms?all", v.all, healthAll("CLEAR", name))

			both(1)
			h.compareNow(t, "at 70: /api/v1/alarms?all", h.all, healthAll("WARNING", local))
			h.compareNow(t, "at 70: the child's /api/v1/alarms?all", v.all, healthAll("WARNING", name))
			h.processed(t, "at 70", local, "WARNING")
			v.processed("at 70", name, "WARNING")
			h.compareNow(t, "at 70: the notifier's calls for localhost", mine, localCalls("WARNING"))
			h.compareNow(t, "at 70: the notifier's calls for the child", v.calls, healthChildCalls(name, "WARNING"))
			// each host's alert is WARNING and notified: the alerts endpoint over two hosts (D234 F3, alerts_v2_test.go)
			alertsV2TwoHosts(t, h, v.cn)
			alertsV2TwoHostsNodes(t, h, v.cn)

			// the child's alerts are silenced, by its hostname
			h.manageStep(t, healthSaved("cmd=SILENCE&hosts="+healthChild.Hostname, healthMsgSilence+healthMsgAdded, silencers).
				with(local+" disabled=false silenced=false"))
			h.compareNow(t, "silenced: the child's alerts' flags", v.flags, healthFlagsWant(name+" disabled=false silenced=true"))

			both(2)
			h.compareNow(t, "back at 10: /api/v1/alarms?all", h.all, healthAll("CLEAR", local))
			h.compareNow(t, "back at 10: the child's /api/v1/alarms?all", v.all, healthAll("CLEAR", name))
			h.processed(t, "back at 10", local, "CLEAR")
			v.processed("back at 10", name, "CLEAR")
			h.compareNow(t, "back at 10: the notifier's calls for localhost", mine, localCalls("WARNING", "CLEAR"))
			h.compareNow(t, "back at 10: the notifier's calls for the child", v.calls, healthChildCalls(name, "WARNING"))

			both(3)
			h.compareNow(t, "back at 70: /api/v1/alarms?all", h.all, healthAll("WARNING", local))
			h.compareNow(t, "back at 70: the child's /api/v1/alarms?all", v.all, healthAll("WARNING", name))
			h.processed(t, "back at 70", local, "WARNING")
			v.processed("back at 70", name, "WARNING")
			h.compareNow(t, "back at 70: the notifier's calls for localhost", mine, localCalls("WARNING", "CLEAR", "WARNING"))
			h.compareNow(t, "back at 70: the notifier's calls for the child", v.calls, healthChildCalls(name, "WARNING"))
			// localhost's first pass links its alert three times and runs it (healthPair.create); the child's connection
			// has HEALTH link it again (healthParentRelink: three entries), then its three statuses; the child's alert:
			// its link and its four statuses
			h.compareNow(t, "at the end: /api/v1/alarm_log", h.log, healthBoth(healthLogEntries(4+3+3), healthHas(`"status":"REMOVED",`)))
			h.compareNow(t, "at the end: the child's /api/v1/alarm_log", v.log, healthLogEntries(1+4))
			// the transitions of the two hosts (D234 F3, alerts_closers_l_test.go)
			alertsV2TwoHostsTransitions(t, h, v.cn)
			time.Sleep(healthUnlinkHold)
			child.disconnect(t)
			h.compareNow(t, "the child's alert log after it disconnected", v.changes, healthLastChange(name, "WARNING->REMOVED"))
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareRecords(t, healthRecNoFile, healthRecWritten,
				healthRecChanged(healthChild.Hostname, name, [2]bool{false, false}, [2]bool{false, true}))
		},
	}
}

// healthChildHalfOffCase is `half-off`: localhost's alarm (two-hosts' hloc_calc) beside a child whose section
// turns its health off, so the parent has one host whose health runs and one whose health never ran. Localhost's
// chart goes 10, 70; the child's collects 70. Then the alerts rows of the two hosts (alertsV2HalfOff).
func healthChildHalfOffCase() healthCase {
	const local = "hloc_calc"
	return healthCase{
		conf:   healthTwoHostsConf,
		stream: healthChildSection("health enabled = no", "postpone alerts on connect = 0"),
		sc:     healthValues(healthTwoHostsChart, "hloc.ctx", []string{"a"}, map[string]int64{"a": 10}, map[string]int64{"a": 70}),
		play: func(t *testing.T, h *healthPair) {
			v := h.childViews(t)
			h.create(t)
			h.compareNow(t, "before the child: the transitions", h.changes, healthChangesOf(local, 4))
			time.Sleep(healthUnlinkHold)
			child := startFanoutChild(t, h.p, healthChild)
			time.Sleep(2 * time.Second)
			child.define(healthChildChart, healthChildContext, []string{"a"}, map[string]int64{"a": 70})
			// localhost became a parent: its alert was linked again
			h.compareNow(t, "the child connected: the transitions", h.changes, healthBoth(healthChangesOf(local, 4+healthParentRelink),
				healthReloaded(local, "CLEAR", "CLEAR")))
			h.release(t, "p1", 1, healthCalcHold)
			h.compareNow(t, "at 70: /api/v1/alarms?all", h.all, healthAll("WARNING", local))
			h.compareNow(t, "at 70: the child's /api/v1/alarms?all", v.all, healthHostOff)
			h.processed(t, "at 70", local, "WARNING")
			alertsV2HalfOff(t, h)
			time.Sleep(healthUnlinkHold)
			child.disconnect(t)
		},
	}
}

// healthChildDynCfgConf is the `dyncfg` case's rule file: a template on the child's context that stays CLEAR at 70.
const healthChildDynCfgConf = `template: hch_base
      on: hchild.ctx
    calc: $a
   every: 1s
    warn: $this > 90
   units: things
    info: the rule file's template on the child's context
`

// healthChildDynCfgCase is `dyncfg`: the child is connected and collects 70. A template on its context is added
// through localhost's DynCfg template (the payload held to the rail: healthPair.send): the alert is linked on the
// child (localhost has no chart of the context), rises, and is notified with the child's names. Then its job is
// disabled: the child's alert is removed.
func healthChildDynCfgCase() healthCase {
	const base, name = "hch_base", "hch_added"
	job := healthJob(name)
	return healthCase{
		conf:   healthChildDynCfgConf,
		stream: healthChildSection("health enabled = yes", "postpone alerts on connect = 0"),
		play: func(t *testing.T, h *healthPair) {
			h.bound = healthStoreBound
			v := h.childViews(t)
			child := startFanoutChild(t, h.p, healthChild)
			time.Sleep(2 * time.Second)
			v.settle(child.define(healthChildChart, healthChildContext, []string{"a"}, map[string]int64{"a": 70}))
			h.compareNow(t, "at the start: the child's /api/v1/alarms?all", v.all, healthAll("CLEAR", base))
			h.compareNow(t, "at the start: the tree", h.tree, healthBoth(healthTemplateNode, healthNode(healthJob(base), `"status":"running"`), healthNoNode(job)))

			time.Sleep(healthUnlinkHold)
			h.cfgStep(t, healthCfgStep{label: "add", query: "action=add&id=" + healthJobPrefix + "&name=" + name,
				body: healthPayload(healthRuleDoc(healthChildContext, "$a", "$this > 50", "type", "template")),
				code: 202, parts: []string{healthMsg(202, "accepted")}})
			h.compareNow(t, "added: the child's /api/v1/alarms?all", v.all, healthWant(map[string]string{base: "CLEAR", name: "WARNING"}))
			h.compareNow(t, "added: /api/v1/alarms?all", h.all, healthBoth(healthWant(map[string]string{}), healthHostOn))
			v.processed("added", name, "WARNING")
			h.compareNow(t, "added: the notifier's calls", v.calls, healthChildCalls(name, "WARNING"))
			h.compareNow(t, "added: the tree", h.tree, healthNode(job, `"status":"accepted",`+healthCmdsAdded, `"source_type":"dyncfg"`))

			time.Sleep(healthUnlinkHold)
			h.cfgStep(t, healthCfgStep{label: "disable", query: "action=disable&id=" + job, code: 200, parts: []string{healthMsg(200, "disabled")}})
			h.compareNow(t, "disabled: the child's alert log", v.changes, healthBoth(healthLastChange(name, "WARNING->REMOVED"),
				healthLastChange(base, "UNINITIALIZED->CLEAR")))
			h.compareNow(t, "disabled: the child's /api/v1/alarms?all", v.all, healthAll("CLEAR", base))
			h.compareNow(t, "disabled: the tree", h.tree, healthNode(job, `"status":"disabled"`, `"user_disabled":true`))
			h.compareNow(t, "disabled: the notifier's calls", v.calls, healthChildCalls(name, "WARNING"))
			// the child leaves: the alert that is left is removed
			time.Sleep(healthUnlinkHold)
			child.disconnect(t)
			h.compareNow(t, "the child's alert log after it disconnected", v.changes, healthBoth(healthLastChange(base, "CLEAR->REMOVED"),
				healthLastChange(name, "WARNING->REMOVED"), healthChangesOf(name, 3)))
			h.compareNow(t, "the child's /api/v1/alarms?all after it disconnected", v.all, healthHostOff)
			// the file's alert: its link, its status and the disconnect's unlink; the added one: its link, its status and
			// the disable's unlink
			h.compareNow(t, "at the end: the child's /api/v1/alarm_log", v.log, healthLogEntries(3+3))
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareCfgRecords(t, healthRecordsCount(2, map[string]int{
				"level=notice … msg=\"DYNCFG USER ACTION 'add' " + name + " on template '" + healthJobPrefix + "' by user": 1,
				"level=notice … msg=\"DYNCFG USER ACTION 'disable' on job '" + job + "' by user":                           1,
			}))
		},
	}
}

// healthChildRestartCase is `restart` (a hand-back, healthCase.again): the child's alert is raised and notified;
// both parents stop with the child connected (the detach of an exit logs no entry, rrdcalc.c:343-362, so the alert's
// last stored entry is its WARNING) and start again on their own directories; the child connects again. The parent
// loads the child's alert log when its health first runs (health_event_loop.c:258-279, sqlite_health.c:528-615): the
// alert takes its id back, and its WARNING is notified again (the load's entry stands between).
func healthChildRestartCase() healthCase {
	const name = "hch_calc"
	var v *healthChildViews
	var ids map[string]string
	return healthCase{
		conf:   healthChildConf,
		stream: healthChildSection("health enabled = yes", "postpone alerts on connect = 0"),
		play: func(t *testing.T, h *healthPair) {
			v = h.childViews(t)
			child := startFanoutChild(t, h.p, healthChild)
			time.Sleep(2 * time.Second)
			v.settle(child.define(healthChildChart, healthChildContext, []string{"a"}, map[string]int64{"a": 70}))
			ids = healthAlarmIDs(h.compareNow(t, "the first run: the child's /api/v1/alarms?all", v.all, healthAll("WARNING", name)))
			v.processed("the first run", name, "WARNING")
			h.compareNow(t, "the first run: the notifier's calls", v.calls, healthChildCalls(name, "WARNING"))
			h.compareNow(t, "the first run: the child's alert log", v.changes, healthSequenceIs(name, "REMOVED", "UNINITIALIZED", "WARNING"))
			h.compareNow(t, "the first run: the child's /api/v1/alarm_log", v.log, healthLogEntries(2))
			// the store has the entries (the log is read from the table); the child stops sending, and stays connected
			time.Sleep(healthUnlinkHold)
			child.halt(t)
		},
		again: func(t *testing.T, h *healthPair) {
			child := startFanoutChild(t, h.p, healthChild)
			time.Sleep(2 * time.Second)
			child.define(healthChildChart, healthChildContext, []string{"a"}, map[string]int64{"a": 70})
			h.compareNow(t, "the second run: the child's /api/v1/alarms?all", v.all, healthBoth(healthAll("WARNING", name), healthHostOn, healthIDsAre(ids)))
			v.processed("the second run", name, "WARNING")
			h.compareNow(t, "the second run: the child's alert log", v.changes, healthLastChanges(name, "REMOVED->UNINITIALIZED", "UNINITIALIZED->WARNING"))
			h.compareNow(t, "the second run: the notifier's calls", v.calls, healthChildCalls(name, "WARNING", "WARNING"))
			h.compareNow(t, "the second run: the child's /api/v1/alarm_log", v.log, healthHolds())
			time.Sleep(healthUnlinkHold)
			child.disconnect(t)
			h.compareNow(t, "the child's alert log after it disconnected", v.changes, healthLastChange(name, "WARNING->REMOVED"))
			h.compareNow(t, "the notifier's calls after the child disconnected", v.calls, healthChildCalls(name, "WARNING", "WARNING"))
		},
	}
}
