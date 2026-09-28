// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
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
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// tlsACL is every feature a web listener serves.
const tlsACL = "dashboard|registry|badges|management|netdata.conf|streaming|mcp"

// selfSigned is a P-256 key and its self-signed certificate for localhost, as PEM.
func selfSigned(t *testing.T) (key, cert []byte) {
	t.Helper()
	k, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	tmpl := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "localhost"},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(24 * time.Hour),
		DNSNames: []string{"localhost"}, IPAddresses: []net.IP{net.ParseIP("127.0.0.1")}}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &k.PublicKey, k)
	if err != nil {
		t.Fatal(err)
	}
	pk, err := x509.MarshalPKCS8PrivateKey(k)
	if err != nil {
		t.Fatal(err)
	}
	return pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: pk}),
		pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})
}

// tlsListeners are a side's default, force and unix listeners, beside its readiness listener (`^SSL=optional`).
type tlsListeners struct{ optional, standard, force, unix string }

// tlsPair boots both sides with `files` (name → content) in `etc/ssl/` and `webExtra` in [web], each with a default
// and a force listener of its own.
func tlsPair(t *testing.T, webExtra string, files map[string][]byte) (*Pair, [2]tlsListeners) {
	t.Helper()
	p := &Pair{}
	var ls [2]tlsListeners
	bins := binaries(t)
	for i, role := range []Role{"tls-oracle", "tls-candidate"} {
		ports := freePorts(t, 2)
		id := parentIdentity
		o := daemon.Options{Binary: bins[i], RunDir: runDir(t, role), Identity: &id, DBMode: "alloc",
			StreamMemoryMode: "ram", StorageTiers: 1, WebExtra: webExtra, LogsExtra: "    level = debug\n",
			BindTo: fmt.Sprintf("127.0.0.1:{port}=%s^SSL=optional 127.0.0.1:%d 127.0.0.1:%d=%s^SSL=force "+
				"unix:{run}/tls.sock", tlsACL, ports[0], ports[1], tlsACL)}
		dir := filepath.Join(o.RunDir, "etc", "ssl")
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
		for name, content := range files {
			if err := os.WriteFile(filepath.Join(dir, name), content, 0o600); err != nil {
				t.Fatal(err)
			}
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
		ls[i] = tlsListeners{d.Addr, fmt.Sprintf("127.0.0.1:%d", ports[0]), fmt.Sprintf("127.0.0.1:%d", ports[1]),
			filepath.Join(o.RunDir, "tls.sock")}
	}
	return p, ls
}

// unixExchange sends `request` on a unix socket and returns the answer's status line.
func unixExchange(path string, request []byte) (string, error) {
	c, err := net.DialTimeout("unix", path, 5*time.Second)
	if err != nil {
		return "", err
	}
	defer c.Close()
	_ = c.SetDeadline(time.Now().Add(5 * time.Second))
	if _, err := c.Write(request); err != nil {
		return "", err
	}
	b, _ := io.ReadAll(c)
	head, _, _ := bytes.Cut(b, []byte("\r\n"))
	return string(head), nil
}

// serverCloses reads `conn` until the server closes it, or reports that it did not within `limit`.
func serverCloses(conn net.Conn, limit time.Duration) string {
	_ = conn.SetReadDeadline(time.Now().Add(limit))
	b, err := io.ReadAll(conn)
	if err != nil {
		return fmt.Sprintf("open after %s (%d bytes)", limit, len(b))
	}
	return fmt.Sprintf("closed (%d bytes)", len(b))
}

// tlsExchange sends `request` over TLS (no verification) and returns the answer up to the server's close or the
// timeout, and the connection's parameters.
func tlsExchange(addr string, request []byte, cfg *tls.Config) ([]byte, tls.ConnectionState, error) {
	c, err := tls.DialWithDialer(&net.Dialer{Timeout: 5 * time.Second}, "tcp", addr, cfg)
	if err != nil {
		return nil, tls.ConnectionState{}, err
	}
	defer c.Close()
	_ = c.SetDeadline(time.Now().Add(5 * time.Second))
	if _, err := c.Write(request); err != nil {
		return nil, c.ConnectionState(), err
	}
	b, _ := io.ReadAll(c)
	return b, c.ConnectionState(), nil
}

// tlsLogMasks hide the sockets and the errno of C's SSL error records, and the extra listeners' ports.
var tlsLogMasks = []logMask{
	{regexp.MustCompile(`local \[\[[^\]]*\]:\d+\] <-> remote \[\[[^\]]*\]:\d+\]`), "local [[L]:P] <-> remote [[R]:P]"},
	{regexp.MustCompile(`Errno \[\d+\]`), "Errno [N]"},
	{regexp.MustCompile(`127\.0\.0\.1:\d{4,5}`), "127.0.0.1:P"},
	{regexp.MustCompile(`(ip '127\.0\.0\.1' port )\d+`), "${1}P"},
	{regexp.MustCompile(`(from localhost port )\d+`), "${1}P"},
	{regexp.MustCompile(`(on socket )\d+( client 'localhost' port ')\d+`), "${1}N${2}P"},
	{responseBytesRe, "${1}N"},
}

// TestWebTLS (check `web.tls`, milestone 6, D96): the web server's TLS against C's.
//   - serve (commit 5): a certificate and key at C's default paths, an optional, a default and a force listener:
//     TLS requests on each (the answers' status lines, the negotiated version and cipher suite), plain requests
//     (redirected on the default and force listeners, served on the optional one), a plain STREAM on force, first
//     bytes that start a TLS handshake that fails; both logs.
//   - config: `tls version = 1.2`, a TLS 1.2 cipher list, a list OpenSSL rejects (reported, the context kept), no
//     files, a key that is not the certificate's and a key that is no PEM (no context: plain everywhere, no
//     redirect): a TLS request's outcome, a plain request on the default listener, both logs.
//   - stream (commit 6): a child streams over TLS to the default and the force listener, then closes: its chart's
//     answer and both logs (the receiver's records "https", the web side's after the takeover "http").
//   - refused-held (review R38a): with one web server thread, a TLS child refused after the takeover (this agent's
//     own machine GUID) that keeps its connection open: the refusal, then a plain request's answer, then the same
//     refusal on a plain connection; both logs (the errno C's records carry after a refusal).
//   - stream-force: a `^SSL=force` listener refuses a plain STREAM even with no certificate configured: 400 "HTTP
//     method requested is not supported..." and C's ERR record naming the child's `hostname=` (up to its `&`, else
//     "not available"). A plain GET is served there (no TLS context, no redirect).
func TestWebTLS(t *testing.T) {
	t.Run("serve", func(t *testing.T) {
		key, cert := selfSigned(t)
		p, ls := tlsPair(t, "", map[string][]byte{"key.pem": key, "cert.pem": cert})
		cfg := &tls.Config{InsecureSkipVerify: true}
		info := []byte("GET /api/v1/info HTTP/1.1\r\nHost: localhost\r\n\r\n")
		for name, pick := range map[string]func(tlsListeners) string{
			"optional": func(l tlsListeners) string { return l.optional },
			"default":  func(l tlsListeners) string { return l.standard },
			"force":    func(l tlsListeners) string { return l.force }} {
			var got [2]string
			for i := range p.Each() {
				b, st, err := tlsExchange(pick(ls[i]), info, cfg)
				if err != nil {
					t.Fatalf("%s: tls on %s: %v", p.Each()[i].Role, name, err)
				}
				head, _, _ := bytes.Cut(b, []byte("\r\n"))
				got[i] = fmt.Sprintf("%s version %x cipher %x", head, st.Version, st.CipherSuite)
			}
			if got[0] != got[1] {
				t.Errorf("tls on %s: oracle %q, candidate %q", name, got[0], got[1])
			}
			t.Logf("tls on %s: %s", name, got[0])
		}
		// in order, the failing handshakes over a second apart: C logs an SSL error once a second per thread
		for _, c := range []struct {
			name string
			req  string
			pick func(tlsListeners) string
			head bool
		}{
			{"plain-default", "GET /api/v1/info?x=1 HTTP/1.1\r\nHost: example:19999\r\n\r\n", func(l tlsListeners) string { return l.standard }, false},
			{"plain-force", "GET /index.html HTTP/1.1\r\nHost: example\r\n\r\n", func(l tlsListeners) string { return l.force }, false},
			{"plain-optional", "GET /api/v1/info HTTP/1.1\r\n\r\n", func(l tlsListeners) string { return l.optional }, true},
			{"plain-options", "OPTIONS /api/v1/info HTTP/1.1\r\nHost: example\r\n\r\n", func(l tlsListeners) string { return l.standard }, false},
			{"stream-force", "STREAM key=k&hostname=child-x&x=1 HTTP/1.1\r\n\r\n", func(l tlsListeners) string { return l.force }, false},
			{"first-byte-handshake", "\x16\x03\x01\x00\x05hello", func(l tlsListeners) string { return l.standard }, false},
			{"first-byte-05", "\x05hello\r\n\r\n", func(l tlsListeners) string { return l.standard }, false},
			{"first-byte-80", "\x80hello\r\n\r\n", func(l tlsListeners) string { return l.standard }, false},
		} {
			var got [2][]byte
			for i, side := range p.Each() {
				b, err := rawExchange(c.pick(ls[i]), []byte(c.req), 5*time.Second)
				if err != nil {
					t.Fatalf("%s: %s: %v", side.Role, c.name, err)
				}
				if c.head {
					b, _, _ = bytes.Cut(b, []byte("\r\n"))
				}
				got[i] = maskTimings(maskRaw(b))
			}
			if !bytes.Equal(got[0], got[1]) {
				t.Errorf("%s: responses differ\n%s", c.name, firstDifference(got[0], got[1]))
			}
			t.Logf("%s: %q", c.name, got[0])
			if strings.HasPrefix(c.name, "first-byte") {
				time.Sleep(1100 * time.Millisecond)
			}
		}
		// a unix client is never looked at: no redirect, no TLS
		var unix [2]string
		for i := range p.Each() {
			head, err := unixExchange(ls[i].unix, []byte("GET /api/v1/info HTTP/1.1\r\n\r\n"))
			if err != nil {
				t.Fatal(err)
			}
			unix[i] = head
		}
		if unix[0] != unix[1] {
			t.Errorf("unix: oracle %q, candidate %q", unix[0], unix[1])
		}
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		for _, l := range logLines(t, p.Oracle.Opts.RunDir, "daemon.log") {
			if strings.Contains(l, "SSL") {
				t.Logf("oracle: %s", normalizeLog(l, p.Oracle.Opts.RunDir, ""))
			}
		}
		compareLogFilesWith(t, p, tlsLogMasks)
	})
	t.Run("config", func(t *testing.T) {
		key, cert := selfSigned(t)
		other, _ := selfSigned(t)
		pair := map[string][]byte{"key.pem": key, "cert.pem": cert}
		for name, c := range map[string]struct {
			web   string
			files map[string][]byte
		}{
			"tls-1.2":     {"    tls version = 1.2\n", pair},
			"ciphers":     {"    tls version = 1.2\n    tls ciphers = ECDHE-ECDSA-AES128-GCM-SHA256\n", pair},
			"bad-ciphers": {"    tls ciphers = NOPE\n", pair},
			"no-files":    {"", nil},
			"mismatch":    {"", map[string][]byte{"key.pem": other, "cert.pem": cert}},
			"garbage-key": {"", map[string][]byte{"key.pem": []byte("not a key\n"), "cert.pem": cert}},
		} {
			t.Run(name, func(t *testing.T) {
				p, ls := tlsPair(t, c.web, c.files)
				var got [2]string
				for i := range p.Each() {
					b, st, err := tlsExchange(ls[i].standard, []byte("GET /api/v1/info HTTP/1.1\r\n\r\n"),
						&tls.Config{InsecureSkipVerify: true})
					head, _, _ := bytes.Cut(b, []byte("\r\n"))
					tlsOutcome := fmt.Sprintf("%s version %x cipher %x", head, st.Version, st.CipherSuite)
					if err != nil {
						tlsOutcome = "no tls"
					}
					plain, err := rawExchange(ls[i].standard, []byte("GET /api/v1/info HTTP/1.1\r\n\r\n"), 5*time.Second)
					if err != nil {
						t.Fatal(err)
					}
					head, _, _ = bytes.Cut(plain, []byte("\r\n"))
					got[i] = tlsOutcome + "; plain: " + string(head)
				}
				if got[0] != got[1] {
					t.Errorf("oracle %q, candidate %q", got[0], got[1])
				}
				t.Logf("%s", got[0])
				for _, side := range p.Each() {
					if err := side.Daemon.Stop(); err != nil {
						t.Fatalf("stop %s: %v", side.Role, err)
					}
				}
				compareLogFilesWith(t, p, tlsLogMasks)
			})
		}
	})
	// the first-request timeout still applies while a handshake stalls, and to a TLS client that sends nothing
	t.Run("timeouts", func(t *testing.T) {
		key, cert := selfSigned(t)
		p, ls := tlsPair(t, "    timeout for first request = 2\n", map[string][]byte{"key.pem": key, "cert.pem": cert})
		for name, open := range map[string]func(addr string) (net.Conn, error){
			"stalled-handshake": func(addr string) (net.Conn, error) {
				c, err := net.DialTimeout("tcp", addr, 5*time.Second)
				if err == nil {
					_, err = c.Write([]byte("\x16\x03\x01"))
				}
				return c, err
			},
			"idle-after-handshake": func(addr string) (net.Conn, error) {
				return tls.DialWithDialer(&net.Dialer{Timeout: 5 * time.Second}, "tcp", addr,
					&tls.Config{InsecureSkipVerify: true})
			},
		} {
			var got [2]string
			for i := range p.Each() {
				c, err := open(ls[i].standard)
				if err != nil {
					t.Fatalf("%s: %v", name, err)
				}
				got[i] = serverCloses(c, 8*time.Second)
				c.Close()
			}
			if got[0] != got[1] {
				t.Errorf("%s: oracle %q, candidate %q", name, got[0], got[1])
			}
			t.Logf("%s: %s", name, got[0])
		}
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		compareLogFilesWith(t, p, tlsLogMasks)
	})
	t.Run("stream", func(t *testing.T) {
		key, cert := selfSigned(t)
		p, ls := tlsPair(t, "", map[string][]byte{"key.pem": key, "cert.pem": cert})
		children := []struct {
			host stream.HostInfo
			pick func(tlsListeners) string
		}{
			{stream.HostInfo{Hostname: "tls-child-a", MachineGUID: "b6b6b6b6-6666-4666-8666-00000000000a"},
				func(l tlsListeners) string { return l.standard }},
			{stream.HostInfo{Hostname: "tls-child-b", MachineGUID: "b6b6b6b6-6666-4666-8666-00000000000b"},
				func(l tlsListeners) string { return l.force }},
		}
		for _, c := range children {
			var got [2]string
			for i, side := range p.Each() {
				conn, err := stream.ConnectTLS(c.pick(ls[i]), side.Daemon.StreamKey, c.host, stream.CapsLive,
					&tls.Config{InsecureSkipVerify: true})
				if err != nil {
					t.Fatalf("%s: %s: %v", side.Role, c.host.Hostname, err)
				}
				now := time.Now().Unix()
				conn.Linef("CHART 'tls.c' '' 'title' 'units' 'family' 'tls.c' line 1000 1 '' tls corpus")
				conn.Linef("DIMENSION 'd' '' absolute 1 1 ''")
				for t := now - 5; t <= now; t++ {
					conn.Linef("BEGIN2 'tls.c' 1 %d #", t)
					conn.Linef("SET2 'd' %d %d A", t%100, t%100)
					conn.Linef("END2")
				}
				if err := conn.Flush(); err != nil {
					t.Fatal(err)
				}
				// the chart answers once the receiver stored it; the polls stay out of the log comparison
				deadline := time.Now().Add(20 * time.Second)
				for {
					b, err := rawExchange(side.Daemon.Addr, []byte("GET /host/"+c.host.Hostname+
						"/api/v1/chart?chart=tls.c&harness=wait HTTP/1.1\r\n\r\n"), 5*time.Second)
					head, _, _ := bytes.Cut(b, []byte("\r\n"))
					got[i] = string(head)
					if (err == nil && strings.Contains(got[i], " 200 ")) || time.Now().After(deadline) {
						break
					}
					time.Sleep(200 * time.Millisecond)
				}
				_ = conn.Close()
			}
			if got[0] != got[1] {
				t.Errorf("%s: oracle %q, candidate %q", c.host.Hostname, got[0], got[1])
			}
			t.Logf("%s: %s", c.host.Hostname, got[0])
		}
		// the receivers see the closes
		time.Sleep(2 * time.Second)
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		// a client hello arrives in one readable event or two: the receptions compare as the connects do
		receptions := logMask{regexp.MustCompile(`stopped after (\d+) connects, (\d+) disconnects \(max concurrent ` +
			`(\d+)\), \d+ receptions`), "stopped after ${1} connects, ${2} disconnects (max concurrent ${3}), ${1} receptions"}
		compareLogFilesWith(t, p, append(slices.Clone(tlsLogMasks), receptions))
	})
	// a TLS client and a TLS child that go away without a close_notify (the client in the middle of its request)
	t.Run("raw-close", func(t *testing.T) {
		key, cert := selfSigned(t)
		p, ls := tlsPair(t, "", map[string][]byte{"key.pem": key, "cert.pem": cert})
		for i, side := range p.Each() {
			raw, err := net.DialTimeout("tcp", ls[i].standard, 5*time.Second)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			c := tls.Client(raw, &tls.Config{InsecureSkipVerify: true})
			_ = c.SetDeadline(time.Now().Add(5 * time.Second))
			if err := c.Handshake(); err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			if _, err := c.Write([]byte("GET /api/v1/info HTTP/1.1\r\n")); err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			time.Sleep(500 * time.Millisecond)
			_ = raw.Close()
			// a child, once its chart is stored
			raw, err = net.DialTimeout("tcp", ls[i].standard, 5*time.Second)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			host := stream.HostInfo{Hostname: "raw-child", MachineGUID: "b6b6b6b6-6666-4666-8666-0000000000cc"}
			conn, err := stream.ConnectOn(tls.Client(raw, &tls.Config{InsecureSkipVerify: true}),
				side.Daemon.StreamKey, host, stream.CapsLive)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			now := time.Now().Unix()
			conn.Linef("CHART 'tls.r' '' 'title' 'units' 'family' 'tls.r' line 1000 1 '' tls corpus")
			conn.Linef("DIMENSION 'd' '' absolute 1 1 ''")
			conn.Linef("BEGIN2 'tls.r' 1 %d #", now)
			conn.Linef("SET2 'd' 1 1 A")
			conn.Linef("END2")
			if err := conn.Flush(); err != nil {
				t.Fatal(err)
			}
			deadline := time.Now().Add(20 * time.Second)
			for {
				b, err := rawExchange(side.Daemon.Addr, []byte("GET /host/raw-child/api/v1/chart?chart=tls.r"+
					"&harness=wait HTTP/1.1\r\n\r\n"), 5*time.Second)
				if (err == nil && bytes.Contains(b, []byte(" 200 "))) || time.Now().After(deadline) {
					break
				}
				time.Sleep(200 * time.Millisecond)
			}
			_ = raw.Close()
		}
		time.Sleep(2 * time.Second)
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		compareLogFilesWith(t, p, tlsLogMasks)
	})
	// a child refused after the takeover that keeps its connection open does not hold up the web server's thread
	t.Run("refused-held", func(t *testing.T) {
		key, cert := selfSigned(t)
		p, ls := tlsPair(t, "    web server threads = 1\n", map[string][]byte{"key.pem": key, "cert.pem": cert})
		var got [2]string
		for i, side := range p.Each() {
			c, err := tls.DialWithDialer(&net.Dialer{Timeout: 5 * time.Second}, "tcp", ls[i].standard,
				&tls.Config{InsecureSkipVerify: true})
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			// this agent's own machine GUID: "machine UUID is my own", refused after the takeover
			req := fmt.Sprintf("STREAM key=%s&hostname=held&registry_hostname=held&machine_guid=%s&update_every=1"+
				"&os=linux&ver=1 HTTP/1.1\r\n\r\n", side.Daemon.StreamKey, parentIdentity.MachineGUID)
			if _, err := c.Write([]byte(req)); err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			_ = c.SetReadDeadline(time.Now().Add(3 * time.Second))
			reply, _ := io.ReadAll(c)
			b, err := rawExchange(side.Daemon.Addr, []byte("GET /api/v1/info HTTP/1.1\r\n\r\n"), 5*time.Second)
			head, _, _ := bytes.Cut(b, []byte("\r\n"))
			// the same refusal on a plain connection
			plain, perr := rawExchange(side.Daemon.Addr, []byte(req), 5*time.Second)
			got[i] = fmt.Sprintf("refusal %q, then %q (%v); plain %q (%v)", reply, head, err, plain, perr)
			_ = c.Close()
		}
		if got[0] != got[1] {
			t.Errorf("oracle %s\ncandidate %s", got[0], got[1])
		}
		t.Log(got[0])
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		compareLogFilesWith(t, p, tlsLogMasks)
	})
	t.Run("stream-force", func(t *testing.T) {
		opts := daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, LogsExtra: "    level = debug\n",
			BindTo: "127.0.0.1:{port}=dashboard|registry|badges|management|netdata.conf|streaming|mcp^SSL=force"}
		p := StartPair(t, opts, parentIdentity)
		requests := []string{
			"STREAM key=%s&hostname=forced-child&registry_hostname=forced-child&machine_guid=" +
				"b6b6b6b6-3333-4333-8333-000000000001&update_every=1&os=linux&ver=1 HTTP/1.1\r\n" +
				"User-Agent: netdata/v0\r\nAccept: */*\r\n\r\n",
			"STREAM key=%s&hostname=last-param HTTP/1.1\r\n\r\n",
			"GET /api/v1/info HTTP/1.1\r\n\r\n",
		}
		for i, format := range requests {
			var got [2][]byte
			for s, side := range p.Each() {
				req := format
				if strings.Contains(format, "%s") {
					req = fmt.Sprintf(format, side.Daemon.StreamKey)
				}
				b, err := rawExchange(side.Daemon.Addr, []byte(req), 10*time.Second)
				if err != nil {
					t.Fatalf("%s: request %d: %v", side.Role, i, err)
				}
				head, _, _ := bytes.Cut(b, []byte("\r\n"))
				if strings.HasPrefix(format, "GET") {
					// the info body is `api.v1-info`'s; here only that it is served
					b = head
				}
				got[s] = maskTimings(maskRaw(b))
			}
			if !bytes.Equal(got[0], got[1]) {
				t.Errorf("request %d: responses differ\n%s", i, firstDifference(got[0], got[1]))
			}
		}
		for _, side := range p.Each() {
			if err := side.Daemon.Stop(); err != nil {
				t.Fatalf("stop %s: %v", side.Role, err)
			}
		}
		var records [2][]string
		for i, side := range p.Each() {
			port := strconv.Itoa(side.Daemon.Opts.Port)
			for _, l := range logLines(t, side.Daemon.Opts.RunDir, "daemon.log") {
				if strings.Contains(l, "please enable the SSL on child") {
					records[i] = append(records[i], normalizeLog(l, side.Daemon.Opts.RunDir, port))
				}
			}
			slices.Sort(records[i])
		}
		if !slices.Equal(records[0], records[1]) {
			t.Errorf("refusal records:\noracle:\n%s\ncandidate:\n%s", strings.Join(records[0], "\n"),
				strings.Join(records[1], "\n"))
		}
		if len(records[0]) != 2 {
			t.Errorf("the oracle logged %d refusals, want 2", len(records[0]))
		}
	})
}
