// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"reflect"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// nodeLabelsRe finds a node's labels in a contexts v2 answer, after its machine GUID (nothing between them has
// braces: name, index, version, or the MCP header).
var nodeLabelsRe = regexp.MustCompile(`"(?:mg|machine_guid)":"([^"]+)"[^{}]*?"labels":(\{[^{}]*\})`)

// streamPathLabels cuts every node's labels out of a /api/v3/stream_path answer and returns them as maps, in node
// order: C prints labels in heap-address order. With cOnlyFor set, the C-only labels of that node are dropped
// (localhost's `_hw_*`, D49 point 3); every label named in maskValues compares by presence only.
func streamPathLabels(t *testing.T, b []byte, cOnlyFor string, maskValues ...string) ([]byte, []map[string]any) {
	t.Helper()
	var all []map[string]any
	out := nodeLabelsRe.ReplaceAllFunc(b, func(m []byte) []byte {
		sub := nodeLabelsRe.FindSubmatch(m)
		var labels map[string]any
		if err := json.Unmarshal(sub[2], &labels); err != nil {
			t.Fatalf("labels: %v: %s", err, sub[2])
		}
		if string(sub[1]) == cOnlyFor {
			for k := range labels {
				if cOnlyHostLabels.MatchString(k) {
					delete(labels, k)
				}
			}
		}
		for _, k := range maskValues {
			if _, ok := labels[k]; ok {
				labels[k] = "<masked>"
			}
		}
		all = append(all, labels)
		return append(bytes.TrimSuffix(m, sub[2]), "{}"...)
	})
	return out, all
}

// parentEntryTimes hides `since` and `first_time_t` of the parent's own stream path entries (the second each parent
// accepted a connection or started, and its retention start).
var parentEntryTimes = regexp.MustCompile(`("host_id":"` + parentIdentity.MachineGUID +
	`",\s*"node_id":[^,]*,\s*"claim_id":[^,]*,\s*"hops":-?\d+,\s*"since":)\d+(,\s*"first_time_t":)\d+`)

// entryTimes hides `since` and `first_time_t` of every stream path entry.
var entryTimes = regexp.MustCompile(`("since":)\d+(,\s*"first_time_t":)\d+`)

// compareStreamPath compares one request on two daemons: the raw answers byte for byte after the masks, with labels
// compared as maps.
func compareStreamPath(t *testing.T, name string, addrs [2]string, path string, times *regexp.Regexp, cOnly [2]string, maskLabels ...string) {
	t.Helper()
	var text [2][]byte
	var labels [2][]map[string]any
	for i, addr := range addrs {
		b, err := rawExchange(addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 10*time.Second)
		if err != nil {
			t.Fatalf("%s %s: %v", name, path, err)
		}
		b = times.ReplaceAll(maskTimings(maskRaw(b)), []byte("${1}0${2}0"))
		text[i], labels[i] = streamPathLabels(t, b, cOnly[i], maskLabels...)
	}
	if !bytes.Equal(text[0], text[1]) {
		t.Errorf("%s %s: differs\n%s", name, path, firstDifference(text[0], text[1]))
	}
	if !reflect.DeepEqual(labels[0], labels[1]) {
		t.Errorf("%s %s: labels differ\noracle:    %v\ncandidate: %v", name, path, labels[0], labels[1])
	}
}

// streamPathRequests are the requests of check `stream.cchild-path-api` for a child named `child` with machine GUID
// `guid` (spec `knowledge/spec-stream-path-api.md` §5.1). Context and window filters are scoped to the child: the
// candidate's localhost has no contexts yet (D51 point 5).
func streamPathRequests(child, guid string) []string {
	return []string{
		"/api/v3/stream_path",
		"/api/v3/stream_path?options=minify",
		"/api/v3/stream_path?options=debug",
		"/api/v3/stream_path?options=debug,minify",
		"/api/v3/stream_path?options=mcp",
		"/api/v3/stream_path?options=long-json-keys",
		"/api/v3/stream_path?options=debug%7Crfc3339&after=-10&before=0",
		"/api/v3/stream_path?nodes=" + child,
		"/api/v3/stream_path?nodes=" + guid,
		"/api/v3/stream_path?nodes=!parity-parent,*",
		"/api/v3/stream_path?nodes=nomatch",
		"/api/v3/stream_path?scope_nodes=nomatch",
		"/api/v3/stream_path?scope_nodes=parity-parent",
		"/api/v3/stream_path?nodes=nomatch&nodes=" + child,
		"/api/v3/stream_path?nodes=parity-parent%26nodes%3Dnomatch",
		"/api/v3/stream_path?nodes=" + child + "&contexts=nomatch",
		"/api/v3/stream_path?nodes=" + child + "&scope_contexts=nomatch",
		"/api/v3/stream_path?nodes=" + child + "&scope_contexts=netdata.*",
		"/api/v3/stream_path?nodes=" + child + "&after=1000000000&before=1000000100",
		"/api/v3/stream_path?timeout=-1",
		"/api/v3/stream_path?timeout=-1&nodes=nomatch",
		"/host/" + child + "/api/v3/stream_path",
		"/api/v3/stream_path/x",
	}
}

// waitOnline waits until a parent says the child's ingestion is online (its /api/v3/stream_info), then a little
// more for replication and the retention to settle.
func waitOnline(t *testing.T, addr, guid string) {
	t.Helper()
	deadline := time.Now().Add(60 * time.Second)
	for {
		b, err := rawExchange(addr, []byte("GET /api/v3/stream_info?machine_guid="+guid+" HTTP/1.1\r\n\r\n"), 5*time.Second)
		if err == nil && bytes.Contains(b, []byte(`"ingest_status":"online"`)) {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("%s: the child never came online: %s", addr, b)
		}
		time.Sleep(time.Second)
	}
	time.Sleep(5 * time.Second)
}

// TestCChildStreamPath gives each parent its own real C child (the same identity, compression off) and compares
// `/api/v3/stream_path` on both parents and each child's own view, while connected and after the children stop:
// check `stream.cchild-path-view`. The children start at different seconds, so every entry's times are masked, and
// each names its own parent in `_streams_to`.
func TestCChildStreamPath(t *testing.T) {
	const hostname, guid = "parity-cchild-view", "5a1e0000-0000-4000-8000-00000000c0aa"
	p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1,
		StreamExtra: fmt.Sprintf("\n[%s]\n    enable compression = no\n", guid)}, parentIdentity)
	var children [2]*daemon.Daemon
	for i, side := range p.Each() {
		child, err := daemon.Start(daemon.Options{
			Binary:       os.Getenv("PARITY_ORACLE"),
			RunDir:       runDir(t, Role("child-of-"+string(side.Role))),
			StorageTiers: 1,
			Identity:     &daemon.Identity{Hostname: hostname, StreamKey: "5a1e0000-0000-4000-8000-00000000c1ff", MachineGUID: guid},
			StreamTo:     &daemon.StreamTo{Destination: side.Daemon.Addr, APIKey: parentIdentity.StreamKey},
		})
		if err != nil {
			t.Fatalf("start child of %s: %v", side.Role, err)
		}
		children[i] = child
	}
	defer func() {
		for _, c := range children {
			if c != nil {
				_ = c.Stop()
			}
		}
	}()
	parents := [2]string{p.Oracle.Addr, p.Candidate.Addr}
	for _, addr := range parents {
		waitOnline(t, addr, guid)
	}
	cOnly := [2]string{parentIdentity.MachineGUID, ""}
	for _, path := range []string{"/api/v3/stream_path", "/api/v3/stream_path?options=minify"} {
		compareStreamPath(t, "connected parents", parents, path, entryTimes, cOnly, "_streams_to")
	}
	// the children are C on both sides: all their labels compare
	compareStreamPath(t, "children", [2]string{children[0].Addr, children[1].Addr}, "/api/v3/stream_path",
		entryTimes, [2]string{}, "_streams_to")
	for i, c := range children {
		_ = c.Stop()
		children[i] = nil
	}
	time.Sleep(3 * time.Second)
	compareStreamPath(t, "stale parents", parents, "/api/v3/stream_path", entryTimes, cOnly, "_streams_to")
	if t.Failed() {
		return
	}
	// the stale child entry names the parent with hops -1
	b, _ := rawExchange(p.Candidate.Addr, []byte("GET /api/v3/stream_path?nodes="+hostname+"&options=minify HTTP/1.1\r\n\r\n"), 5*time.Second)
	if !strings.Contains(string(b), `"state":"stale"`) || !strings.Contains(string(b), `"hops":-1`) {
		t.Errorf("candidate stale view: %s", httpBody(b))
	}
}
