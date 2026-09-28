// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bufio"
	"bytes"
	"fmt"
	"net"
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
// crash report attempt, stopped on the box.
type proxyRecorder struct {
	ln       net.Listener
	mu       sync.Mutex
	requests []string
}

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
	_, _ = c.Write([]byte("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n"))
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

// reportPair boots both sides as the status file checks do, with `crash reports = <mode>`, each side's reports going
// to its own recorder, and `env` added.
func reportPair(t *testing.T, mode string, env ...string) (*Pair, [2]*proxyRecorder) {
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
			GlobalExtra: "    crash reports = " + mode + "\n", Env: append([]string{rec.env()}, env...)}
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
// file's "Cannot find a status file"), with the startup's MESSAGE_ID.
func TestCrashReport(t *testing.T) {
	noStrayStatusFiles(t)
	t.Run("all-and-crashes", func(t *testing.T) {
		p, recs := reportPair(t, "all")
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
	t.Run("ci", func(t *testing.T) {
		p, recs := reportPair(t, "all", "CI=")
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
