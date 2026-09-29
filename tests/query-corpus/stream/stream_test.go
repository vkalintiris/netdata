// SPDX-License-Identifier: GPL-3.0-or-later

package stream

import (
	"bufio"
	"fmt"
	"math"
	"net"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

func startFakeParent(t *testing.T, requested uint32, response, following string, keepOpen bool) string {
	t.Helper()

	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	hold := make(chan struct{})
	done := make(chan error, 1)
	go func() {
		defer listener.Close()
		conn, err := listener.Accept()
		if err != nil {
			done <- err
			return
		}
		defer conn.Close()

		reader := bufio.NewReader(conn)
		var request strings.Builder
		for {
			line, err := reader.ReadString('\n')
			if err != nil {
				done <- fmt.Errorf("read request: %w", err)
				return
			}
			request.WriteString(line)
			if strings.HasSuffix(request.String(), "\r\n\r\n") {
				break
			}
		}
		wantVersion := "&ver=" + strconv.FormatUint(uint64(requested), 10) + "&"
		if !strings.Contains(request.String(), wantVersion) {
			done <- fmt.Errorf("request does not advertise %q", wantVersion)
			return
		}

		// One-byte writes exercise fragmented prompt/capability delivery. The
		// receiver deliberately sends no delimiter after the decimal mask.
		for _, b := range []byte(prompt + response + following) {
			if _, err := conn.Write([]byte{b}); err != nil {
				done <- nil // the client may close immediately on a deliberate mismatch
				return
			}
		}
		if keepOpen {
			<-hold
		}
		done <- nil
	}()

	var once sync.Once
	release := func() error {
		once.Do(func() { close(hold) })
		return <-done
	}
	t.Cleanup(func() {
		if err := release(); err != nil {
			t.Errorf("fake parent: %v", err)
		}
	})
	return listener.Addr().String()
}

type connectResult struct {
	conn *Conn
	err  error
}

func connectWithin(t *testing.T, addr string, requested uint32, timeout time.Duration) connectResult {
	t.Helper()

	result := make(chan connectResult, 1)
	go func() {
		conn, err := Connect(addr, "fixture-key", HostInfo{
			Hostname:    "fixture-child",
			MachineGUID: "00000000-0000-0000-0000-000000000001",
		}, requested)
		result <- connectResult{conn: conn, err: err}
	}()

	select {
	case got := <-result:
		return got
	case <-time.After(timeout):
		t.Fatalf("Connect did not finish within %s while the parent kept the undelimited reply open", timeout)
		return connectResult{}
	}
}

func TestConnectAcceptsRequiredCapabilitiesWithoutDelimiter(t *testing.T) {
	for _, requested := range []uint32{CapsLive, CapsLiveV1, CapsReplication} {
		t.Run(strconv.FormatUint(uint64(requested), 10), func(t *testing.T) {
			addr := startFakeParent(
				t, requested, strconv.FormatUint(uint64(requested), 10), "", true)
			got := connectWithin(t, addr, requested, time.Second)
			if got.err != nil {
				t.Fatal(got.err)
			}
			defer got.conn.Close()
			if got.conn.Negotiated != requested {
				t.Fatalf("negotiated capabilities = %d, want %d", got.conn.Negotiated, requested)
			}
		})
	}
}

func TestConnectRejectsDifferentCapabilityMask(t *testing.T) {
	addr := startFakeParent(
		t, CapsReplication, strconv.FormatUint(uint64(CapsLive), 10), "", true)
	got := connectWithin(t, addr, CapsReplication, time.Second)
	if got.conn != nil {
		got.conn.Close()
		t.Fatal("Connect returned a connection for a parent missing required capabilities")
	}
	if got.err == nil {
		t.Fatal("Connect accepted a parent missing required capabilities")
	}
}

func TestConnectRejectsTruncatedCapabilityMask(t *testing.T) {
	required := strconv.FormatUint(uint64(CapsReplication), 10)
	addr := startFakeParent(t, CapsReplication, required[:len(required)-1], "", false)
	got := connectWithin(t, addr, CapsReplication, time.Second)
	if got.conn != nil {
		got.conn.Close()
		t.Fatal("Connect returned a connection for a truncated capability mask")
	}
	if got.err == nil {
		t.Fatal("Connect accepted a truncated capability mask")
	}
}

func TestConnectPreservesFirstProtocolLineAfterCapabilities(t *testing.T) {
	const sentinel = "REPLAY_CHART fixture.chart 1 2"
	addr := startFakeParent(
		t, CapsReplication, strconv.FormatUint(uint64(CapsReplication), 10), sentinel+"\n", true)
	got := connectWithin(t, addr, CapsReplication, time.Second)
	if got.err != nil {
		t.Fatal(got.err)
	}
	defer got.conn.Close()

	line, err := got.conn.ReadLine(time.Now().Add(time.Second))
	if err != nil {
		t.Fatal(err)
	}
	if line != sentinel {
		t.Fatalf("first protocol line = %q, want %q", line, sentinel)
	}
}

func TestServeReplicationPreservesChartCadence(t *testing.T) {
	childSide, parentSide := net.Pipe()
	t.Cleanup(func() {
		_ = childSide.Close()
		_ = parentSide.Close()
	})

	child := &Conn{
		conn: childSide,
		r:    bufio.NewReader(childSide),
		w:    bufio.NewWriter(childSide),
	}
	if err := parentSide.SetDeadline(time.Now().Add(time.Second)); err != nil {
		t.Fatal(err)
	}

	result := make(chan error, 1)
	go func() {
		_, err := child.ServeReplication(
			map[string]ReplayChart{
				"fixture.chart": {FirstT: 100, LastT: 105, UpdateEvery: 5},
			},
			105,
			func(_ string, _, _ int64) []ReplayRow {
				return []ReplayRow{{
					T: 105,
					Dims: []ReplayValue{{
						ID: "value", Collected: "7", Flags: FlagNotAnomalous,
					}},
				}}
			},
			time.Second,
		)
		result <- err
	}()

	if _, err := fmt.Fprintln(parentSide, `REPLAY_CHART "fixture.chart" "true" 100 105`); err != nil {
		t.Fatal(err)
	}
	reader := bufio.NewReader(parentSide)
	want := []string{
		"RBEGIN 'fixture.chart'",
		"RBEGIN 'fixture.chart' 100 105 105",
		"RSET 'value' 7 A",
		"REND 5 100 105 true 100 105 105",
	}
	for i, expected := range want {
		line, err := reader.ReadString('\n')
		if err != nil {
			t.Fatalf("replication response line %d: %v", i, err)
		}
		if got := strings.TrimSpace(line); got != expected {
			t.Fatalf("replication response line %d = %q, want %q", i, got, expected)
		}
	}
	if err := <-result; err != nil {
		t.Fatal(err)
	}
}

// The encodings of C's `buffer_print_*_encoded()`: `#A` is 0 and `@A` is 0.0 (the RDSTATE of a C child's capture),
// the digits most significant first, a negative integer's sign before its prefix.
func TestEncodingsAsC(t *testing.T) {
	for got, want := range map[string]string{
		EncodeU64(EncBase64, 0):    "#A",
		EncodeU64(EncBase64, 1):    "#B",
		EncodeU64(EncBase64, 64):   "#BA",
		EncodeU64(EncHex, 0):       "0x0",
		EncodeU64(EncHex, 255):     "0xFF",
		EncodeU64(EncDecimal, 42):  "42",
		EncodeI64(EncHex, -255):    "-0xFF",
		EncodeI64(EncBase64, -1):   "-#B",
		EncodeI64(EncDecimal, -3):  "-3",
		EncodeF64(EncBase64, 0):    "@A",
		EncodeF64(EncHex, 1):       "%3FF0000000000000",
		EncodeF64(EncDecimal, 1.5): "1.5",
	} {
		if got != want {
			t.Errorf("got %q, want %q", got, want)
		}
	}
	if got := EncodeF64(EncDecimal, math.NaN()); got != "NAN" {
		t.Errorf("NaN: %q", got)
	}
	if i, d := EncodingFor(CapIEEE754); i != EncBase64 || d != EncBase64 {
		t.Errorf("IEEE754: %v %v", i, d)
	}
	if i, d := EncodingFor(0); i != EncHex || d != EncDecimal {
		t.Errorf("no IEEE754: %v %v", i, d)
	}
}

// Serve answers a parent's REPLAY_CHART in the background while the caller's bursts write, never interleaved; the
// request carries the child's hops; a line outside a burst panics.
func TestServeAnswersWhileBursting(t *testing.T) {
	p, err := StartParent(func(r Request) Answer {
		a := PlaintextAnswer(r)
		// the stub reads a sender's double-quoted CHART lines; this child quotes with ', so the chart is named here
		a.Replay = func(ev ReplayEvent) []string {
			if !ev.Answer {
				return []string{`REPLAY_CHART "a.b" "true" 10 20`}
			}
			return nil
		}
		return a
	})
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	c, err := Connect(p.Addr(), "k", HostInfo{Hostname: "h", MachineGUID: "00000000-0000-0000-0000-000000000002",
		Hops: 3}, CapsReplication)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	rows := func(string, int64, int64) []ReplayRow {
		return []ReplayRow{{T: 20, Dims: []ReplayValue{{ID: "d", Collected: "5", Flags: FlagNotAnomalous}}}}
	}
	c.Serve(map[string]ReplayChart{"a.b": {FirstT: 10, LastT: 20}}, 30, rows)
	_ = c.Burst(func() {
		c.DefineChart(Chart{ID: "a.b"})
		c.Dimension("d", "", 0, 0)
		c.ChartDefinitionEnd(10, 20, 30)
	})
	if err := c.WaitGranted([]string{"a.b"}, 5*time.Second); err != nil {
		t.Fatal(err)
	}
	_ = c.Burst(func() {
		c.Begin2Raw("", "a.b", "1", EncodeU64(EncDecimal, 21), "#")
		c.Set2Raw("", "d", "6", "6", FlagNotAnomalous)
		c.End2()
	})
	s := p.WaitSession(1, 5*time.Second)
	if s == nil {
		t.Fatal("no session")
	}
	if !strings.Contains(s.Request.Line, "&hops=3&") {
		t.Errorf("request: %s", s.Request.Line)
	}
	want := "RBEGIN 'a.b'\nRBEGIN 'a.b' 19 20 30\nRSET 'd' 5 A\nREND 1 10 20 true 10 20 30\n" +
		"BEGIN2 'a.b' 1 21 #\nSET2 'd' 6 6 A\nEND2\n"
	deadline := time.Now().Add(5 * time.Second)
	for !strings.HasSuffix(string(s.Data()), want) && time.Now().Before(deadline) {
		time.Sleep(20 * time.Millisecond)
	}
	if got := string(s.Data()); !strings.HasSuffix(got, want) {
		t.Errorf("the parent got:\n%s", got)
	}
	if down := c.Downstream(); len(down) == 0 || down[0].Line != `REPLAY_CHART "a.b" "true" 10 20` {
		t.Errorf("downstream: %v", down)
	}
	defer func() {
		if recover() == nil {
			t.Error("a line outside a burst did not panic")
		}
	}()
	c.End2()
}

// A ReplayPlan asks a chart's windows in turn, the first at its first CHART_DEFINITION_END and the next at each
// REND, and starts an unplanned chart at once; a repeated definition asks nothing more.
func TestReplayPlanAsksInTurn(t *testing.T) {
	plan := ReplayPlan(map[string][]ReplayWindow{"p.c": {{false, 1, 2}, {true, 2, 3}}})
	p, err := StartParent(func(r Request) Answer {
		a := PlaintextAnswer(r)
		a.Replay = plan
		return a
	})
	if err != nil {
		t.Fatal(err)
	}
	defer p.Close()
	c, err := Connect(p.Addr(), "k", HostInfo{Hostname: "h", MachineGUID: "00000000-0000-0000-0000-000000000003"},
		CapsReplication)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	// a sender's lines, double-quoted as C writes them
	_ = c.WriteRaw([]byte("CHART \"p.c\" '' t u f c line 1 1 '' p m\nCHART_DEFINITION_END 1 2 3\n" +
		"CHART \"u.c\" '' t u f c line 1 1 '' p m\nCHART_DEFINITION_END 1 2 3\n" +
		"RBEGIN 'p.c'\nREND 1 1 2 false 1 2 3\nCHART \"p.c\" '' t u f c line 1 1 '' p m\n" +
		"CHART_DEFINITION_END 1 2 3\nRBEGIN 'p.c'\nREND 1 1 3 true 2 3 3\n"))
	want := []string{`REPLAY_CHART "p.c" "false" 1 2`, `REPLAY_CHART "u.c" "true" 0 0`, `REPLAY_CHART "p.c" "true" 2 3`}
	var got []string
	deadline := time.Now().Add(5 * time.Second)
	for len(got) < len(want) && time.Now().Before(deadline) {
		line, err := c.ReadLine(deadline)
		if err != nil {
			break
		}
		got = append(got, line)
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Errorf("asked:\n%s\nwant:\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
	if line, err := c.ReadLine(time.Now().Add(300 * time.Millisecond)); err == nil {
		t.Errorf("asked more: %s", line)
	}
}
