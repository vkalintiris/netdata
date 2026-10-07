// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// capabilityDiff is a capability the Rust agent turns off until its subsystem is ported (D92.1): C's version and
// enabled flag, the candidate's, and the milestone that closes the difference.
type capabilityDiff struct{ c, rust, closes string }

// capabilityDiffs are those capabilities by name, as the oracle (built with ML, D2) answers under the harness's
// configuration (`[ml]` and `[health]` off). A milestone that ports the subsystem deletes its entry; a candidate that matches C on a listed
// capability fails until then. They apply only when the candidate is another binary than the oracle (sameBinary).
// `health` is not listed: off, both say 2/false; on, both say 2/true (the node-instance list this answer prints,
// aclk_capas.c:51, through database/contexts/api_v2_contexts.c:440; `health.api` `endpoints` asks it with health on).
var capabilityDiffs = map[string]capabilityDiff{
	"proto":      {"1/true", "1/false", "M11 Cloud"},
	"ml":         {"1/false", "0/false", "M14 ML"},
	"mc":         {"1/true", "1/false", "M10 weights"},
	"req_cancel": {"1/true", "1/false", "M11 Cloud"},
}

// infoV2Volatile hides what differs between two runs of one binary: the clocks (their text's shape is compared),
// the durations (C's constant zeros are not), each agent's start (the cloud status's `since` and `age`). The build
// info's slots and the capabilities' values are compared on their own (compareBuildinfoSlots, capabilityDiffs).
var infoV2Volatile = []Mask{
	{"agents.[].now", "the clock"},
	{"agents.[].application", "compared slot by slot"},
	{"agents.[].capabilities.[].version", "compared by name"},
	{"agents.[].capabilities.[].enabled", "compared by name"},
	{"agents.[].cloud.since", "each agent's start"},
	{"agents.[].cloud.age", "each agent's start"},
	{"agents.[].db_size.[].to", "the clock"},
	{"agents.[].timings.query_ms", "durations"},
	{"agents.[].timings.output_ms", "durations"},
	{"agents.[].timings.total_ms", "durations"},
	{"agents.[].timings.cloud_ms", "durations"},
	{"timings.total_ms", "durations"},
}

// digitsRe finds the digits of a clock's text, which leaves its shape (a number, or an RFC 3339 date).
var digitsRe = regexp.MustCompile(`[0-9]+`)

// clockShapes are the shapes of the first agent's `now` and its tiers' `to`.
func clockShapes(v Value) []string {
	var out []string
	if now, ok := agentMember(v, "now"); ok {
		out = append(out, digitsRe.ReplaceAllString(now.String(), "9"))
	}
	if db, ok := agentMember(v, "db_size"); ok {
		for _, tier := range db.Items {
			for _, m := range tier.Members {
				if m.Key == "to" {
					out = append(out, digitsRe.ReplaceAllString(m.Value.String(), "9"))
				}
			}
		}
	}
	return out
}

// infoV2Since hides a tier's retention start where it is not data: the memory engine's window ends now, and an
// empty dbengine tier starts with its engine.
var infoV2Since = Mask{"agents.[].db_size.[].from", "the clock or each engine's start"}

// infoV2Retention hides what a dbengine tier's retention takes from the clock: its length and what is extrapolated
// from it.
var infoV2Retention = []Mask{
	{"agents.[].db_size.[].retention", "the clock"},
	{"agents.[].db_size.[].retention_human", "the clock"},
	{"agents.[].db_size.[].expected_retention", "the clock"},
	{"agents.[].db_size.[].expected_retention_human", "the clock"},
}

// infoV2Fresh hides what a fresh dbengine's tiers take from their engine's start and from the clock.
var infoV2Fresh = append([]Mask{infoV2Since}, infoV2Retention...)

// infoV2RunR is runR's list: infoV2Retention, and tier 0's sample count, which each agent sums at its start from its
// journal files in parallel pool jobs that can count a metric's samples twice (rrdengine.c:1933-2025, mrg.c:508-590):
// seen 4 to 106565 higher, on either side (D228). Tiers 1 and 2 never varied and stay compared.
var infoV2RunR = append(slices.Clone(infoV2Retention),
	Mask{"agents.[].db_size.[0].samples", "tier 0's count at start: parallel journal population (D228)"})

// jsonScalarRe finds every key and scalar value of a JSON text, which leaves its layout.
var jsonScalarRe = regexp.MustCompile(`"(?:[^"\\]|\\.)*"|-?[0-9][0-9.eE+-]*|true|false|null`)

// infoV2Requests are the requests of check `api.v2-info` for a routed host named `host`, each with the status C
// answers: the options, a window, the node filters (the walk matches no node in this mode, and feeds only the
// timeout), both API versions, a routed host (ignored) and a subpath.
func infoV2Requests(host string) [][2]string {
	return [][2]string{
		{"/api/v2/info", "200"},
		{"/api/v2/info?options=minify", "200"},
		{"/api/v2/info?options=debug", "200"},
		{"/api/v2/info?options=debug,minify", "200"},
		{"/api/v2/info?options=mcp", "200"},
		{"/api/v2/info?options=rfc3339", "200"},
		{"/api/v2/info?options=long-json-keys", "200"},
		{"/api/v2/info?after=-60", "200"},
		{"/api/v2/info?after=-60&before=0&options=debug%7Crfc3339", "200"},
		{"/api/v2/info?nodes=nomatch&options=debug", "200"},
		{"/api/v2/info?timeout=-1", "504"},
		{"/api/v2/info?scope_nodes=nomatch&timeout=-1", "200"},
		{"/api/v3/info", "200"},
		{"/api/v3/info?options=minify", "200"},
		{"/host/" + host + "/api/v2/info", "200"},
		{"/api/v2/info/x", "400"},
	}
}

// infoV2Get sends one request and returns the raw answer, from `from` when set.
func infoV2Get(t *testing.T, side Role, addr, from, path string) []byte {
	t.Helper()
	request := []byte("GET " + path + " HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
	var b []byte
	var err error
	if from != "" {
		b, err = rawExchangeFrom(from, addr, request, 10*time.Second)
	} else {
		b, err = rawExchange(addr, request, 10*time.Second)
	}
	if err != nil {
		t.Fatalf("%s %s: %v", side, path, err)
	}
	return b
}

// agentMember is the member `key` of the first agent.
func agentMember(v Value, key string) (Value, bool) {
	for _, m := range v.Members {
		if m.Key == "agents" && len(m.Value.Items) > 0 {
			for _, a := range m.Value.Items[0].Members {
				if a.Key == key {
					return a.Value, true
				}
			}
		}
	}
	return Value{}, false
}

// capabilityValues are a capability list's names in order and each one's `version/enabled`.
func capabilityValues(v Value) ([]string, map[string]string) {
	var names []string
	values := map[string]string{}
	for _, item := range v.Items {
		var name, version, enabled string
		for _, m := range item.Members {
			switch m.Key {
			case "name":
				name = m.Value.Text
			case "version":
				version = m.Value.String()
			case "enabled":
				enabled = m.Value.String()
			}
		}
		names = append(names, name)
		values[name] = version + "/" + enabled
	}
	return names, values
}

// compareCapabilities checks the two capability lists: the same names in order, equal but for capabilityDiffs (all
// equal when the oracle is its own candidate).
func compareCapabilities(t *testing.T, where string, o, c Value) {
	t.Helper()
	same := sameBinary(t)
	names, oValues := capabilityValues(o)
	cNames, cValues := capabilityValues(c)
	if strings.Join(names, " ") != strings.Join(cNames, " ") {
		t.Fatalf("%s: capabilities differ\noracle:    %v\ncandidate: %v", where, names, cNames)
	}
	listed := 0
	for _, name := range names {
		ov, cv := oValues[name], cValues[name]
		d, ok := capabilityDiffs[name]
		switch {
		case ok && !same:
			listed++
			if ov != d.c || cv != d.rust {
				t.Errorf("%s: capability %s (%s): oracle %s, candidate %s; listed %s and %s", where, name,
					d.closes, ov, cv, d.c, d.rust)
			}
			if ov == cv {
				t.Errorf("%s: capability %s: the candidate now says %s as C does: remove it from capabilityDiffs (%s)",
					where, name, cv, d.closes)
			}
		case ov != cv:
			t.Errorf("%s: capability %s: oracle %s, candidate %s", where, name, ov, cv)
		}
	}
	if !same && listed != len(capabilityDiffs) {
		t.Errorf("%s: %d of the %d listed capabilities exist", where, listed, len(capabilityDiffs))
	}
}

// infoV2Family is the family of an info answer: the clocks and durations masked (infoV2Volatile), and masks.
func infoV2Family(masks ...Mask) v2Family {
	return v2Family{masks: slices.Concat(infoV2Volatile, masks)}
}

// compareInfoV2 compares each request's answers (from `from` when set) as the v2 envelope: compareV2 with the
// clocks and durations masked, and masks.
func compareInfoV2(t *testing.T, p *Pair, from string, requests [][2]string, masks ...Mask) {
	t.Helper()
	fam := infoV2Family(masks...)
	for _, r := range requests {
		compareV2(t, p, v2Req{name: r[0], target: r[0], status: r[1], from: from}, fam)
	}
}

// TestInfoV2 compares `/api/v2/info` and `/api/v3/info` (check `api.v2-info`) on an alloc agent (its memory tier,
// and the NOCHECK routes from a client the dashboard list excludes), a fresh dbengine and the runR fixture's three
// tiers (`NETDATA_DBENGINE_FIXTURES`; skipped without it), and a parent with a live child (the node counts). The
// pulse charts are off: with them the counts move with each side's own collection.
func TestInfoV2(t *testing.T) {
	requests := infoV2Requests(parentIdentity.Hostname)
	t.Run("alloc", func(t *testing.T) {
		p := StartPair(t, daemon.Options{DBMode: "alloc", StreamMemoryMode: "ram", StorageTiers: 1, PulseOff: true,
			WebExtra: "    allow dashboard from = localhost\n"}, parentIdentity)
		compareInfoV2(t, p, "", requests, infoV2Since)
		compareInfoV2(t, p, "127.0.0.2", [][2]string{{"/api/v2/info", "200"}, {"/api/v3/info", "200"},
			{"/api/v1/info", "451"}}, infoV2Since)
	})
	t.Run("fresh-dbengine", func(t *testing.T) {
		p := StartPair(t, daemon.Options{StorageTiers: 3, TierRetentionMB: [3]int{25, 25, 25}, PulseOff: true},
			parentIdentity)
		// an empty tier's retention starts with its engine: a request in that second has none
		time.Sleep(2 * time.Second)
		compareInfoV2(t, p, "", requests[:1], infoV2Fresh...)
	})
	t.Run("runR", func(t *testing.T) {
		fx, id := runRParent(t)
		p := StartPair(t, daemon.Options{StorageTiers: 3, TierRetentionMB: [3]int{25, 25, 25},
			SeedCache: filepath.Join(fx, "runR", "cache"), PulseOff: true}, id)
		compareInfoV2(t, p, "", infoV2Requests(id.Hostname)[:3], infoV2RunR...)
	})
	t.Run("child", func(t *testing.T) {
		p := StartPair(t, daemon.Options{DBMode: "alloc", StreamMemoryMode: "ram", StorageTiers: 1, PulseOff: true},
			parentIdentity)
		dashChild(t, p, dashBase())
		compareInfoV2(t, p, "", [][2]string{{"/api/v2/info", "200"},
			{fmt.Sprintf("/host/%s/api/v2/info", childHost.Hostname), "200"}}, infoV2Since)
	})
	// a child whose sender is connected counts itself sending (review R55 M1: the rows above count 0 on both sides)
	t.Run("sender", func(t *testing.T) {
		bins := binaries(t)
		var nodes [2]string
		for i, role := range []Role{Oracle, Candidate} {
			parent, err := stream.StartParent(stream.PlaintextAnswer)
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { parent.Close() })
			d := senderChild(t, bins[i], Role(string(role)+"-sending"), parent, "")
			if parent.WaitSession(1, 60*time.Second) == nil {
				t.Fatalf("%s: no STREAM connection within 60 s", role)
			}
			// the sender is connected once the parent answered
			time.Sleep(2 * time.Second)
			body := infoV2Get(t, role, d.Addr, "", "/api/v2/info")
			nodes[i] = strings.Join(strings.Fields(infoV2NodesRe.FindString(string(body))), "")
		}
		if nodes[0] != `"nodes":{"total":1,"receiving":0,"sending":1,"archived":0}` {
			t.Errorf("the oracle's sending child: %s", nodes[0])
		}
		if nodes[0] != nodes[1] {
			t.Errorf("nodes: oracle %s, candidate %s", nodes[0], nodes[1])
		}
	})
}

// infoV2NodesRe is the first agent's node counts.
var infoV2NodesRe = regexp.MustCompile(`"nodes"\s*:\s*\{[^}]*\}`)
