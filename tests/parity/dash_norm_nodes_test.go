// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"strings"
	"testing"
)

// testDashNormNodes pins the node rows the forks of milestone 10's commit 2 add (D231): F1's guard (nodesHealthOn),
// F2's family and guard (fnStreamNodesFamily, fnStreamNodesFacts) and the context scope's row (`v2-nodes-ctx`), on
// C's answers recorded C against C (H33's probe p1, 14:00-14:01Z: `health.api` `endpoints`, `fn.stream` `calls`,
// `api.v2-nodes`), trimmed to the members under test.
func testDashNormNodes(t *testing.T) {
	const (
		parent = `{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,`
		health = `{"status":"online","alerts":{"critical":0,"warning":1,"clear":2,"undefined":0,"uninitialized":0}}`
		off    = `"state":"reachable","health":{"status":"disabled"},`
		// the fixture child as C lists it (no label, no system info)
		child = `{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"v":"1.0","labels":{},` +
			`"hw":{"architecture":"","cpu_frequency":"","cpus":"","memory":"","disk_space":"","virtualization":"",` +
			`"container":""},"os":{"id":"","nm":"","v":"","kernel":{"nm":"","v":""}},` + off + `"capabilities":`
		timings = `"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.041}`
	)
	parse := func(b string) Value {
		t.Helper()
		v, err := ParseJSON([]byte(b))
		if err != nil {
			t.Fatalf("%v: %s", err, b)
		}
		return v
	}

	// F1: localhost with health on, as C's `/api/v3/nodes` printed it in `endpoints`
	healthOn := `{"api":2,"nodes":[` + parent + `"state":"reachable","health":` + health + `,"capabilities":` +
		nodeCapsOf(capFuncsOn, capHealthOn, capDyncfgOn) + `}],` + timings + `}`
	for name, c := range map[string]struct {
		in  string
		bad bool
	}{
		"C's answer":   {healthOn, false},
		"no warning":   {strings.Replace(healthOn, `"warning":1`, `"warning":0`, 1), true},
		"initializing": {strings.Replace(healthOn, `"online"`, `"initializing"`, 1), true},
		"four counts":  {strings.Replace(healthOn, `,"uninitialized":0`, ``, 1), true},
		"the counts moved": {strings.Replace(healthOn, `"critical":0,"warning":1`, `"warning":1,"critical":0`, 1),
			true},
		"health off":      {strings.Replace(healthOn, `"health",`+capHealthOn, `"health",`+capHealthOff, 1), true},
		"health disabled": {strings.Replace(healthOn, health, `{"status":"disabled"}`, 1), true},
	} {
		if err := dashGuard(nodesHealthOn)(parse(c.in)); (err != nil) != c.bad {
			t.Errorf("nodesHealthOn, %s: %v", name, err)
		}
	}

	// F2: `/api/v3/nodes?options=minify` on fn.stream's two C parents (labels trimmed): the child names its own
	// parent in `_streams_to`, and the vnode's labels came in another order on the second parent
	addrs := [2]string{"127.0.0.1:43647", "127.0.0.1:40391"}
	childCaps, vnodeCaps := nodeCapsOf(capFuncsOn, capHealthOff, capDyncfgOn),
		nodeCapsOf(capFuncsOn, capHealthOff, capDyncfgOff)
	fnNodes := func(dest, vnodeLabels, childCaps, vnodeCaps string) string {
		return `{"api":2,"nodes":[` + parent + `"labels":{"_is_parent":"true","_hostname":"parity-parent"},` + off +
			`"capabilities":` + nodeCaps(true) + `},{"mg":"5a1e0000-0000-4000-8000-00000000c0bb",` +
			`"nm":"parity-rchild","ni":1,"labels":{"_is_parent":"false","_streams_to":"` + dest +
			`","_hostname":"parity-rchild"},` + off +
			`"capabilities":` + childCaps + `},{"mg":"5a1e0000-0000-4000-8000-00000000c0b2","nm":"rchild-v","ni":2,` +
			`"labels":` + vnodeLabels + `,` + off + `"capabilities":` + vnodeCaps + `}],` + timings + `}`
	}
	const vLabels = `{"role":"edge","_os":"Netdata Virtual Host 1.0","_hostname":"rchild-v"}`
	fnO := fnNodes(addrs[0], vLabels, childCaps, vnodeCaps)
	fnC := fnNodes(addrs[1], `{"_hostname":"rchild-v","role":"edge","_os":"Netdata Virtual Host 1.0"}`, childCaps,
		vnodeCaps)
	fam := fnStreamNodesFamily(addrs)
	for name, c := range map[string]struct {
		candidate string
		fam       v2Family
		want      string
	}{
		"the recorded pair":  {fnC, fam, ""},
		"without the render": {fnC, nodesFamily, "$.nodes[1].labels._streams_to"},
		"another parent's address": {strings.Replace(fnC, `"_streams_to":"`+addrs[1], `"_streams_to":"`+addrs[0], 1),
			fam, "$.nodes[1].labels._streams_to"},
		"a label of the vnode": {strings.Replace(fnC, `"role":"edge"`, `"role":"core"`, 1), fam,
			"$.nodes[2].labels.role"},
	} {
		if got := dashNormDiffs(t, c.fam, c.fam.masks, [2]int64{}, fnO, c.candidate); got != c.want {
			t.Errorf("fn.stream nodes, %s: differences at %q, want %q", name, got, c.want)
		}
	}
	rendered := func(b string) string { return string(fam.normalise(0, [2]int64{}, []byte(b))) }
	for name, c := range map[string]struct {
		in  string
		bad bool
	}{
		"C's answer":   {rendered(fnO), false},
		"not rendered": {fnO, true},
		"the child without functions": {rendered(fnNodes(addrs[0], vLabels,
			nodeCapsOf(capFuncsOff, capHealthOff, capDyncfgOn), vnodeCaps)), true},
		"the child without dyncfg": {rendered(fnNodes(addrs[0], vLabels,
			nodeCapsOf(capFuncsOn, capHealthOff, capDyncfgOff), vnodeCaps)), true},
		"the vnode with dyncfg": {rendered(fnNodes(addrs[0], vLabels, childCaps,
			nodeCapsOf(capFuncsOn, capHealthOff, capDyncfgOn))), true},
		"the vnode without functions": {rendered(fnNodes(addrs[0], vLabels, childCaps,
			nodeCapsOf(capFuncsOff, capHealthOff, capDyncfgOff))), true},
	} {
		if err := dashGuard(fnStreamNodesFacts)(parse(c.in)); (err != nil) != c.bad {
			t.Errorf("fnStreamNodesFacts, %s: %v", name, err)
		}
	}

	// the context scope's row: the child alone, numbered 0; the parent's answer is refused
	var ctxRow v2Req
	for _, r := range nodesRows() {
		if r.name == "v2-nodes-ctx" {
			ctxRow = r
		}
	}
	if ctxRow.guard == nil {
		t.Fatal("no row v2-nodes-ctx")
	}
	ctxAnswer := `{"api":2,"nodes":[` + child + nodeCaps(false) + `}],` + timings + `}`
	both := `{"api":2,"nodes":[` + parent + `"v":"v","labels":{},"hw":{},"os":{},` + off + `"capabilities":` +
		nodeCaps(true) + `},` + strings.Replace(child, `"ni":0`, `"ni":1`, 1) + nodeCaps(false) + `}],` + timings + `}`
	if err := ctxRow.guard(parse(ctxAnswer)); err != nil {
		t.Errorf("v2-nodes-ctx's guard on C's answer: %v", err)
	}
	if err := ctxRow.guard(parse(both)); err == nil {
		t.Errorf("v2-nodes-ctx's guard took localhost")
	}
	after := `{"api":2,"nodes":[` + child + nodeCaps(false) + `},` + strings.Replace(parent, `"ni":0`, `"ni":1`, 1) +
		`"v":"v","labels":{},"hw":{},"os":{},` + off + `"capabilities":` + nodeCaps(true) + `}],` + timings + `}`
	if err := ctxRow.guard(parse(after)); err == nil {
		t.Errorf("v2-nodes-ctx's guard took localhost after the child")
	}
}
