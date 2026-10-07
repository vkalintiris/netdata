// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"fmt"
	"regexp"
	"strconv"
	"testing"
)

// The `netdata-streaming` built-in (milestone 10 commit 12): C's table of every host's status
// (web/api/functions/function-netdata-streaming.c:21-1014), which reads the host status of every host
// (rrdhost_status, :77). `fn.builtins` keeps the admin's call as a deviation guard until then (fnBuiltinsPending).

// TestFnNetdataStreaming (check `fn.netdata-streaming`, milestone 10 commit 12): on `fn.stream`'s `calls` topology
// (two parents, a real C child each, the child's vnode released after it), the admin's
// `/api/v1/function?function=netdata-streaming` on each parent once the cases played.
func TestFnNetdataStreaming(t *testing.T) {
	topo := fnStreamCalls()
	topo.compare = fnStreamingCompare
	runFnStream(t, topo)
}

// fnStreamingTx is the admin's call's transaction (fnStreamTx's scheme, beyond the `calls` cases' numbers).
var fnStreamingTx = fnStreamTx(0x100)

// fnStreamingExpiresRe is fnBuiltinsExpires' rendering of the expiry, which the JSON parser needs as a string.
var fnStreamingExpiresRe = regexp.MustCompile(`"expires":(NOW[+-][0-9]+)`)

// fnStreamingDoc is a response's body as JSON, its expiry rendered as its distance from the head's Date (the
// handler writes now + 10, :1010; fnBuiltinsExpires).
func fnStreamingDoc(resp []byte) ([]byte, Value, error) {
	body := fnBuiltinsExpires(httpBody(resp), fnHTTPDate(resp))
	body = fnStreamingExpiresRe.ReplaceAll(body, []byte(`"expires":"$1"`))
	v, err := ParseJSON(body)
	return body, v, err
}

// fnStreamingVolatile are the table's columns whose cells (`data.[].[<index>]`) and maxima (`columns.<name>.max`)
// differed C against C, by name: the retention's ends and length (:155-167), the status changes' times and ages
// (:183-196, :233-246) and each inbound connection's ephemeral ports (:214-218). Only their numbers other than 0 are
// masked (fnStreamingMasks): localhost's retention start, length and ports read 0 or null in every C run. The
// outbound ports, traffic and attempts stay compared: the parents stream nowhere, so C writes them 0 or null.
var fnStreamingVolatile = []string{
	"dbFrom", "dbTo", "dbDuration",
	"InSince", "InAge", "OutSince", "OutAge",
	"InLocalPort", "InRemotePort",
}

// fnStreamingColumns are a table's column indexes by name (`columns.<name>.index`).
func fnStreamingColumns(v Value) map[string]int {
	out := map[string]int{}
	cols, err := dashMember(v, "columns")
	if err != nil {
		return out
	}
	for _, c := range cols.Members {
		if i, err := dashMember(c.Value, "index"); err == nil {
			if n, err := strconv.Atoi(i.Text); err == nil {
				out[c.Key] = n
			}
		}
	}
	return out
}

// fnStreamingMasks are the masks of v's cells of the columns names (`data.[r].[i]`, at each column's own index) and
// of their maxima (`columns.<name>.max`) that hold a number other than 0: a 0 or a null stays compared.
func fnStreamingMasks(v Value, names []string) []Mask {
	cols := fnStreamingColumns(v)
	data, _ := dashMember(v, "data")
	var masks []Mask
	for _, n := range names {
		i, ok := cols[n]
		if !ok {
			continue
		}
		for r, row := range data.Items {
			if i < len(row.Items) && fnStreamingNonZero(row.Items[i]) {
				masks = append(masks, Mask{fmt.Sprintf("data.[%d].[%d]", r, i), n})
			}
		}
		if top, err := dashAt(v, "columns", n, "max"); err == nil && fnStreamingNonZero(top) {
			masks = append(masks, Mask{"columns." + n + ".max", n})
		}
	}
	return masks
}

// fnStreamingNonZero tells whether v is a number other than 0.
func fnStreamingNonZero(v Value) bool {
	x, err := strconv.ParseFloat(v.Text, 64)
	return v.Kind == KindNumber && err == nil && x != 0
}

// fnStreamingCell is row r's cell of column name.
func fnStreamingCell(v Value, r int, name string) (Value, error) {
	data, err := dashMember(v, "data")
	if err != nil {
		return Value{}, err
	}
	i, ok := fnStreamingColumns(v)[name]
	if !ok || r >= len(data.Items) || i >= len(data.Items[r].Items) {
		return Value{}, fmt.Errorf("no cell %s of row %d", name, r)
	}
	return data.Items[r].Items[i], nil
}

// fnStreamingGuard checks the oracle's table: a table of three hosts in creation order (dfe over rrdhost_root_index,
// :75): the parent's localhost, still initializing with no metric of its own (rrdhost-status.c:124-130, :172-173),
// then the child and its vnode, each online on a receiver of its own, so each one's inbound reason is its handshake's
// (rrdhost-status.c:213, :225-230: the receiver before the virtual flag; :199-204; stream-handshake.c:10-12).
func fnStreamingGuard(v Value) error {
	if err := dashIs(`"table"`, "type")(v); err != nil {
		return err
	}
	data, err := dashMember(v, "data")
	if err != nil {
		return err
	}
	want := [][3]string{
		{parentIdentity.Hostname, "LOCALHOST", "initializing"},
		{rchildHostname, "CONNECTED", "online"},
		{rvName, "CONNECTED", "online"},
	}
	if len(data.Items) != len(want) {
		return fmt.Errorf("%d rows, want %d", len(data.Items), len(want))
	}
	for r, w := range want {
		var got [3]string
		for k, col := range []string{"Node", "InReason", "InStatus"} {
			c, err := fnStreamingCell(v, r, col)
			if err != nil {
				return err
			}
			got[k] = c.Text
		}
		if got != w {
			return fmt.Errorf("row %d: node, reason, status %q, want %q", r, got, w)
		}
	}
	return nil
}

// fnStreamingCompare asks each parent for the admin's netdata-streaming table and compares the answers: the oracle's
// status and guard first, the heads (fnHTTPMask, the length aside), the bodies as ordered JSON with the volatile
// columns' numbers other than 0 masked by name, their layouts.
func fnStreamingCompare(t *testing.T, p *Pair, _ [2]*fnStreamSide) {
	t.Run("netdata-streaming", func(t *testing.T) {
		req := fnHTTPGet("/api/v1/function?function=netdata-streaming", fnStreamingTx, fnBuiltinsUsers[2].header)
		var raw [2][]byte
		for i, side := range p.Each() {
			b, err := rawExchange(side.Daemon.Addr, req, fnWait)
			if err != nil {
				t.Fatalf("%s: %v", side.Role, err)
			}
			raw[i] = b
		}
		if s := fnStatusLine(raw[0]); s != "HTTP/1.1 200 OK" {
			t.Fatalf("oracle: answered %q", truncateBytes(raw[0]))
		}
		var body [2][]byte
		var doc [2]Value
		var err error
		if body[0], doc[0], err = fnStreamingDoc(raw[0]); err != nil {
			t.Fatalf("oracle: %v: %s", err, truncateBytes(raw[0]))
		}
		if err := fnStreamingGuard(doc[0]); err != nil {
			t.Fatalf("oracle: %v: %s", err, truncateBytes(body[0]))
		}
		var head [2]string
		for i := range raw {
			h, _, _ := bytes.Cut(raw[i], []byte("\r\n\r\n"))
			head[i] = contentLengthRe.ReplaceAllString(fnHTTPMask(h), "Content-Length: <masked>")
		}
		if head[0] != head[1] {
			t.Errorf("heads differ\noracle:    %q\ncandidate: %q", head[0], head[1])
		}
		if body[1], doc[1], err = fnStreamingDoc(raw[1]); err != nil {
			t.Fatalf("candidate: %v: %s", err, truncateBytes(raw[1]))
		}
		o := ApplyMasks(doc[0], fnStreamingMasks(doc[0], fnStreamingVolatile))
		c := ApplyMasks(doc[1], fnStreamingMasks(doc[1], fnStreamingVolatile))
		for _, d := range Compare(o, c) {
			t.Errorf("%s", d)
		}
		if l := v2Layouts(body, false); l != "" {
			t.Errorf("%s", l)
		}
		if t.Failed() {
			t.Logf("oracle:\n%s", body[0])
		}
	})
}
