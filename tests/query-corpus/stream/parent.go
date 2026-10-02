// SPDX-License-Identifier: GPL-3.0-or-later

package stream

import (
	"bufio"
	"bytes"
	"crypto/tls"
	"errors"
	"fmt"
	"io"
	"net"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"
)

// More capability bits (src/streaming/stream-capabilities.h), for a parent's answers.
const (
	CapV1            uint32 = 1 << 3
	CapV2            uint32 = 1 << 4
	CapVN            uint32 = 1 << 5
	CapClaim         uint32 = 1 << 8
	CapLZ4           uint32 = 1 << 10
	CapFunctions     uint32 = 1 << 11
	CapBinary        uint32 = 1 << 13
	CapIEEE754       uint32 = 1 << 15
	CapSlots         uint32 = 1 << 18
	CapZSTD          uint32 = 1 << 19
	CapGZIP          uint32 = 1 << 20
	CapBrotli        uint32 = 1 << 21
	CapProgress      uint32 = 1 << 22
	CapDynCfg        uint32 = 1 << 23
	CapNodeID        uint32 = 1 << 24
	CapPaths         uint32 = 1 << 25
	CapMLModels      uint32 = 1 << 26
	CapFloatBaseline uint32 = 1 << 27
	CapFunctionDel   uint32 = 1 << 28

	// CapsCompression are the compression bits; a parent that drops them gets plaintext.
	CapsCompression = CapLZ4 | CapZSTD | CapGZIP | CapBrotli
)

// The parent's rejections (src/streaming/stream-handshake.h).
const (
	RejectSameLocalhost    = "Don't hit me baby, you are trying to stream my localhost back"
	RejectLocalVnode       = "Don't hit me baby, you are trying to stream my vnode back"
	RejectAlreadyStreaming = "This GUID is already streaming to this server"
	RejectNotPermitted     = "You are not permitted to access this. Check the logs for more info."
	RejectBusy             = "The server is too busy now to accept this request. Try later."
	RejectInternalError    = "The server encountered an internal error. Try later."
	RejectInitializing     = "The server is initializing. Try later."
)

// chartDefRe is a CHART line's id, with or without a slot.
var chartDefRe = regexp.MustCompile(`^CHART (?:SLOT:\S+ )?"([^"]*)"`)

// Request is a child's STREAM request as a parent reads it.
type Request struct {
	// Raw is every byte up to and including the blank line.
	Raw string
	// Line is the request line without its CRLF.
	Line string
	// Params are the query's parameters, decoded.
	Params url.Values
	// Headers are the header lines in order, without CRLF.
	Headers []string
}

// Caps is the capabilities the child offered (`ver`).
func (r Request) Caps() uint32 {
	v, _ := strconv.ParseUint(r.Params.Get("ver"), 10, 32)
	return uint32(v)
}

// Answer is what a scripted parent does with a request.
type Answer struct {
	// Reply is written after the request: a prompt with its capabilities (VCaps) or a rejection.
	Reply string
	// Close ends the connection after the reply.
	Close bool
	// Down are lines written after the reply, each ending with a newline.
	Down []string
	// DownAfter delays Down: a child reads its prompt with one receive, so lines sent with it would be read as part
	// of the prompt.
	DownAfter time.Duration
	// StopReading leaves the child's data unread after the handshake (a parent whose buffers fill).
	StopReading bool
	// StartStreaming answers each chart's CHART_DEFINITION_END with `REPLAY_CHART "<id>" "true" 0 0`: nothing to
	// replay, start streaming (a child with REPLICATION sends a chart's data only after its parent's request).
	StartStreaming bool
	// Replay, when set, decides instead what the parent sends at each chart's CHART_DEFINITION_END and at each
	// replication answer's REND: the lines it returns.
	Replay func(ReplayEvent) []string
	// Silent writes no reply and keeps the connection until the child closes it (a parent that never answers).
	Silent bool
	// CloseNow closes the connection without a reply.
	CloseNow bool
}

// ReplayEvent is a line an Answer's Replay reacts to: a chart's definition end, or its replication answer's end.
type ReplayEvent struct {
	// Chart is the chart whose definition or answer ends.
	Chart string
	// Answer is true for a REND, false for a CHART_DEFINITION_END.
	Answer bool
	// Line is the line itself.
	Line string
}

// ReplayWindow is one replication request a parent asks: whether to start streaming, and the (after, before]
// window.
type ReplayWindow struct {
	Start         bool
	After, Before int64
}

// ReplayPlan is an Answer.Replay asking each planned chart's windows in turn: the first at its first
// CHART_DEFINITION_END, the next at each REND of its answers, nothing after the last. A chart without a plan starts
// streaming at once (`"true" 0 0`). It keeps each chart's position: one per session.
func ReplayPlan(plan map[string][]ReplayWindow) func(ReplayEvent) []string {
	var mu sync.Mutex
	next := map[string]int{}
	return func(ev ReplayEvent) []string {
		mu.Lock()
		defer mu.Unlock()
		windows, planned := plan[ev.Chart]
		if !planned {
			if ev.Answer {
				return nil
			}
			return []string{`REPLAY_CHART "` + ev.Chart + `" "true" 0 0`}
		}
		i := next[ev.Chart]
		if i >= len(windows) || (!ev.Answer && i > 0) {
			return nil
		}
		next[ev.Chart] = i + 1
		w := windows[i]
		return []string{fmt.Sprintf(`REPLAY_CHART "%s" "%t" %d %d`, ev.Chart, w.Start, w.After, w.Before)}
	}
}

// replayChartRe is a replication answer's first line, which names its chart (the steps' RBEGIN lines name none).
var replayChartRe = regexp.MustCompile(`^RBEGIN (?:SLOT:\S+ )?'([^']+)'$`)

// VCaps is the capabilities prompt with `caps`.
func VCaps(caps uint32) string {
	return prompt + strconv.FormatUint(uint64(caps), 10)
}

// PlaintextAnswer accepts with what the child offered minus compression, so its stream arrives as text, and starts
// each chart's streaming at once.
func PlaintextAnswer(r Request) Answer {
	return Answer{Reply: VCaps(r.Caps() &^ CapsCompression), StartStreaming: true}
}

// Chunk is one read of a session's data and when it arrived.
type Chunk struct {
	At   time.Time
	Data []byte
}

// Session is one accepted STREAM connection.
type Session struct {
	Request Request
	// At is when the parent read the request.
	At     time.Time
	mu     sync.Mutex
	chunks []Chunk
	closed bool
	conn   net.Conn
}

// Data is every byte the child sent after the handshake so far.
func (s *Session) Data() []byte {
	s.mu.Lock()
	defer s.mu.Unlock()
	var b bytes.Buffer
	for _, c := range s.chunks {
		b.Write(c.Data)
	}
	return b.Bytes()
}

// Chunks are the reads so far, with their arrival times.
func (s *Session) Chunks() []Chunk {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]Chunk(nil), s.chunks...)
}

// WaitData waits up to `timeout` until what the child sent satisfies `cond`.
func (s *Session) WaitData(cond func(data []byte) bool, timeout time.Duration) bool {
	deadline := time.Now().Add(timeout)
	for !cond(s.Data()) {
		if time.Now().After(deadline) {
			return false
		}
		time.Sleep(20 * time.Millisecond)
	}
	return true
}

// Closed tells whether the child closed the connection (or it failed).
func (s *Session) Closed() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.closed
}

// Send writes lines down to the child, each ending with a newline.
func (s *Session) Send(lines ...string) error {
	var b strings.Builder
	for _, l := range lines {
		b.WriteString(l)
		b.WriteByte('\n')
	}
	_, err := s.conn.Write([]byte(b.String()))
	return err
}

// Close ends the session from the parent's side.
func (s *Session) Close() error {
	return s.conn.Close()
}

// CloseNotify sends a TLS close_notify and keeps the TCP connection, which CloseRaw ends.
func (s *Session) CloseNotify() error {
	if tc, ok := s.conn.(*tls.Conn); ok {
		return tc.CloseWrite()
	}
	return nil
}

// CloseRaw ends the session's TCP connection without a TLS close_notify (Close sends one over TLS).
func (s *Session) CloseRaw() error {
	if tc, ok := s.conn.(*tls.Conn); ok {
		return tc.NetConn().Close()
	}
	return s.conn.Close()
}

// Reset ends the session's TCP connection with a reset (SO_LINGER 0, no TLS close_notify): the child's socket loses
// its peer at once.
func (s *Session) Reset() error {
	nc := s.conn
	if tc, ok := nc.(*tls.Conn); ok {
		nc = tc.NetConn()
	}
	if tcp, ok := nc.(*net.TCPConn); ok {
		_ = tcp.SetLinger(0)
	}
	return nc.Close()
}

// Hello is a TLS client's ClientHello as a TLS parent read it, and when.
type Hello struct {
	At                time.Time
	ServerName        string
	SupportedProtos   []string
	SupportedVersions []uint16
	CipherSuites      []uint16
	SupportedCurves   []tls.CurveID
	SignatureSchemes  []tls.SignatureScheme
	SupportedPoints   []uint8
	Extensions        []uint16
}

// String is the hello without its time, for comparisons.
func (h Hello) String() string {
	return fmt.Sprintf("sni=%q alpn=%q versions=%x suites=%x curves=%d schemes=%x points=%d extensions=%d",
		h.ServerName, h.SupportedProtos, h.SupportedVersions, h.CipherSuites, h.SupportedCurves, h.SignatureSchemes,
		h.SupportedPoints, h.Extensions)
}

// NotFound is the default answer to a `stream_info` probe: 404 with no content (the child keeps the parent as a
// candidate).
const NotFound = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"

// Parent is a scripted streaming parent on 127.0.0.1: it answers a child's `stream_info` probe as `Probe` says
// (NotFound when nil), an h2o `GET /stream` upgrade as `Upgrade` says (a 101 answer keeps the connection for the
// STREAM request that follows), records each STREAM request and every byte after it, and answers as `Script` says
// (PlaintextAnswer when nil). Set `Probe` and `Upgrade` before a child connects. A TLS parent (StartParentTLS)
// does the same over TLS, recording each connection's ClientHello.
type Parent struct {
	Script func(Request) Answer
	// Probe is the raw answer to a probe's raw request; nil bytes close the connection without one.
	Probe func(raw string) []byte
	// Upgrade is the raw answer to `GET /stream`; nil or empty bytes close without one.
	Upgrade func(raw string) []byte
	// RefuseHandshake, on a TLS parent, fails the n-th connection's handshake (1-based) with an alert after its
	// ClientHello.
	RefuseHandshake func(n int) bool
	ln              net.Listener
	tls             *tls.Config
	mu              sync.Mutex
	sessions        []*Session
	probes          []string
	probeAt         []time.Time
	probeRaw        []string
	upgrades        []string
	hellos          []Hello
	accepted        int
}

// StartParent listens on a free port of 127.0.0.1.
func StartParent(script func(Request) Answer) (*Parent, error) {
	return StartParentTLS(script, nil)
}

// StartParentOn is StartParent listening on `addr` (host:port), e.g. a loopback address of its own at the agents'
// default port.
func StartParentOn(addr string, script func(Request) Answer) (*Parent, error) {
	return startParent(addr, script, nil)
}

// StartParentTLS is StartParent over TLS with `config` (its certificates), plain text when nil.
func StartParentTLS(script func(Request) Answer, config *tls.Config) (*Parent, error) {
	return startParent("127.0.0.1:0", script, config)
}

func startParent(addr string, script func(Request) Answer, config *tls.Config) (*Parent, error) {
	ln, err := net.Listen("tcp", addr)
	if err != nil {
		return nil, err
	}
	p := &Parent{Script: script, ln: ln, tls: config}
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			if config == nil {
				go p.serve(c)
			} else {
				go p.serveTLS(c)
			}
		}
	}()
	return p, nil
}

// SetRefuseHandshake replaces which TLS handshakes fail while children connect.
func (p *Parent) SetRefuseHandshake(refuse func(n int) bool) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.RefuseHandshake = refuse
}

// Hellos are a TLS parent's ClientHellos so far, in arrival order.
func (p *Parent) Hellos() []Hello {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]Hello(nil), p.hellos...)
}

// serveTLS records the connection's ClientHello, fails its handshake when RefuseHandshake says so, and serves the
// rest over TLS.
func (p *Parent) serveTLS(c net.Conn) {
	p.mu.Lock()
	p.accepted++
	n, refuse := p.accepted, p.RefuseHandshake
	p.mu.Unlock()
	config := p.tls.Clone()
	config.GetConfigForClient = func(h *tls.ClientHelloInfo) (*tls.Config, error) {
		p.mu.Lock()
		p.hellos = append(p.hellos, Hello{At: time.Now(), ServerName: h.ServerName, SupportedProtos: h.SupportedProtos,
			SupportedVersions: h.SupportedVersions, CipherSuites: h.CipherSuites, SupportedCurves: h.SupportedCurves,
			SignatureSchemes: h.SignatureSchemes, SupportedPoints: h.SupportedPoints, Extensions: h.Extensions})
		p.mu.Unlock()
		if refuse != nil && refuse(n) {
			return nil, errors.New("handshake refused")
		}
		return nil, nil
	}
	tc := tls.Server(c, config)
	_ = tc.SetDeadline(time.Now().Add(30 * time.Second))
	if err := tc.Handshake(); err != nil {
		_ = c.Close()
		return
	}
	_ = tc.SetDeadline(time.Time{})
	p.serve(tc)
}

// SetProbe replaces the probe's answer while children connect.
func (p *Parent) SetProbe(probe func(raw string) []byte) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.Probe = probe
}

// SetScript replaces the answer to the STREAM requests that follow.
func (p *Parent) SetScript(script func(Request) Answer) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.Script = script
}

// Addr is the parent's host:port, a child's `destination`.
func (p *Parent) Addr() string { return p.ln.Addr().String() }

// Close stops listening and ends every session.
func (p *Parent) Close() error {
	err := p.ln.Close()
	p.mu.Lock()
	defer p.mu.Unlock()
	for _, s := range p.sessions {
		_ = s.conn.Close()
	}
	return err
}

// Sessions are the STREAM connections so far, in arrival order.
func (p *Parent) Sessions() []*Session {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]*Session(nil), p.sessions...)
}

// ProbeTimes are when the parent read each `stream_info` probe so far.
func (p *Parent) ProbeTimes() []time.Time {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]time.Time(nil), p.probeAt...)
}

// Probes are the request lines of the `stream_info` probes so far.
func (p *Parent) Probes() []string {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]string(nil), p.probes...)
}

// ProbeRequests are the probes' raw requests so far.
func (p *Parent) ProbeRequests() []string {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]string(nil), p.probeRaw...)
}

// Upgrades are the raw `GET /stream` requests so far.
func (p *Parent) Upgrades() []string {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]string(nil), p.upgrades...)
}

// readRequest reads an HTTP request's head, up to and including its blank line.
func readRequest(c net.Conn, br *bufio.Reader) (Request, bool) {
	_ = c.SetReadDeadline(time.Now().Add(30 * time.Second))
	var raw strings.Builder
	for !strings.HasSuffix(raw.String(), "\r\n\r\n") {
		line, err := br.ReadString('\n')
		raw.WriteString(line)
		if err != nil {
			return Request{}, false
		}
	}
	_ = c.SetReadDeadline(time.Time{})
	lines := strings.Split(strings.TrimSuffix(raw.String(), "\r\n\r\n"), "\r\n")
	return Request{Raw: raw.String(), Line: lines[0], Headers: lines[1:]}, true
}

// WaitSession waits up to `timeout` for the n-th session (1-based).
func (p *Parent) WaitSession(n int, timeout time.Duration) *Session {
	deadline := time.Now().Add(timeout)
	for {
		if s := p.Sessions(); len(s) >= n {
			return s[n-1]
		}
		if time.Now().After(deadline) {
			return nil
		}
		time.Sleep(50 * time.Millisecond)
	}
}

// SessionsFor are the STREAM connections so far whose request names the machine GUID `guid`, in arrival order: one
// child streams each of its hosts (localhost, each vnode) on a connection of its own.
func (p *Parent) SessionsFor(guid string) []*Session {
	var out []*Session
	for _, s := range p.Sessions() {
		if s.Request.Params.Get("machine_guid") == guid {
			out = append(out, s)
		}
	}
	return out
}

// WaitSessionFor waits up to `timeout` for the n-th session (1-based) of the machine GUID `guid`.
func (p *Parent) WaitSessionFor(guid string, n int, timeout time.Duration) *Session {
	deadline := time.Now().Add(timeout)
	for {
		if s := p.SessionsFor(guid); len(s) >= n {
			return s[n-1]
		}
		if time.Now().After(deadline) {
			return nil
		}
		time.Sleep(50 * time.Millisecond)
	}
}

func (p *Parent) serve(c net.Conn) {
	br := bufio.NewReader(c)
	req, ok := readRequest(c, br)
	if !ok {
		_ = c.Close()
		return
	}
	if strings.HasPrefix(req.Line, "GET /stream ") {
		p.mu.Lock()
		p.upgrades = append(p.upgrades, req.Raw)
		upgrade := p.Upgrade
		p.mu.Unlock()
		var answer []byte
		if upgrade != nil {
			answer = upgrade(req.Raw)
		}
		if len(answer) > 0 {
			_, _ = c.Write(answer)
		}
		if !strings.HasPrefix(string(answer), "HTTP/1.1 101 ") {
			_ = c.Close()
			return
		}
		if req, ok = readRequest(c, br); !ok {
			_ = c.Close()
			return
		}
	}
	if !strings.HasPrefix(req.Line, "STREAM ") {
		p.mu.Lock()
		p.probes = append(p.probes, req.Line)
		p.probeAt = append(p.probeAt, time.Now())
		p.probeRaw = append(p.probeRaw, req.Raw)
		probe := p.Probe
		p.mu.Unlock()
		answer := []byte(NotFound)
		if probe != nil {
			answer = probe(req.Raw)
		}
		if answer != nil {
			_, _ = c.Write(answer)
		}
		_ = c.Close()
		return
	}
	at := time.Now()
	query := strings.TrimSuffix(strings.TrimPrefix(req.Line, "STREAM "), " HTTP/1.1")
	req.Params, _ = url.ParseQuery(query)
	p.mu.Lock()
	script := p.Script
	p.mu.Unlock()
	if script == nil {
		script = PlaintextAnswer
	}
	a := script(req)
	s := &Session{Request: req, At: at, conn: c}
	p.mu.Lock()
	p.sessions = append(p.sessions, s)
	p.mu.Unlock()
	if a.CloseNow {
		_ = c.Close()
		s.mu.Lock()
		s.closed = true
		s.mu.Unlock()
		return
	}
	if a.Silent {
		_, _ = io.Copy(io.Discard, br)
		_ = c.Close()
		s.mu.Lock()
		s.closed = true
		s.mu.Unlock()
		return
	}
	if _, err := c.Write([]byte(a.Reply)); err != nil || a.Close {
		_ = c.Close()
		s.mu.Lock()
		s.closed = true
		s.mu.Unlock()
		return
	}
	if len(a.Down) > 0 {
		time.Sleep(a.DownAfter)
		_ = s.Send(a.Down...)
	}
	if a.StopReading {
		return
	}
	buf := make([]byte, 65536)
	var partial []byte // the last line not complete yet
	chart := ""        // the chart whose definition is being read
	replayed := ""     // the chart whose replication answer is being read
	for {
		n, err := br.Read(buf)
		if n > 0 {
			s.mu.Lock()
			s.chunks = append(s.chunks, Chunk{At: time.Now(), Data: append([]byte(nil), buf[:n]...)})
			s.mu.Unlock()
			if a.StartStreaming || a.Replay != nil {
				partial = append(partial, buf[:n]...)
				for {
					at := bytes.IndexByte(partial, '\n')
					if at < 0 {
						break
					}
					line := string(partial[:at])
					partial = partial[at+1:]
					switch m := chartDefRe.FindStringSubmatch(line); {
					case m != nil:
						chart = m[1]
					case strings.HasPrefix(line, "CHART_DEFINITION_END "):
						if a.Replay != nil {
							_ = s.Send(a.Replay(ReplayEvent{Chart: chart, Line: line})...)
						} else {
							_ = s.Send(`REPLAY_CHART "` + chart + `" "true" 0 0`)
						}
					case replayChartRe.MatchString(line):
						replayed = replayChartRe.FindStringSubmatch(line)[1]
					case strings.HasPrefix(line, "REND ") && a.Replay != nil:
						_ = s.Send(a.Replay(ReplayEvent{Chart: replayed, Answer: true, Line: line})...)
					}
				}
			}
		}
		if err != nil {
			s.mu.Lock()
			s.closed = true
			s.mu.Unlock()
			_ = c.Close()
			return
		}
	}
}
