// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bufio"
	"bytes"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"fmt"
	"io"
	"math/big"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// proxyRecorder is an `HTTPS_PROXY` that records each CONNECT request libcurl sends it and refuses it (403): an agent's
// crash report attempt, stopped on the box. With `tls` it accepts the tunnel instead, ends the agent's TLS with the
// harness CA's certificate for agent-events, records the POST and answers `status`.
type proxyRecorder struct {
	ln       net.Listener
	tls      *tls.Config
	status   int
	mu       sync.Mutex
	requests []string
	posts    []reportPost
}

// reportPost is a report as the recorder received it.
type reportPost struct {
	method, uri string
	header      http.Header
	body        []byte
}

// bufferedConn reads what the CONNECT's reader buffered before the connection.
type bufferedConn struct {
	net.Conn
	r *bufio.Reader
}

func (c bufferedConn) Read(b []byte) (int, error) { return c.r.Read(b) }

func newProxyRecorder() (*proxyRecorder, error) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	r := &proxyRecorder{ln: ln}
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			go r.serve(c)
		}
	}()
	return r, nil
}

func (r *proxyRecorder) serve(c net.Conn) {
	defer c.Close()
	_ = c.SetDeadline(time.Now().Add(10 * time.Second))
	br := bufio.NewReader(c)
	var head bytes.Buffer
	for !bytes.HasSuffix(head.Bytes(), []byte("\r\n\r\n")) {
		line, err := br.ReadBytes('\n')
		head.Write(line)
		if err != nil {
			break
		}
	}
	r.mu.Lock()
	r.requests = append(r.requests, head.String())
	r.mu.Unlock()
	if r.tls == nil {
		_, _ = c.Write([]byte("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n"))
		return
	}
	if _, err := c.Write([]byte("HTTP/1.1 200 Connection established\r\n\r\n")); err != nil {
		return
	}
	tc := tls.Server(bufferedConn{c, br}, r.tls)
	req, err := http.ReadRequest(bufio.NewReader(tc))
	if err != nil {
		return
	}
	body, _ := io.ReadAll(req.Body)
	r.mu.Lock()
	r.posts = append(r.posts, reportPost{req.Method, req.RequestURI, req.Header.Clone(), body})
	r.mu.Unlock()
	_, _ = fmt.Fprintf(tc, "HTTP/1.1 %d %s\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", r.status,
		http.StatusText(r.status))
	_ = tc.Close()
}

// takePosts returns the reports so far and forgets them.
func (r *proxyRecorder) takePosts() []reportPost {
	r.mu.Lock()
	defer r.mu.Unlock()
	got := r.posts
	r.posts = nil
	return got
}

// reportCA is a harness CA's bundle and its certificate for agent-events.netdata.cloud.
type reportCA struct {
	bundle string
	leaf   tls.Certificate
}

func newReportCA(t *testing.T) *reportCA {
	t.Helper()
	caKey, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	caTmpl := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "parity crash report CA"},
		NotBefore: now.Add(-time.Hour), NotAfter: now.Add(24 * time.Hour), IsCA: true, BasicConstraintsValid: true,
		KeyUsage: x509.KeyUsageCertSign | x509.KeyUsageDigitalSignature}
	caDER, err := x509.CreateCertificate(rand.Reader, caTmpl, caTmpl, &caKey.PublicKey, caKey)
	if err != nil {
		t.Fatal(err)
	}
	ca, err := x509.ParseCertificate(caDER)
	if err != nil {
		t.Fatal(err)
	}
	leafKey, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	leafTmpl := &x509.Certificate{SerialNumber: big.NewInt(2), Subject: pkix.Name{CommonName: "agent-events.netdata.cloud"},
		DNSNames: []string{"agent-events.netdata.cloud"}, NotBefore: now.Add(-time.Hour), NotAfter: now.Add(24 * time.Hour),
		KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth}}
	leafDER, err := x509.CreateCertificate(rand.Reader, leafTmpl, ca, &leafKey.PublicKey, caKey)
	if err != nil {
		t.Fatal(err)
	}
	bundle := filepath.Join(t.TempDir(), "ca.pem")
	if err := os.WriteFile(bundle, pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: caDER}), 0o644); err != nil {
		t.Fatal(err)
	}
	return &reportCA{bundle: bundle, leaf: tls.Certificate{Certificate: [][]byte{leafDER}, PrivateKey: leafKey}}
}

// trustedWrap runs a daemon in a user and mount namespace whose system CA bundle (libcurl's compiled-in
// `/etc/ssl/certs/ca-certificates.crt`, for C and the Rust agent alike) is `bundle`, back as the invoking user with no
// capabilities; each `unshare` and `sh` execs in place, so the PID stays the daemon's.
func trustedWrap(bundle string) []string {
	return []string{"unshare", "--user", "--map-root-user", "--mount", "sh", "-c",
		`mount --bind "$1" /etc/ssl/certs/ca-certificates.crt && shift && exec unshare --user --map-user=` +
			strconv.Itoa(os.Getuid()) + ` --map-group=` + strconv.Itoa(os.Getgid()) + ` -- "$@"`, "sh", bundle}
}

// env is the variable that sends a daemon's reports here.
func (r *proxyRecorder) env() string {
	return "HTTPS_PROXY=http://" + r.ln.Addr().String()
}

// take returns the requests so far and forgets them.
func (r *proxyRecorder) take() []string {
	r.mu.Lock()
	defer r.mu.Unlock()
	got := r.requests
	r.requests = nil
	return got
}

// reportGuard receives the reports of every daemon no check points elsewhere (TestMain): none may go out.
var reportGuard *proxyRecorder

// startReportGuard points libcurl's proxy at the guard for every daemon the harness starts.
func startReportGuard() {
	g, err := newProxyRecorder()
	if err != nil {
		fmt.Fprintf(os.Stderr, "parity: report guard: %v\n", err)
		os.Exit(1)
	}
	reportGuard = g
	os.Setenv("HTTPS_PROXY", "http://"+g.ln.Addr().String())
}

// reportGuardFailed reports the attempts the guard stopped: a check that enables crash reports without its own
// recorder.
func reportGuardFailed() bool {
	if got := reportGuard.take(); len(got) > 0 {
		fmt.Fprintf(os.Stderr, "parity: %d crash report attempts reached the guard proxy\n", len(got))
		return true
	}
	return false
}

// reportOpts are a report pair's extras: environment variables, and a CA both sides trust, whose recorder then
// accepts the reports and answers `status`.
type reportOpts struct {
	env    []string
	ca     *reportCA
	status int
}

// reportPair boots both sides as the status file checks do, with `crash reports = <mode>`, each side's reports going
// to its own recorder.
func reportPair(t *testing.T, mode string, ro reportOpts) (*Pair, [2]*proxyRecorder) {
	t.Helper()
	p := &Pair{}
	var recs [2]*proxyRecorder
	bins := binaries(t)
	for i, role := range []Role{"report-oracle", "report-candidate"} {
		rec, err := newProxyRecorder()
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { rec.ln.Close() })
		recs[i] = rec
		id := parentIdentity
		o := daemon.Options{Binary: bins[i], RunDir: runDir(t, role), Identity: &id, DBMode: "alloc", StorageTiers: 1,
			GlobalExtra: "    crash reports = " + mode + "\n", Env: append([]string{rec.env()}, ro.env...)}
		if ro.ca != nil {
			rec.mu.Lock()
			rec.tls = &tls.Config{Certificates: []tls.Certificate{ro.ca.leaf}, NextProtos: []string{"http/1.1"}}
			rec.status = ro.status
			rec.mu.Unlock()
			o.Wrap = trustedWrap(ro.ca.bundle)
		}
		d, err := daemon.Start(o)
		if err != nil {
			t.Fatalf("parity: start %s: %v", role, err)
		}
		t.Cleanup(func() { _ = d.Stop() })
		if i == 0 {
			p.Oracle = d
		} else {
			p.Candidate = d
		}
	}
	return p, recs
}

// reportRecords are a daemon's records of its reports (the attempts' outcomes, the dedup file's absence), normalized.
func reportRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var got []string
	port := strconv.Itoa(d.Opts.Port)
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if strings.Contains(l, "agent-events") || strings.Contains(l, "Cannot find a status file") {
			got = append(got, normalizeLog(l, d.Opts.RunDir, port))
		}
	}
	return got
}

// expectAttempts checks that each side tried `n` reports since the last call, the same requests, and the same
// records; the records accumulate over the daemon's log.
func expectAttempts(t *testing.T, stage string, p *Pair, recs [2]*proxyRecorder, n int) {
	t.Helper()
	var reqs [2][]string
	var records [2][]string
	for i, side := range p.Each() {
		reqs[i] = recs[i].take()
		if len(reqs[i]) != n {
			t.Errorf("%s: %s: %d report attempts, want %d", stage, side.Role, len(reqs[i]), n)
		}
		records[i] = reportRecords(t, side.Daemon)
	}
	if !slices.Equal(reqs[0], reqs[1]) {
		t.Errorf("%s: requests differ:\noracle %q\ncandidate %q", stage, reqs[0], reqs[1])
	}
	for _, r := range reqs[0] {
		if !strings.HasPrefix(r, "CONNECT agent-events.netdata.cloud:443 HTTP/1.1\r\n") {
			t.Errorf("%s: request %q", stage, r)
		}
	}
	if !slices.Equal(records[0], records[1]) {
		t.Errorf("%s: report records:\noracle:\n%s\ncandidate:\n%s", stage, strings.Join(records[0], "\n"),
			strings.Join(records[1], "\n"))
	}
}

// reportDifferences are what two agents' reports say differently beyond their records' (statusVolatile,
// statusDifferences): the process now and the live memory.
var reportDifferences = []Mask{
	{"agent_pid_now", "process"},
	{"host_memory_free_percent", "live"},
}

// expectPosts checks that each side posted `n` reports since the last call, the same requests (their length aside),
// and returns them.
func expectPosts(t *testing.T, stage string, p *Pair, recs [2]*proxyRecorder, n int) [2][]reportPost {
	t.Helper()
	var posts [2][]reportPost
	for i, side := range p.Each() {
		recs[i].take()
		posts[i] = recs[i].takePosts()
		if len(posts[i]) != n {
			t.Errorf("%s: %s: %d reports posted, want %d", stage, side.Role, len(posts[i]), n)
		}
	}
	for k := 0; k < n && k < len(posts[0]) && k < len(posts[1]); k++ {
		a, b := posts[0][k], posts[1][k]
		for _, post := range []reportPost{a, b} {
			if post.method != "POST" || post.uri != "/agent-events" || post.header.Get("Content-Type") != "application/json" ||
				post.header.Get("Content-Length") != strconv.Itoa(len(post.body)) {
				t.Errorf("%s: request %s %s %v", stage, post.method, post.uri, post.header)
			}
		}
		ha, hb := a.header.Clone(), b.header.Clone()
		ha.Del("Content-Length")
		hb.Del("Content-Length")
		if fmt.Sprint(ha) != fmt.Sprint(hb) {
			t.Errorf("%s: headers differ: oracle %v, candidate %v", stage, ha, hb)
		}
		var bodies [2]Value
		for i, body := range [][]byte{a.body, b.body} {
			v, err := ParseJSON(stepDurations.ReplaceAll(body, []byte("$1: D")))
			if err != nil {
				t.Fatalf("%s: body %q: %v", stage, body, err)
			}
			masks := append(append(append([]Mask{}, statusVolatile...), statusDifferences...), reportDifferences...)
			bodies[i] = ApplyMasks(v, masks)
		}
		for _, d := range Compare(bodies[0], bodies[1]) {
			t.Errorf("%s: body: %s", stage, d)
		}
	}
	return posts
}

// dedupFile reads a side's dedup file and checks it is C's table: its size, magic, version and slots' hash.
func dedupFile(t *testing.T, d *daemon.Daemon) []byte {
	t.Helper()
	b, err := os.ReadFile(filepath.Join(d.Opts.RunDir, "lib", "dedup-netdata.dat"))
	if err != nil {
		t.Fatalf("parity: %v", err)
	}
	le := func(at int) uint64 {
		var v uint64
		for i := 7; i >= 0; i-- {
			v = v<<8 | uint64(b[at+i])
		}
		return v
	}
	hash := uint64(14695981039346656037)
	for _, c := range b[24:] {
		hash = (hash ^ uint64(c)) * 1099511628211
	}
	if len(b) != 1224 || le(0) != 0x1DEDA9F17EDA7150 || le(8) != 1 || le(16) != hash {
		t.Errorf("parity: %s: dedup file of %d bytes, magic %#x, version %d, hash ok %v", d.Opts.RunDir, len(b), le(0),
			le(8), le(16) == hash)
	}
	return b
}

// setCrashReports rewrites a side's `[global] crash reports` for its next start.
func setCrashReports(t *testing.T, d *daemon.Daemon, from, to string) {
	t.Helper()
	path := filepath.Join(d.Opts.RunDir, "etc", "netdata.conf")
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	old := "crash reports = " + from + "\n"
	if !bytes.Contains(b, []byte(old)) {
		t.Fatalf("parity: %s has no %q", path, old)
	}
	if err := os.WriteFile(path, bytes.ReplaceAll(b, []byte(old), []byte("crash reports = "+to+"\n")), 0o644); err != nil {
		t.Fatal(err)
	}
}

// restartBoth restarts both sides one after the other.
func restartBoth(t *testing.T, p *Pair) {
	t.Helper()
	for _, side := range p.Each() {
		if err := side.Daemon.Restart(); err != nil {
			t.Fatalf("parity: restart %s: %v", side.Role, err)
		}
	}
}

// TestCrashReport (check `daemon.crash-report`, status file commit 7, brief `knowledge/brief-status-file-commit7.md`):
// the crash report's gate and its failure, each side's libcurl sent to a recorder that refuses the tunnel:
//   - all: a first run reports itself, a clean restart reports again (every start is reported);
//   - crashes: a clean restart is not reported, a start after a SIGKILL is;
//   - ci: with `CI` set (empty) a run reports only once the last one restarted more than once;
//
// each time the CONNECT requests (the same libcurl on both sides) and the records ("Failed to post...", the dedup
// file's "Cannot find a status file"), with the startup's MESSAGE_ID. With a CA both sides trust (tier B):
//   - posted: a first run's report reaches the recorder, which answers 500 (still a success): the POSTs compared
//     (headers, the body with the status file's masks), `agent.posts` 1, C's dedup file on both;
//   - dedup-across: each side's report of its own SIGKILL, then each agent restarted on the other's crashed record and
//     dedup file posts nothing.
func TestCrashReport(t *testing.T) {
	noStrayStatusFiles(t)
	t.Run("all-and-crashes", func(t *testing.T) {
		p, recs := reportPair(t, "all", reportOpts{})
		for _, side := range p.Each() {
			waitRunning(t, side.Daemon)
		}
		expectAttempts(t, "first run", p, recs, 1)
		restartBoth(t, p)
		expectAttempts(t, "clean restart", p, recs, 1)
		stopBoth(t, p)
		for _, side := range p.Each() {
			setCrashReports(t, side.Daemon, "all", "crashes")
		}
		restartBoth(t, p)
		expectAttempts(t, "crashes: clean restart", p, recs, 0)
		for _, side := range p.Each() {
			waitRunning(t, side.Daemon)
			if err := side.Daemon.Kill(); err != nil {
				t.Fatalf("parity: kill %s: %v", side.Role, err)
			}
		}
		restartBoth(t, p)
		expectAttempts(t, "crashes: after a kill", p, recs, 1)
		stopBoth(t, p)
	})
	// Tier B: each side trusts the harness CA (trustedWrap), its recorder takes the report
	t.Run("posted", func(t *testing.T) {
		p, recs := reportPair(t, "all", reportOpts{ca: newReportCA(t), status: http.StatusInternalServerError})
		for _, side := range p.Each() {
			waitRunning(t, side.Daemon)
		}
		// a first run reports itself; C reads no answer code, so a 500 is a success too
		expectPosts(t, "first run", p, recs, 1)
		expectAttempts(t, "first run", p, recs, 0)
		stopBoth(t, p)
		for _, side := range p.Each() {
			if got := statusMember(statusFile(t, side.Daemon), "agent.posts"); got != "1" {
				t.Errorf("%s: agent.posts %s, want 1", side.Role, got)
			}
			dedupFile(t, side.Daemon)
		}
	})
	// the dedup file across agents: each side's report of its own crash, then the other agent, started on that crashed
	// record and that dedup file, finds the report already posted
	t.Run("dedup-across", func(t *testing.T) {
		p, recs := reportPair(t, "all", reportOpts{ca: newReportCA(t), status: http.StatusOK})
		for _, side := range p.Each() {
			waitRunning(t, side.Daemon)
		}
		expectPosts(t, "first run", p, recs, 1)
		var crashed, dedups [2][]byte
		for i, side := range p.Each() {
			d := side.Daemon
			if err := d.Kill(); err != nil {
				t.Fatalf("parity: kill %s: %v", side.Role, err)
			}
			b, err := os.ReadFile(filepath.Join(d.Opts.RunDir, "lib", "status-netdata.json"))
			if err != nil {
				t.Fatal(err)
			}
			crashed[i] = b
			if err := d.Restart(); err != nil {
				t.Fatalf("parity: restart %s: %v", side.Role, err)
			}
			waitRunning(t, d)
		}
		expectPosts(t, "after a kill", p, recs, 1)
		stopBoth(t, p)
		for i, side := range p.Each() {
			dedups[i] = dedupFile(t, side.Daemon)
		}
		for i, side := range p.Each() {
			lib := filepath.Join(side.Daemon.Opts.RunDir, "lib")
			if err := os.WriteFile(filepath.Join(lib, "status-netdata.json"), crashed[1-i], 0o664); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(lib, "dedup-netdata.dat"), dedups[1-i], 0o664); err != nil {
				t.Fatal(err)
			}
		}
		restartBoth(t, p)
		for _, side := range p.Each() {
			waitRunning(t, side.Daemon)
		}
		expectPosts(t, "the other agent's report", p, recs, 0)
		expectAttempts(t, "the other agent's report", p, recs, 0)
		stopBoth(t, p)
	})
	t.Run("ci", func(t *testing.T) {
		p, recs := reportPair(t, "all", reportOpts{env: []string{"CI="}})
		for _, side := range p.Each() {
			waitRunning(t, side.Daemon)
		}
		expectAttempts(t, "ci: first run", p, recs, 0)
		restartBoth(t, p)
		expectAttempts(t, "ci: last run restarted once", p, recs, 0)
		restartBoth(t, p)
		expectAttempts(t, "ci: last run restarted twice", p, recs, 1)
		stopBoth(t, p)
	})
}
