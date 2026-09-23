// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"errors"
	"net"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// rawExchange sends request as-is and returns every byte the server sends back until it closes the connection
// or stays silent for timeout (a request C never completes shows up as "<timeout>").
func rawExchange(addr string, request []byte, timeout time.Duration) ([]byte, error) {
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		return nil, err
	}
	defer conn.Close()
	if _, err := conn.Write(request); err != nil {
		return nil, err
	}
	var out bytes.Buffer
	buf := make([]byte, 64*1024)
	for {
		if err := conn.SetReadDeadline(time.Now().Add(timeout)); err != nil {
			return nil, err
		}
		n, err := conn.Read(buf)
		out.Write(buf[:n])
		if err != nil {
			var ne net.Error
			if errors.As(err, &ne) && ne.Timeout() {
				out.WriteString("<timeout>")
			}
			return out.Bytes(), nil
		}
	}
}

// rawMasks hide the header values that differ between any two responses (clock and random transaction ids).
var rawMasks = []*regexp.Regexp{
	regexp.MustCompile(`(?m)^(Date|Expires): [^\r]*`),
	regexp.MustCompile(`(?m)^X-Transaction-ID: [0-9a-f]*`),
}

func maskRaw(b []byte) []byte {
	for _, re := range rawMasks {
		b = re.ReplaceAllFunc(b, func(m []byte) []byte {
			name, _, _ := bytes.Cut(m, []byte(":"))
			return append(name, []byte(": <masked>")...)
		})
	}
	return b
}

// TestRawProtocol sends byte-exact requests that exercise the HTTP layer (method checks, OPTIONS, header
// parsing, limits) and compares the complete raw responses.
func TestRawProtocol(t *testing.T) {
	p := StartPair(t, daemon.Options{}, parentIdentity)
	cases := map[string][]byte{
		"unsupported-method":                   []byte("PATCH / HTTP/1.1\r\n\r\n"),
		"lowercase-method":                     []byte("get / HTTP/1.1\r\n\r\n"),
		"head-is-unsupported":                  []byte("HEAD /api/v1/info HTTP/1.1\r\n\r\n"),
		"options":                              []byte("OPTIONS /api/v1/info HTTP/1.1\r\nOrigin: http://x\r\n\r\n"),
		"options-mcp":                          []byte("OPTIONS /mcp HTTP/1.1\r\n\r\n"),
		"header-without-colon-never-completes": []byte("GET / HTTP/1.1\r\nFoo\r\n\r\n"),
		"uri-too-long": append(append([]byte("GET /"), bytes.Repeat([]byte("a"), 1<<20)...),
			[]byte(" HTTP/1.1\r\n\r\n")...),
	}
	compareRaw(t, p, cases)
}

// compareRaw sends each request to both daemons and compares the masked raw responses.
func compareRaw(t *testing.T, p *Pair, cases map[string][]byte) {
	t.Helper()
	for name, request := range cases {
		t.Run(name, func(t *testing.T) {
			var got [2][]byte
			for i, side := range p.Each() {
				b, err := rawExchange(side.Daemon.Addr, request, 2*time.Second)
				if err != nil {
					t.Fatalf("%s: %v", side.Role, err)
				}
				got[i] = maskRaw(b)
			}
			if !bytes.Equal(got[0], got[1]) {
				t.Errorf("responses differ\noracle:    %q\ncandidate: %q", truncateBytes(got[0]), truncateBytes(got[1]))
			}
		})
	}
}

// TestStaticAndRouting covers URL routing (API versions and commands, dashboard version prefixes, host switching)
// and static files, with both daemons serving the oracle's web directory.
func TestStaticAndRouting(t *testing.T) {
	webDir := oracleWebDir(t)
	p := StartPair(t, daemon.Options{WebDir: webDir}, parentIdentity)
	get := func(path string) []byte { return []byte("GET " + path + " HTTP/1.1\r\n\r\n") }
	paths := []string{
		"/", "/index.html", "/v3", "/v3/", "/v3/?x=1", "/v2/", "/v2/index.html", "/nonexistent",
		"/nonexistent.js", "/static/", "/netdata-swagger.json", "/bad%20name", "/a/../index.html",
		"/host/other/api/v1/info", "/host/", "/host/parity-parent", "/host/parity-parent/v3?y",
		"/api", "/api/v9", "/api/v1", "/api/v1//info", "/api/v1/info/x", "/api/v1/nope%2Fx", "/v1/v2/",
		"/node/5A1E0000-0000-4000-8000-0000000000AA/x.js", "/host/localhost/x.js", "/host/localhost",
		"/node/00000000-0000-0000-0000-000000000000/x.js", "/host/5A1E00000000400080000000000000AA/x.js",
		"/host/5a1e0000-0000-4000-8000-0000000000aaxyz/x.js",
	}
	cases := map[string][]byte{}
	for _, path := range paths {
		cases[strings.ReplaceAll(path, "/", "_")] = get(path)
	}
	compareRaw(t, p, cases)
}

func truncateBytes(b []byte) string {
	s := string(b)
	if len(s) > 600 {
		return s[:600] + "..."
	}
	return strings.ToValidUTF8(s, "?")
}

// oracleWebDir is the web directory of the oracle's install, which both daemons serve in the static-file checks.
func oracleWebDir(t *testing.T) string {
	t.Helper()
	webDir := filepath.Join(filepath.Dir(os.Getenv("PARITY_ORACLE")), "..", "share", "netdata", "web")
	if _, err := os.Stat(filepath.Join(webDir, "index.html")); err != nil {
		t.Fatalf("parity: the oracle's web directory: %v", err)
	}
	return webDir
}

// largestFile is the web path of the biggest file under dir.
func largestFile(t *testing.T, dir string) string {
	t.Helper()
	var best string
	var size int64
	err := filepath.WalkDir(dir, func(path string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		info, err := d.Info()
		if err == nil && info.Size() > size {
			best, size = path, info.Size()
		}
		return err
	})
	if err != nil || best == "" {
		t.Fatalf("parity: no file under %s: %v", dir, err)
	}
	rel, _ := filepath.Rel(dir, best)
	return "/" + filepath.ToSlash(rel)
}

// TestRequestDuringLargeResponse sends a second request while the response to the first (the largest static file)
// is still being written: both daemons must answer both, in order.
func TestRequestDuringLargeResponse(t *testing.T) {
	webDir := oracleWebDir(t)
	big := largestFile(t, webDir)
	p := StartPair(t, daemon.Options{WebDir: webDir}, parentIdentity)
	var got [2][]byte
	for i, side := range p.Each() {
		conn, err := net.Dial("tcp", side.Daemon.Addr)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		if _, err := conn.Write([]byte("GET " + big + " HTTP/1.1\r\nConnection: keep-alive\r\n\r\n")); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		time.Sleep(200 * time.Millisecond)
		if _, err := conn.Write([]byte("GET /nonexistent.js HTTP/1.1\r\n\r\n")); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		var out bytes.Buffer
		buf := make([]byte, 1<<20)
		for {
			_ = conn.SetReadDeadline(time.Now().Add(3 * time.Second))
			n, err := conn.Read(buf)
			out.Write(buf[:n])
			if err != nil {
				break
			}
		}
		conn.Close()
		got[i] = maskRaw(out.Bytes())
	}
	for i, side := range []string{"oracle", "candidate"} {
		if !bytes.HasSuffix(got[i], []byte("File does not exist, or is not accessible: nonexistent.js")) {
			t.Errorf("%s: the second request was not answered", side)
		}
	}
	if !bytes.Equal(got[0], got[1]) {
		t.Errorf("responses differ (lengths %d and %d)\noracle tail:    %q\ncandidate tail: %q", len(got[0]), len(got[1]),
			truncateBytes(got[0][max(0, len(got[0])-400):]), truncateBytes(got[1][max(0, len(got[1])-400):]))
	}
}

// TestNoReadWhileWriting sends junk behind a request whose response the client does not read: the server must stop
// reading (C polls only for writing while it sends), so the client's writes block after the socket buffers fill.
func TestNoReadWhileWriting(t *testing.T) {
	webDir := oracleWebDir(t)
	big := largestFile(t, webDir)
	p := StartPair(t, daemon.Options{WebDir: webDir}, parentIdentity)
	const junk = 64 << 20
	for _, side := range p.Each() {
		conn, err := net.Dial("tcp", side.Daemon.Addr)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		if _, err := conn.Write([]byte("GET " + big + " HTTP/1.1\r\nConnection: keep-alive\r\n\r\n")); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		time.Sleep(200 * time.Millisecond)
		_ = conn.SetWriteDeadline(time.Now().Add(2 * time.Second))
		written, _ := conn.Write(make([]byte, junk))
		conn.Close()
		if written >= 16<<20 {
			t.Errorf("%s accepted %d bytes of junk while writing a response; want backpressure", side.Role, written)
		}
	}
}

// TestIgnoredSignals sends signals C neither handles nor lets kill it (it blocks everything but the deadly ones):
// both daemons must keep serving.
func TestIgnoredSignals(t *testing.T) {
	p := StartPair(t, daemon.Options{}, parentIdentity)
	for _, side := range p.Each() {
		for _, sig := range []syscall.Signal{syscall.SIGUSR1, syscall.SIGALRM, syscall.SIGPIPE} {
			if err := syscall.Kill(side.Daemon.PID(), sig); err != nil {
				t.Fatalf("%s: kill %v: %v", side.Role, sig, err)
			}
		}
	}
	time.Sleep(500 * time.Millisecond)
	for _, side := range p.Each() {
		b, err := rawExchange(side.Daemon.Addr, []byte("GET /api/v1/info HTTP/1.1\r\n\r\n"), 2*time.Second)
		if err != nil || !bytes.HasPrefix(b, []byte("HTTP/1.1 200 OK")) {
			t.Errorf("%s stopped serving after ignored signals: %v %q", side.Role, err, truncateBytes(b))
		}
	}
}
