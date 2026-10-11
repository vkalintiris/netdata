// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"crypto/tls"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// A child received over TLS on IPv6 (R110's run-only item 8, "a parent on [::1] with TLS"; T5's planted bug m30, the
// slot never taking its TLS flag): the node instances' `ingest.source` ends with `:SSL` on `[::1]`
// (api_v2_contexts.c:366-371) and `netdata-streaming`'s InSSL reads `SSL` with `::1` for the two addresses
// (function-netdata-streaming.c:214-221), from the receiver socket's peers and flag (nd_sock_socket_peers(),
// nd_sock_is_ssl(), rrdhost-status.c:218-219).

// niTLSChildPair starts a parent pair (dashPairOptions: no pulse, `ram` children, one tier; the bearer tokens) whose
// listeners take TLS on the agents' own port, on 127.0.0.1 (the launcher's) and on [::1] (`^SSL=optional`, the
// self-signed certificate in `etc/ssl`, as tlsPairOf), waits until the start windows are over (niReady), and connects
// the fixture child to each side over TLS on [::1] (stream.ConnectTLS, the certificate not verified) with the data
// fixture (streamChartsFixture), then 2.5 s (dashChildLinkAs' pause). It hands back the pair, each side's niSide (the
// child's connection opened in, its own port its peer) and the fixture's base. C's IPv6 listener is IPv6-only
// (listen-sockets.c:196, :226), so the port is the same number on both addresses.
func niTLSChildPair(t *testing.T) (*Pair, [2]niSide, int64) {
	t.Helper()
	key, cert := selfSigned(t)
	prepare := func(t *testing.T, runDir string) {
		fnWriteTokens(t, runDir)
		dir := filepath.Join(runDir, "etc", "ssl")
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatal(err)
		}
		for name, content := range map[string][]byte{"key.pem": key, "cert.pem": cert} {
			if err := os.WriteFile(filepath.Join(dir, name), content, 0o600); err != nil {
				t.Fatal(err)
			}
		}
	}
	opts := dashPairOptions(daemon.Options{BindTo: "127.0.0.1:{port}=" + tlsACL + "^SSL=optional [::1]:{port}=" +
		tlsACL + "^SSL=optional"})
	p := startPairWith(t, opts, parentIdentity, binaries(t), [2]string{}, [2]Role{"nitls-oracle", "nitls-candidate"},
		prepare)
	niReady(p)
	base := dashBase()
	var opened [2][2]int64
	var conns [2]*stream.Conn
	for i, side := range p.Each() {
		from := time.Now().Unix()
		_, port, _ := net.SplitHostPort(side.Daemon.Addr)
		conn, err := stream.ConnectTLS(net.JoinHostPort("::1", port), side.Daemon.StreamKey, childHost, stream.CapsLive,
			&tls.Config{InsecureSkipVerify: true})
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		t.Cleanup(func() { _ = conn.Close() })
		conns[i], opened[i] = conn, [2]int64{from, time.Now().Unix()}
	}
	for _, conn := range conns {
		streamChartsFixture(t, conn, base, qCharts)
	}
	time.Sleep(2500 * time.Millisecond)
	sides := niSides(p, opened)
	for i := range sides {
		sides[i].peer = niConnPort(conns[i])
	}
	return p, sides, base
}

// niTLSChildRow is `/api/v3/node_instances` of niTLSChildPair: as TestNodeInstancesAPI's `v3-ni` (localhost, then the
// child; niLocalFacts, niAgentFacts), the child's source `[::1]:LISTEN:SSL` from `[::1]:PEER:SSL` (niChildFactsOver).
var niTLSChildRow = v2Req{name: "v3-ni", target: "/api/v3/node_instances", status: "200",
	guard: dashGuard([]dashFact{niMembers, dashAbsent("nodes", "[2]")}, niLocalFacts(niShort),
		niChildFactsOver(niShort, 1, 1, "::1", ":SSL"), niAgentFacts)}

// niTLSChild (TestNodeInstancesAPI/tls): niTLSChildRow on niTLSChildPair, compared by nodeInstancesFamily (PEER the
// child's own port).
func niTLSChild(t *testing.T) {
	p, sides, _ := niTLSChildPair(t)
	t.Run(niTLSChildRow.name, func(t *testing.T) { compareV2(t, p, niTLSChildRow, nodeInstancesFamily(sides)) })
}

// fnStreamingTLSGuard is the guard of the oracle's table on niTLSChildPair: localhost initializing, then the child
// online on its receiver (CONNECTED, rrdhost-status.c:213), over TLS (InSSL `SSL`), on IPv6's loopback address, with
// the capabilities it negotiated (stream.CapsLive) (function-netdata-streaming.c:199-222).
var fnStreamingTLSGuard = fnStreamingRows(
	fnStreamingRow{"Node": strconv.Quote(parentIdentity.Hostname), "InReason": `"LOCALHOST"`,
		"InStatus": `"initializing"`},
	fnStreamingRow{"Node": strconv.Quote(childHost.Hostname), "InReason": `"CONNECTED"`, "InStatus": `"online"`,
		"InLocalIP": `"::1"`, "InRemoteIP": `"::1"`, "InSSL": `"SSL"`,
		"InCapabilities": `["VCAPS","HLABELS","CLABELS","INTERPOLATED"]`},
)

// fnStreamingTLSChild (TestFnNetdataStreaming/tls-child): the admin's table on niTLSChildPair
// (fnStreamingCompareWith), the child's InRemotePort held to its own port besides fnStreamingFacts (InLocalPort the
// parent's port).
func fnStreamingTLSChild(t *testing.T) {
	p, sides, _ := niTLSChildPair(t)
	fnStreamingCompareWith(t, p, fnStreamingAsk{target: "/api/v1/function?function=netdata-streaming",
		guard: fnStreamingTLSGuard, facts: func(i int, v Value, _ [2]int64) error {
			return fnStreamingTLSFacts(v, sides[i].peer)
		}})
}

// fnStreamingTLSFacts holds the TLS child's row (row 1) of a side's table to the child's own port, peer: its
// InRemotePort (socket-peers.c:22-50, function-netdata-streaming.c:218-219).
func fnStreamingTLSFacts(v Value, peer string) error {
	c, err := fnStreamingCell(v, 1, "InRemotePort")
	if err != nil {
		return err
	}
	if c.Text != peer {
		return fmt.Errorf("row 1: InRemotePort %s, want the child's own port %s", c, peer)
	}
	return nil
}
