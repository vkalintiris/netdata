// SPDX-License-Identifier: GPL-3.0-or-later

package parity

// The rows of `api.v2-q` (TestSearchAPI) as one C-against-C run answered them, both sides (0 the oracle): each
// row's target, its two bodies and the seconds each request was in flight. A JSON body is written without the
// whitespace outside its strings: its members, their order and every scalar's text are C's; `lines` are the lines
// each body had as C wrote it. Generated from the dumps of a copy of the rows that writes each answer out (the
// harness agent H37's run of 2026-10-08T17:45Z; its hand-back says how to make it again).

// dashNormSearchRow is one recorded row.
type dashNormSearchRow struct {
	target string
	bodies [2]string
	flight [2][2]int64
	lines  [2]int
}

// dashNormSearchRows are the recorded rows by `<case>/<row>`.
var dashNormSearchRows = map[string]dashNormSearchRow{
	"data/v3-child": {
		target: "/api/v3/q?q=*alpha*&scope_nodes=parity-child",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.155,"output_ms":0.19,"total_ms":0.345,"cloud_ms":0.345}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.345}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.054,"output_ms":0.031,"total_ms":0.085,"cloud_ms":0.085}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.085}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{51, 51},
	},
	"data/v2-all": {
		target: "/api/v2/q?q=*alpha*",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.011,"output_ms":0.007,"total_ms":0.018,"cloud_ms":0.018}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.018}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.014,"output_ms":0.007,"total_ms":0.021,"cloud_ms":0.021}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.021}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{60, 60},
	},
	"data/plain": {
		target: "/api/v2/q?q=A",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"family":"fam","matched":["families","instances","dimensions"],"instances":["q.q_a_name"],"dimensions":["alpha","a"]}},"searches":{"strings":13,"char":0,"total":13},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.015,"output_ms":0.031,"total_ms":0.046,"cloud_ms":0.046}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.046}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"family":"fam","matched":["families","instances","dimensions"],"instances":["q.q_a_name"],"dimensions":["alpha","a"]}},"searches":{"strings":13,"char":0,"total":13},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.011,"output_ms":0.008,"total_ms":0.019,"cloud_ms":0.019}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.019}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{62, 62},
	},
	"data/label-key": {
		target: "/api/v2/q?q=k",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["labels"],"labels":{"k":["v2","v1"]}}},"searches":{"strings":15,"char":2,"total":17},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.037,"output_ms":0.03,"total_ms":0.067,"cloud_ms":0.067}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.067}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["labels"],"labels":{"k":["v1","v2"]}}},"searches":{"strings":15,"char":2,"total":17},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.011,"output_ms":0.007,"total_ms":0.018,"cloud_ms":0.018}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.018}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{62, 62},
	},
	"data/label-value": {
		target: "/api/v2/q?q=v2",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["labels"],"labels":{"k":["v2"]}}},"searches":{"strings":15,"char":1,"total":16},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.014,"output_ms":0.008,"total_ms":0.022,"cloud_ms":0.022}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.022}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["labels"],"labels":{"k":["v2"]}}},"searches":{"strings":15,"char":1,"total":16},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.033,"output_ms":0.037,"total_ms":0.07,"cloud_ms":0.07}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.07}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{62, 62},
	},
	"data/nomatch": {
		target: "/api/v2/q?q=nomatch",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.019,"output_ms":0.011,"total_ms":0.03,"cloud_ms":0.03}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.03}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.011,"output_ms":0.006,"total_ms":0.017,"cloud_ms":0.017}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.017}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{56, 56},
	},
	"data/nomatch-scoped": {
		target: "/api/v2/q?q=nomatch&scope_contexts=q.*",
		bodies: [2]string{
			`{"api":2,"nodes":[],"contexts":{},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.007,"output_ms":0.004,"total_ms":0.011,"cloud_ms":0.011}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.011}}`,
			`{"api":2,"nodes":[],"contexts":{},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.006,"output_ms":0.003,"total_ms":0.009,"cloud_ms":0.009}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.009}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{38, 38},
	},
	"data/contexts-beside": {
		target: "/api/v2/q?q=alpha&contexts=nomatch",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.012,"output_ms":0.007,"total_ms":0.019,"cloud_ms":0.019}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.019}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.009,"output_ms":0.007,"total_ms":0.016,"cloud_ms":0.016}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.016}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{51, 51},
	},
	"data/fields": {
		target: "/api/v2/q?q=fam|units|title",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","family":"fam","units":"units","matched":["title","units","families"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.011,"output_ms":0.007,"total_ms":0.018,"cloud_ms":0.018}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.018}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","family":"fam","units":"units","matched":["title","units","families"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.012,"output_ms":0.007,"total_ms":0.019,"cloud_ms":0.019}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.019}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{62, 62},
	},
	"data/cut": {
		target: "/api/v2/q?q=alpha|b|z|inc",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha","b","... 2 dimensions more"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.032,"output_ms":0.009,"total_ms":0.041,"cloud_ms":0.041}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.041}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha","b","... 2 dimensions more"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.017,"output_ms":0.007,"total_ms":0.024,"cloud_ms":0.024}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.024}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{60, 60},
	},
	"data/cut4": {
		target: "/api/v2/q?q=alpha|b|z|inc&cardinality=4",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha","b","z","inc"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.013,"output_ms":0.021,"total_ms":0.034,"cloud_ms":0.034}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.034}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha","b","z","inc"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.015,"output_ms":0.007,"total_ms":0.022,"cloud_ms":0.022}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.022}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{60, 60},
	},
	"data/cut2": {
		target: "/api/v2/q?q=alpha|b|z|inc&cardinality=2",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha","b","... 2 dimensions more"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.015,"output_ms":0.009,"total_ms":0.024,"cloud_ms":0.024}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.024}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha","b","... 2 dimensions more"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.014,"output_ms":0.006,"total_ms":0.02,"cloud_ms":0.02}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.02}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{60, 60},
	},
	"data/all": {
		target: "/api/v2/q",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":[]}},"searches":{"strings":0,"char":0,"total":0},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.007,"output_ms":0.005,"total_ms":0.012,"cloud_ms":0.012}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.012}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":[]}},"searches":{"strings":0,"char":0,"total":0},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.027,"output_ms":0.006,"total_ms":0.033,"cloud_ms":0.033}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.033}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{59, 59},
	},
	"data/wordless": {
		target: "/api/v2/q?q=,",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":[]}},"searches":{"strings":0,"char":0,"total":0},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.006,"output_ms":0.005,"total_ms":0.011,"cloud_ms":0.011}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.011}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":[]}},"searches":{"strings":0,"char":0,"total":0},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.006,"output_ms":0.005,"total_ms":0.011,"cloud_ms":0.011}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.011}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{59, 59},
	},
	"data/star": {
		target: "/api/v2/q?q=*",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":[]}},"searches":{"strings":0,"char":0,"total":0},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.009,"output_ms":0.006,"total_ms":0.015,"cloud_ms":0.015}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.015}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":[]}},"searches":{"strings":0,"char":0,"total":0},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.014,"output_ms":0.006,"total_ms":0.02,"cloud_ms":0.02}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.02}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{59, 59},
	},
	"data/debug": {
		target: "/api/v2/q?q=alpha&options=debug",
		bodies: [2]string{
			`{"api":2,"request":{"mode":["versions","agents","nodes","search"],"options":["debug","instances","dimensions","labels","titles","family","units"],"scope":{"scope_nodes":null,"scope_contexts":null},"selectors":{"nodes":null,"contexts":null},"filters":{"q":"alpha","after":0,"before":0}},"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.077,"output_ms":0.008,"total_ms":0.085,"cloud_ms":0.085}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.085}}`,
			`{"api":2,"request":{"mode":["versions","agents","nodes","search"],"options":["debug","instances","dimensions","labels","titles","family","units"],"scope":{"scope_nodes":null,"scope_contexts":null},"selectors":{"nodes":null,"contexts":null},"filters":{"q":"alpha","after":0,"before":0}},"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.021,"output_ms":0.006,"total_ms":0.027,"cloud_ms":0.027}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.027}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{77, 77},
	},
	"data/debug-noq": {
		target: "/api/v2/q?options=debug",
		bodies: [2]string{
			`{"api":2,"request":{"mode":["versions","agents","nodes","search"],"options":["debug","instances","dimensions","labels","titles","family","units"],"scope":{"scope_nodes":null,"scope_contexts":null},"selectors":{"nodes":null,"contexts":null},"filters":{"q":null,"after":0,"before":0}},"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":[]}},"searches":{"strings":0,"char":0,"total":0},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.021,"output_ms":0.006,"total_ms":0.027,"cloud_ms":0.027}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.027}}`,
			`{"api":2,"request":{"mode":["versions","agents","nodes","search"],"options":["debug","instances","dimensions","labels","titles","family","units"],"scope":{"scope_nodes":null,"scope_contexts":null},"selectors":{"nodes":null,"contexts":null},"filters":{"q":null,"after":0,"before":0}},"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":[]}},"searches":{"strings":0,"char":0,"total":0},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.012,"output_ms":0.006,"total_ms":0.018,"cloud_ms":0.018}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.018}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{76, 76},
	},
	"data/debug-selectors": {
		target: "/api/v2/q?q=alpha&options=debug&scope_nodes=*&nodes=*&scope_contexts=q.*&contexts=*ctx&cardinality=7&timeout=20000&after=-600&before=0",
		bodies: [2]string{
			`{"api":2,"request":{"mode":["versions","agents","nodes","search"],"options":["debug","instances","dimensions","labels","titles","family","units"],"scope":{"scope_nodes":"*","scope_contexts":"q.*"},"selectors":{"nodes":"*","contexts":"*ctx"},"filters":{"q":"alpha","after":-600,"before":0}},"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481561,"ai":0,"timings":{"prep_ms":0,"query_ms":0.019,"output_ms":0.005,"total_ms":0.024,"cloud_ms":0.024}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.024}}`,
			`{"api":2,"request":{"mode":["versions","agents","nodes","search"],"options":["debug","instances","dimensions","labels","titles","family","units"],"scope":{"scope_nodes":"*","scope_contexts":"q.*"},"selectors":{"nodes":"*","contexts":"*ctx"},"filters":{"q":"alpha","after":-600,"before":0}},"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481561,"ai":0,"timings":{"prep_ms":0,"query_ms":0.015,"output_ms":0.006,"total_ms":0.021,"cloud_ms":0.021}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.021}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{68, 68},
	},
	"data/mcp": {
		target: "/api/v2/q?q=alpha&options=mcp",
		bodies: [2]string{
			`{"nodes":[{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent","relationship":"localhost","connected":true},{"machine_guid":"5a1e0000-0000-4000-8000-0000000000bb","hostname":"parity-child","relationship":"child","connected":true}],"contexts":{"q.ctx":{"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.009,"output_ms":0.02,"total_ms":0.029,"cloud_ms":0.029}}]}`,
			`{"nodes":[{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent","relationship":"localhost","connected":true},{"machine_guid":"5a1e0000-0000-4000-8000-0000000000bb","hostname":"parity-child","relationship":"child","connected":true}],"contexts":{"q.ctx":{"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.013,"output_ms":0.007,"total_ms":0.02,"cloud_ms":0.02}}]}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{45, 45},
	},
	"data/minify": {
		target: "/api/v2/q?q=alpha&options=minify",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.009,"output_ms":0.006,"total_ms":0.015,"cloud_ms":0.015}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.015}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.011,"output_ms":0.005,"total_ms":0.016,"cloud_ms":0.016}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.016}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{1, 1},
	},
	"data/long-keys": {
		target: "/api/v2/q?q=alpha&options=long-json-keys",
		bodies: [2]string{
			`{"api":2,"nodes":[{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent","nodes_array_index":0,"status":{"agents_array_index":0,"code":200,"msg":""}},{"machine_guid":"5a1e0000-0000-4000-8000-0000000000bb","hostname":"parity-child","nodes_array_index":1,"status":{"agents_array_index":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.013,"output_ms":0.011,"total_ms":0.024,"cloud_ms":0.024}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.024}}`,
			`{"api":2,"nodes":[{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent","nodes_array_index":0,"status":{"agents_array_index":0,"code":200,"msg":""}},{"machine_guid":"5a1e0000-0000-4000-8000-0000000000bb","hostname":"parity-child","nodes_array_index":1,"status":{"agents_array_index":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.033,"output_ms":0.021,"total_ms":0.054,"cloud_ms":0.054}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.054}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{60, 60},
	},
	"data/every": {
		target: "/api/v2/q?q=**",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","family":"fam","units":"units","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["q.q_a_name","q.two"],"dimensions":["alpha","b","... 4 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v2","v1"]}}},"searches":{"strings":13,"char":12,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.027,"output_ms":0.011,"total_ms":0.038,"cloud_ms":0.038}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.038}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","family":"fam","units":"units","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["q.q_a_name","q.two"],"dimensions":["alpha","b","... 4 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v1","v2"]}}},"searches":{"strings":13,"char":12,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.027,"output_ms":0.01,"total_ms":0.037,"cloud_ms":0.037}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.037}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{69, 69},
	},
	"data/every-whole": {
		target: "/api/v2/q?q=**&cardinality=6",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","family":"fam","units":"units","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["q.q_a_name","q.two"],"dimensions":["alpha","b","z","inc","h","a"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v2","v1"]}}},"searches":{"strings":13,"char":12,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.012,"output_ms":0.01,"total_ms":0.022,"cloud_ms":0.022}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.022}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","family":"fam","units":"units","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["q.q_a_name","q.two"],"dimensions":["alpha","b","z","inc","h","a"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v1","v2"]}}},"searches":{"strings":13,"char":12,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.013,"output_ms":0.009,"total_ms":0.022,"cloud_ms":0.022}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.022}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{69, 69},
	},
	"data/negative": {
		target: "/api/v2/q?q=!*",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.007,"output_ms":0.005,"total_ms":0.012,"cloud_ms":0.012}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.012}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.007,"output_ms":0.005,"total_ms":0.012,"cloud_ms":0.012}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.012}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{56, 56},
	},
	"data/negative-first": {
		target: "/api/v2/q?q=!fam,*",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","units":"units","matched":["id","title","units","instances","dimensions","labels"],"instances":["q.q_a_name","q.two"],"dimensions":["alpha","b","... 4 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v2","v1"]}}},"searches":{"strings":13,"char":12,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.031,"output_ms":0.015,"total_ms":0.046,"cloud_ms":0.046}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.046}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","units":"units","matched":["id","title","units","instances","dimensions","labels"],"instances":["q.q_a_name","q.two"],"dimensions":["alpha","b","... 4 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v1","v2"]}}},"searches":{"strings":13,"char":12,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.04,"output_ms":0.018,"total_ms":0.058,"cloud_ms":0.058}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.058}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{68, 68},
	},
	"data/window-every": {
		target: "/api/v2/q?q=**&after=1791481450&before=1791481460",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","family":"fam","units":"units","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["q.q_a_name","q.two"],"dimensions":["alpha","b","... 4 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v2","v1"]}}},"searches":{"strings":13,"char":12,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481561,"ai":0,"timings":{"prep_ms":0,"query_ms":0.045,"output_ms":0.024,"total_ms":0.069,"cloud_ms":0.069}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.069}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","family":"fam","units":"units","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["q.q_a_name","q.two"],"dimensions":["alpha","b","... 4 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v1","v2"]}}},"searches":{"strings":13,"char":12,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481561,"ai":0,"timings":{"prep_ms":0,"query_ms":0.017,"output_ms":0.01,"total_ms":0.027,"cloud_ms":0.027}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.027}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{60, 60},
	},
	"data/window-negative": {
		target: "/api/v2/q?q=!*&after=1791481450&before=1791481460",
		bodies: [2]string{
			`{"api":2,"nodes":[],"contexts":{},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481561,"ai":0,"timings":{"prep_ms":0,"query_ms":0.007,"output_ms":0.004,"total_ms":0.011,"cloud_ms":0.011}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.011}}`,
			`{"api":2,"nodes":[],"contexts":{},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481561,"ai":0,"timings":{"prep_ms":0,"query_ms":0.015,"output_ms":0.006,"total_ms":0.021,"cloud_ms":0.021}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.021}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{38, 38},
	},
	"data/window-before": {
		target: "/api/v2/q?q=**&after=1791480840&before=1791481140",
		bodies: [2]string{
			`{"api":2,"nodes":[],"contexts":{},"searches":{"strings":0,"char":0,"total":0},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481561,"ai":0,"timings":{"prep_ms":0,"query_ms":0.005,"output_ms":0.003,"total_ms":0.008,"cloud_ms":0.008}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.008}}`,
			`{"api":2,"nodes":[],"contexts":{},"searches":{"strings":0,"char":0,"total":0},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481561,"ai":0,"timings":{"prep_ms":0,"query_ms":0.004,"output_ms":0.003,"total_ms":0.007,"cloud_ms":0.007}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.007}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{38, 38},
	},
	"data/comma": {
		target: "/api/v2/q?q=nomatch%2Calpha",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.016,"output_ms":0.01,"total_ms":0.026,"cloud_ms":0.026}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.026}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.017,"output_ms":0.008,"total_ms":0.025,"cloud_ms":0.025}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.025}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{60, 60},
	},
	"data/plus": {
		target: "/api/v2/q?q=title+%5Bx%5D",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","matched":["title"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.011,"output_ms":0.006,"total_ms":0.017,"cloud_ms":0.017}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.017}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","matched":["title"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.028,"output_ms":0.011,"total_ms":0.039,"cloud_ms":0.039}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.039}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{60, 60},
	},
	"data/backslash": {
		target: "/api/v2/q?q=alpha%5C%2Cnomatch",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.012,"output_ms":0.006,"total_ms":0.018,"cloud_ms":0.018}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.018}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.012,"output_ms":0.006,"total_ms":0.018,"cloud_ms":0.018}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.018}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{56, 56},
	},
	"data/backslash-plain": {
		target: "/api/v2/q?q=al%5Cpha",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.015,"output_ms":0.007,"total_ms":0.022,"cloud_ms":0.022}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.022}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.017,"output_ms":0.008,"total_ms":0.025,"cloud_ms":0.025}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.025}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{60, 60},
	},
	"data/ampersand": {
		target: "/api/v2/q?q=nomatch%26q=alpha",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.016,"output_ms":0.007,"total_ms":0.023,"cloud_ms":0.023}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.023}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.01,"output_ms":0.006,"total_ms":0.016,"cloud_ms":0.016}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.016}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{60, 60},
	},
	"data/control": {
		target: "/api/v2/q?q=nomatch%0Aalpha",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.012,"output_ms":0.007,"total_ms":0.019,"cloud_ms":0.019}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.019}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.013,"output_ms":0.01,"total_ms":0.023,"cloud_ms":0.023}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.023}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{56, 56},
	},
	"data/control-cut": {
		target: "/api/v2/q?q=alpha%09nomatch&scope_nodes=parity-child",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.019,"output_ms":0.011,"total_ms":0.03,"cloud_ms":0.03}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.03}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"],"dimensions":["alpha"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":13,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481562,"ai":0,"timings":{"prep_ms":0,"query_ms":0.018,"output_ms":0.007,"total_ms":0.025,"cloud_ms":0.025}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.025}}`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{60, 60},
	},
	"data/timeout": {
		target: "/api/v2/q?q=a&timeout=-1",
		bodies: [2]string{
			`query timeout`,
			`query timeout`,
		},
		flight: [2][2]int64{{1791481562, 1791481562}, {1791481562, 1791481562}},
		lines:  [2]int{1, 1},
	},
	"merge/union": {
		target: "/api/v2/q?q=q_a",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["instances"],"instances":["q.q_a_name","q.q_a_other"]}},"searches":{"strings":45,"char":0,"total":45},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.075,"output_ms":0.031,"total_ms":0.106,"cloud_ms":0.106}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.106}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["instances"],"instances":["q.q_a_name","q.q_a_other"]}},"searches":{"strings":45,"char":0,"total":45},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.068,"output_ms":0.031,"total_ms":0.099,"cloud_ms":0.099}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.099}}`,
		},
		flight: [2][2]int64{{1791481575, 1791481575}, {1791481575, 1791481575}},
		lines:  [2]int{69, 69},
	},
	"merge/truncated": {
		target: "/api/v2/q?q=fam&cardinality=1",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"family":"fam[x]","matched":["families"]},"__truncated__":{"total_contexts":2,"returned":1,"remaining":1}},"searches":{"strings":45,"char":0,"total":45},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.016,"output_ms":0.009,"total_ms":0.025,"cloud_ms":0.025}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.025}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"family":"fam[x]","matched":["families"]},"__truncated__":{"total_contexts":2,"returned":1,"remaining":1}},"searches":{"strings":45,"char":0,"total":45},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.043,"output_ms":0.02,"total_ms":0.063,"cloud_ms":0.063}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.063}}`,
		},
		flight: [2][2]int64{{1791481575, 1791481575}, {1791481575, 1791481575}},
		lines:  [2]int{74, 74},
	},
	"merge/truncated-mcp": {
		target: "/api/v2/q?q=fam&cardinality=1&options=mcp",
		bodies: [2]string{
			`{"nodes":[{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent","relationship":"localhost","connected":true},{"machine_guid":"5a1e0000-0000-4000-8000-0000000000bb","hostname":"parity-child","relationship":"child","connected":true},{"machine_guid":"5a1e0000-0000-4000-8000-0000000000cc","hostname":"parity-child2","relationship":"child","connected":true}],"contexts":{"q.ctx":{"family":"fam[x]"},"__truncated__":{"total_contexts":2,"returned":1,"remaining":1}},"info":"Cardinality limit reached. Use cardinality_limit parameter to see more results.","searches":{"strings":45,"char":0,"total":45},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.026,"output_ms":0.019,"total_ms":0.045,"cloud_ms":0.045}}]}`,
			`{"nodes":[{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent","relationship":"localhost","connected":true},{"machine_guid":"5a1e0000-0000-4000-8000-0000000000bb","hostname":"parity-child","relationship":"child","connected":true},{"machine_guid":"5a1e0000-0000-4000-8000-0000000000cc","hostname":"parity-child2","relationship":"child","connected":true}],"contexts":{"q.ctx":{"family":"fam[x]"},"__truncated__":{"total_contexts":2,"returned":1,"remaining":1}},"info":"Cardinality limit reached. Use cardinality_limit parameter to see more results.","searches":{"strings":45,"char":0,"total":45},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.025,"output_ms":0.011,"total_ms":0.036,"cloud_ms":0.036}}]}`,
		},
		flight: [2][2]int64{{1791481575, 1791481575}, {1791481575, 1791481575}},
		lines:  [2]int{56, 56},
	},
	"merge/titles": {
		target: "/api/v2/q?q=title|other",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"[x] [x]","matched":["title","instances"],"instances":["q.q_a_other"]},"r.ctx":{"title":"r title [x]","matched":["title"]}},"searches":{"strings":45,"char":0,"total":45},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.028,"output_ms":0.007,"total_ms":0.035,"cloud_ms":0.035}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.035}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"[x] [x]","matched":["title","instances"],"instances":["q.q_a_other"]},"r.ctx":{"title":"r title [x]","matched":["title"]}},"searches":{"strings":45,"char":0,"total":45},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.033,"output_ms":0.012,"total_ms":0.045,"cloud_ms":0.045}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.045}}`,
		},
		flight: [2][2]int64{{1791481575, 1791481575}, {1791481575, 1791481575}},
		lines:  [2]int{74, 74},
	},
	"merge/title-one": {
		target: "/api/v2/q?q=other",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"other [x]","matched":["title","instances"],"instances":["q.q_a_other"]}},"searches":{"strings":45,"char":0,"total":45},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.036,"output_ms":0.011,"total_ms":0.047,"cloud_ms":0.047}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.047}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"other [x]","matched":["title","instances"],"instances":["q.q_a_other"]}},"searches":{"strings":45,"char":0,"total":45},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.015,"output_ms":0.007,"total_ms":0.022,"cloud_ms":0.022}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.022}}`,
		},
		flight: [2][2]int64{{1791481575, 1791481575}, {1791481575, 1791481575}},
		lines:  [2]int{70, 70},
	},
	"merge/a-or-b": {
		target: "/api/v2/q?q=a|b",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"family":"fam[x]","matched":["families","instances","dimensions"],"instances":["q.q_a_name","q.q_a_other"],"dimensions":["alpha","b","a"]},"r.ctx":{"family":"rfam","matched":["families","instances","dimensions"],"instances":["r.r_a_name"],"dimensions":["alpha","b","a"]}},"searches":{"strings":39,"char":0,"total":39},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.064,"output_ms":0.022,"total_ms":0.086,"cloud_ms":0.086}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.086}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"family":"fam[x]","matched":["families","instances","dimensions"],"instances":["q.q_a_name","q.q_a_other"],"dimensions":["alpha","b","a"]},"r.ctx":{"family":"rfam","matched":["families","instances","dimensions"],"instances":["r.r_a_name"],"dimensions":["alpha","b","a"]}},"searches":{"strings":39,"char":0,"total":39},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.034,"output_ms":0.009,"total_ms":0.043,"cloud_ms":0.043}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.043}}`,
		},
		flight: [2][2]int64{{1791481575, 1791481575}, {1791481575, 1791481575}},
		lines:  [2]int{77, 77},
	},
	"merge/labels": {
		target: "/api/v2/q?q=k",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["labels"],"labels":{"k":["v1","v2"]}},"r.ctx":{"matched":["labels"],"labels":{"k":["v1","v2"]}}},"searches":{"strings":45,"char":6,"total":51},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.023,"output_ms":0.01,"total_ms":0.033,"cloud_ms":0.033}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.033}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["labels"],"labels":{"k":["v1","v2"]}},"r.ctx":{"matched":["labels"],"labels":{"k":["v1","v2"]}}},"searches":{"strings":45,"char":6,"total":51},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.018,"output_ms":0.008,"total_ms":0.026,"cloud_ms":0.026}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.026}}`,
		},
		flight: [2][2]int64{{1791481575, 1791481575}, {1791481575, 1791481575}},
		lines:  [2]int{77, 77},
	},
	"merge/every-shared": {
		target: "/api/v2/q?q=**&cardinality=8",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"[x] [x]","family":"fam[x]","units":"units","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["q.q_a_name","q.two","q.q_a_other"],"dimensions":["alpha","b","z","... 3 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v1","v2"]}},"r.ctx":{"title":"r title [x]","family":"rfam","units":"runits","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["r.r_a_name","r.two"],"dimensions":["alpha","b","z","... 3 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v1","v2"]}}},"searches":{"strings":39,"char":36,"total":75},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.048,"output_ms":0.013,"total_ms":0.061,"cloud_ms":0.061}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.061}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"[x] [x]","family":"fam[x]","units":"units","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["q.q_a_name","q.two","q.q_a_other"],"dimensions":["alpha","b","z","... 3 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v1","v2"]}},"r.ctx":{"title":"r title [x]","family":"rfam","units":"runits","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["r.r_a_name","r.two"],"dimensions":["alpha","b","z","... 3 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"k":["v1","v2"]}}},"searches":{"strings":39,"char":36,"total":75},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":39,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481575,"ai":0,"timings":{"prep_ms":0,"query_ms":0.049,"output_ms":0.013,"total_ms":0.062,"cloud_ms":0.062}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.062}}`,
		},
		flight: [2][2]int64{{1791481575, 1791481575}, {1791481575, 1791481575}},
		lines:  [2]int{91, 91},
	},
	"labels/four": {
		target: "/api/v2/q?q=lk&scope_nodes=parity-child",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"matched":["labels"],"labels":{"lk":["w1","w2","... 2 values more"]}}},"searches":{"strings":12,"char":4,"total":16},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":16,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.063,"output_ms":0.029,"total_ms":0.092,"cloud_ms":0.092}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.092}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"matched":["labels"],"labels":{"lk":["w1","w2","... 2 values more"]}}},"searches":{"strings":12,"char":4,"total":16},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":16,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.066,"output_ms":0.029,"total_ms":0.095,"cloud_ms":0.095}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.095}}`,
		},
		flight: [2][2]int64{{1791481588, 1791481588}, {1791481588, 1791481588}},
		lines:  [2]int{53, 53},
	},
	"labels/four-whole": {
		target: "/api/v2/q?q=lk&cardinality=4&scope_nodes=parity-child",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"matched":["labels"],"labels":{"lk":["w1","w2","w3","w4"]}}},"searches":{"strings":12,"char":4,"total":16},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":16,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.038,"output_ms":0.019,"total_ms":0.057,"cloud_ms":0.057}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.057}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"matched":["labels"],"labels":{"lk":["w1","w2","w3","w4"]}}},"searches":{"strings":12,"char":4,"total":16},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":16,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.031,"output_ms":0.009,"total_ms":0.04,"cloud_ms":0.04}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.04}}`,
		},
		flight: [2][2]int64{{1791481588, 1791481588}, {1791481588, 1791481588}},
		lines:  [2]int{53, 53},
	},
	"labels/five": {
		target: "/api/v2/q?q=lk",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"matched":["labels"],"labels":{"lk":["w5","w1","... 3 values more"]}}},"searches":{"strings":20,"char":5,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.037,"output_ms":0.012,"total_ms":0.049,"cloud_ms":0.049}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.049}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"matched":["labels"],"labels":{"lk":["w1","w2","... 3 values more"]}}},"searches":{"strings":20,"char":5,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.027,"output_ms":0.008,"total_ms":0.035,"cloud_ms":0.035}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.035}}`,
		},
		flight: [2][2]int64{{1791481588, 1791481588}, {1791481588, 1791481588}},
		lines:  [2]int{71, 71},
	},
	"labels/five-whole": {
		target: "/api/v2/q?q=lk&cardinality=5",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"matched":["labels"],"labels":{"lk":["w5","w1","w2","w3","w4"]}}},"searches":{"strings":20,"char":5,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.02,"output_ms":0.008,"total_ms":0.028,"cloud_ms":0.028}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.028}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"matched":["labels"],"labels":{"lk":["w1","w2","w3","w4","w5"]}}},"searches":{"strings":20,"char":5,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.017,"output_ms":0.007,"total_ms":0.024,"cloud_ms":0.024}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.024}}`,
		},
		flight: [2][2]int64{{1791481588, 1791481588}, {1791481588, 1791481588}},
		lines:  [2]int{71, 71},
	},
	"labels/three": {
		target: "/api/v2/q?q=l3",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"matched":["labels"],"labels":{"l3":["x1","x2","x3"]}}},"searches":{"strings":20,"char":5,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.032,"output_ms":0.01,"total_ms":0.042,"cloud_ms":0.042}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.042}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"matched":["labels"],"labels":{"l3":["x1","x2","x3"]}}},"searches":{"strings":20,"char":5,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.017,"output_ms":0.008,"total_ms":0.025,"cloud_ms":0.025}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.025}}`,
		},
		flight: [2][2]int64{{1791481588, 1791481588}, {1791481588, 1791481588}},
		lines:  [2]int{71, 71},
	},
	"labels/take": {
		target: "/api/v2/q?q=ltitle|only2",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"title":"[x]","matched":["title","labels"],"labels":{"only2":["z"]}}},"searches":{"strings":20,"char":1,"total":21},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.022,"output_ms":0.009,"total_ms":0.031,"cloud_ms":0.031}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.031}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"title":"[x]","matched":["title","labels"],"labels":{"only2":["z"]}}},"searches":{"strings":20,"char":1,"total":21},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.017,"output_ms":0.007,"total_ms":0.024,"cloud_ms":0.024}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.024}}`,
		},
		flight: [2][2]int64{{1791481588, 1791481588}, {1791481588, 1791481588}},
		lines:  [2]int{72, 72},
	},
	"labels/keep": {
		target: "/api/v2/q?q=w1|mtitle",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"title":"[x]","matched":["title","labels"],"labels":{"lk":["w1"]}}},"searches":{"strings":20,"char":1,"total":21},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.026,"output_ms":0.01,"total_ms":0.036,"cloud_ms":0.036}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.036}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"title":"[x]","matched":["title","labels"],"labels":{"lk":["w1"]}}},"searches":{"strings":20,"char":1,"total":21},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.023,"output_ms":0.007,"total_ms":0.03,"cloud_ms":0.03}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.03}}`,
		},
		flight: [2][2]int64{{1791481588, 1791481588}, {1791481588, 1791481588}},
		lines:  [2]int{72, 72},
	},
	"labels/every": {
		target: "/api/v2/q?q=**",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"title":"[x]","family":"[x]fam","units":"lunits","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["l.c1","l.c2","... 3 instances more"],"dimensions":["d","epsilon"],"labels":{"only2":["z"],"u":["Grün"],"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"lk":["w5","w1","... 3 values more"],"l3":["x1","x2","x3"]}}},"searches":{"strings":19,"char":44,"total":63},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.046,"output_ms":0.013,"total_ms":0.059,"cloud_ms":0.059}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.059}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"title":"[x]","family":"[x]fam","units":"lunits","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["l.c1","l.c2","... 3 instances more"],"dimensions":["d","epsilon"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"lk":["w1","w2","... 3 values more"],"l3":["x1","x2","x3"],"only2":["z"],"u":["Grün"]}}},"searches":{"strings":19,"char":44,"total":63},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.062,"output_ms":0.012,"total_ms":0.074,"cloud_ms":0.074}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.074}}`,
		},
		flight: [2][2]int64{{1791481588, 1791481588}, {1791481588, 1791481588}},
		lines:  [2]int{81, 81},
	},
	"labels/fold-ascii": {
		target: "/api/v2/q?q=GR%C3%BCN",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"matched":["labels"],"labels":{"u":["Grün"]}}},"searches":{"strings":20,"char":1,"total":21},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.022,"output_ms":0.01,"total_ms":0.032,"cloud_ms":0.032}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.032}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"l.ctx":{"matched":["labels"],"labels":{"u":["Grün"]}}},"searches":{"strings":20,"char":1,"total":21},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.038,"output_ms":0.022,"total_ms":0.06,"cloud_ms":0.06}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.06}}`,
		},
		flight: [2][2]int64{{1791481588, 1791481588}, {1791481588, 1791481588}},
		lines:  [2]int{71, 71},
	},
	"labels/fold-bytes": {
		target: "/api/v2/q?q=gr%C3%9Cn",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{},"searches":{"strings":20,"char":0,"total":20},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.022,"output_ms":0.009,"total_ms":0.031,"cloud_ms":0.031}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.031}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000cc","nm":"parity-child2","ni":2,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{},"searches":{"strings":20,"char":0,"total":20},"versions":{"routing_hard_hash":1,"nodes_hard_hash":3,"contexts_hard_hash":21,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481588,"ai":0,"timings":{"prep_ms":0,"query_ms":0.011,"output_ms":0.006,"total_ms":0.017,"cloud_ms":0.017}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.017}}`,
		},
		flight: [2][2]int64{{1791481588, 1791481588}, {1791481588, 1791481588}},
		lines:  [2]int{65, 65},
	},
	"window/collected": {
		target: "/api/v2/q?q=**&after=1791481501&before=1791481510",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"w.ctx":{"title":"wtitle [x]","family":"wfam","units":"wunits","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["w.old","w.a"],"dimensions":["o1","p1","p2"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"wl":["old","a"]}}},"searches":{"strings":9,"char":12,"total":21},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":11,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481598,"ai":0,"timings":{"prep_ms":0,"query_ms":0.065,"output_ms":0.037,"total_ms":0.102,"cloud_ms":0.102}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.102}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"w.ctx":{"title":"wtitle [x]","family":"wfam","units":"wunits","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["w.old","w.a"],"dimensions":["o1","p1","p2"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"wl":["old","a"]}}},"searches":{"strings":9,"char":12,"total":21},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":11,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481598,"ai":0,"timings":{"prep_ms":0,"query_ms":0.043,"output_ms":0.015,"total_ms":0.058,"cloud_ms":0.058}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.058}}`,
		},
		flight: [2][2]int64{{1791481599, 1791481599}, {1791481599, 1791481599}},
		lines:  [2]int{60, 60},
	},
	"window/gone-late": {
		target: "/api/v2/q?q=**&after=1791481501&before=1791481510",
		bodies: [2]string{
			`{"api":2,"nodes":[],"contexts":{},"searches":{"strings":0,"char":0,"total":0},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":11,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481598,"ai":0,"timings":{"prep_ms":0,"query_ms":0.003,"output_ms":0.003,"total_ms":0.006,"cloud_ms":0.006}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.006}}`,
			`{"api":2,"nodes":[],"contexts":{},"searches":{"strings":0,"char":0,"total":0},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":11,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481598,"ai":0,"timings":{"prep_ms":0,"query_ms":0.011,"output_ms":0.01,"total_ms":0.021,"cloud_ms":0.021}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.021}}`,
		},
		flight: [2][2]int64{{1791481599, 1791481599}, {1791481599, 1791481599}},
		lines:  [2]int{38, 38},
	},
	"window/stopped": {
		target: "/api/v2/q?q=**&after=1791481461&before=1791481500",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"w.ctx":{"title":"wtitle [x]","family":"wfam","units":"wunits","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["w.a"],"dimensions":["p1","p3"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"wl":["a"]}}},"searches":{"strings":7,"char":6,"total":13},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":11,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481599,"ai":0,"timings":{"prep_ms":0,"query_ms":0.048,"output_ms":0.03,"total_ms":0.078,"cloud_ms":0.078}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.078}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"w.ctx":{"title":"wtitle [x]","family":"wfam","units":"wunits","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["w.a"],"dimensions":["p1","p3"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"wl":["a"]}}},"searches":{"strings":7,"char":6,"total":13},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":11,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481599,"ai":0,"timings":{"prep_ms":0,"query_ms":0.036,"output_ms":0.022,"total_ms":0.058,"cloud_ms":0.058}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.058}}`,
		},
		flight: [2][2]int64{{1791481600, 1791481600}, {1791481600, 1791481600}},
		lines:  [2]int{60, 60},
	},
	"window/stopped-edge": {
		target: "/api/v2/q?q=**&after=1791481460&before=1791481500",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"w.ctx":{"title":"wtitle [x]","family":"wfam","units":"wunits","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["w.a"],"dimensions":["p1","p2","p3"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"wl":["a"]}}},"searches":{"strings":8,"char":6,"total":14},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":11,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481599,"ai":0,"timings":{"prep_ms":0,"query_ms":0.034,"output_ms":0.013,"total_ms":0.047,"cloud_ms":0.047}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.047}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"w.ctx":{"title":"wtitle [x]","family":"wfam","units":"wunits","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["w.a"],"dimensions":["p1","p2","p3"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"wl":["a"]}}},"searches":{"strings":8,"char":6,"total":14},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":11,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481599,"ai":0,"timings":{"prep_ms":0,"query_ms":0.015,"output_ms":0.01,"total_ms":0.025,"cloud_ms":0.025}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.025}}`,
		},
		flight: [2][2]int64{{1791481600, 1791481600}, {1791481600, 1791481600}},
		lines:  [2]int{60, 60},
	},
	"window/old": {
		target: "/api/v2/q?q=**&after=1791481380&before=1791481390",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"w.ctx":{"title":"wtitle [x]","family":"wfam","units":"wunits","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["w.old"],"dimensions":["o1"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"wl":["old"]}}},"searches":{"strings":6,"char":6,"total":12},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":11,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481599,"ai":0,"timings":{"prep_ms":0,"query_ms":0.045,"output_ms":0.022,"total_ms":0.067,"cloud_ms":0.067}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.067}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"w.ctx":{"title":"wtitle [x]","family":"wfam","units":"wunits","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["w.old"],"dimensions":["o1"],"labels":{"_collect_plugin":["fixture-pusher"],"_collect_module":["corpus"],"wl":["old"]}}},"searches":{"strings":6,"char":6,"total":12},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":11,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481599,"ai":0,"timings":{"prep_ms":0,"query_ms":0.035,"output_ms":0.025,"total_ms":0.06,"cloud_ms":0.06}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.06}}`,
		},
		flight: [2][2]int64{{1791481600, 1791481600}, {1791481600, 1791481600}},
		lines:  [2]int{60, 60},
	},
	"window/between": {
		target: "/api/v2/q?q=**&after=1791481381&before=1791481390",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"w.ctx":{"title":"wtitle [x]","family":"wfam","units":"wunits","matched":["id","title","units","families"]}},"searches":{"strings":4,"char":0,"total":4},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":11,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481599,"ai":0,"timings":{"prep_ms":0,"query_ms":0.007,"output_ms":0.01,"total_ms":0.017,"cloud_ms":0.017}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.017}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":0,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"w.ctx":{"title":"wtitle [x]","family":"wfam","units":"wunits","matched":["id","title","units","families"]}},"searches":{"strings":4,"char":0,"total":4},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":11,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481599,"ai":0,"timings":{"prep_ms":0,"query_ms":0.008,"output_ms":0.007,"total_ms":0.015,"cloud_ms":0.015}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.015}}`,
		},
		flight: [2][2]int64{{1791481600, 1791481600}, {1791481600, 1791481600}},
		lines:  [2]int{53, 53},
	},
	"restart/stored-labels": {
		target: "/api/v2/q?q=k",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["labels"],"labels":{"k":["v1","v2"]}}},"searches":{"strings":15,"char":2,"total":17},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":2,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481626,"ai":0,"timings":{"prep_ms":0,"query_ms":0.187,"output_ms":0.037,"total_ms":0.224,"cloud_ms":0.224}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.224}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["labels"],"labels":{"k":["v2","v1"]}}},"searches":{"strings":15,"char":2,"total":17},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":2,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481626,"ai":0,"timings":{"prep_ms":0,"query_ms":0.117,"output_ms":0.013,"total_ms":0.13,"cloud_ms":0.13}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.13}}`,
		},
		flight: [2][2]int64{{1791481626, 1791481626}, {1791481626, 1791481626}},
		lines:  [2]int{62, 62},
	},
	"restart/stored-name": {
		target: "/api/v2/q?q=q_a",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["instances"],"instances":["q_a_name"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":2,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481626,"ai":0,"timings":{"prep_ms":0,"query_ms":0.032,"output_ms":0.02,"total_ms":0.052,"cloud_ms":0.052}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.052}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["instances"],"instances":["q_a_name"]}},"searches":{"strings":15,"char":0,"total":15},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":2,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481626,"ai":0,"timings":{"prep_ms":0,"query_ms":0.011,"output_ms":0.006,"total_ms":0.017,"cloud_ms":0.017}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.017}}`,
		},
		flight: [2][2]int64{{1791481626, 1791481626}, {1791481626, 1791481626}},
		lines:  [2]int{60, 60},
	},
	"restart/nameless/null-name": {
		target: "/api/v2/q?q=z",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"]}},"searches":{"strings":16,"char":0,"total":16},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":2,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481635,"ai":0,"timings":{"prep_ms":0,"query_ms":0.224,"output_ms":0.03,"total_ms":0.254,"cloud_ms":0.254}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.254}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"]}},"searches":{"strings":16,"char":0,"total":16},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":2,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481635,"ai":0,"timings":{"prep_ms":0,"query_ms":0.167,"output_ms":0.028,"total_ms":0.195,"cloud_ms":0.195}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.195}}`,
		},
		flight: [2][2]int64{{1791481635, 1791481635}, {1791481635, 1791481635}},
		lines:  [2]int{59, 59},
	},
	"restart/nameless/empty-name": {
		target: "/api/v2/q?q=inc",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"]}},"searches":{"strings":16,"char":0,"total":16},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":2,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481635,"ai":0,"timings":{"prep_ms":0,"query_ms":0.011,"output_ms":0.007,"total_ms":0.018,"cloud_ms":0.018}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.018}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"matched":["dimensions"]}},"searches":{"strings":16,"char":0,"total":16},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":2,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481635,"ai":0,"timings":{"prep_ms":0,"query_ms":0.01,"output_ms":0.007,"total_ms":0.017,"cloud_ms":0.017}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.017}}`,
		},
		flight: [2][2]int64{{1791481635, 1791481635}, {1791481635, 1791481635}},
		lines:  [2]int{59, 59},
	},
	"restart/nameless/names-left": {
		target: "/api/v2/q?q=**",
		bodies: [2]string{
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","family":"fam","units":"units","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["q_a_name","q.two"],"dimensions":["alpha","b","... 2 dimensions more"],"labels":{"_collect_plugin":["fixture-pusher"],"k":["v1","v2"],"_collect_module":["corpus"]}}},"searches":{"strings":13,"char":12,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":2,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481635,"ai":0,"timings":{"prep_ms":0,"query_ms":0.031,"output_ms":0.013,"total_ms":0.044,"cloud_ms":0.044}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.044}}`,
			`{"api":2,"nodes":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nm":"parity-parent","ni":0,"st":{"ai":0,"code":200,"msg":""}},{"mg":"5a1e0000-0000-4000-8000-0000000000bb","nm":"parity-child","ni":1,"st":{"ai":0,"code":200,"msg":""}}],"contexts":{"q.ctx":{"title":"title [x]","family":"fam","units":"units","matched":["id","title","units","families","instances","dimensions","labels"],"instances":["q_a_name","q.two"],"dimensions":["alpha","b","... 2 dimensions more"],"labels":{"k":["v2","v1"],"_collect_module":["corpus"],"_collect_plugin":["fixture-pusher"]}}},"searches":{"strings":13,"char":12,"total":25},"versions":{"routing_hard_hash":1,"nodes_hard_hash":2,"contexts_hard_hash":2,"contexts_soft_hash":0,"alerts_hard_hash":0,"alerts_soft_hash":0},"agents":[{"mg":"5a1e0000-0000-4000-8000-0000000000aa","nd":null,"nm":"parity-parent","now":1791481635,"ai":0,"timings":{"prep_ms":0,"query_ms":0.015,"output_ms":0.009,"total_ms":0.024,"cloud_ms":0.024}}],"timings":{"routing_ms":0,"node_max_ms":0,"total_ms":0.024}}`,
		},
		flight: [2][2]int64{{1791481635, 1791481635}, {1791481635, 1791481635}},
		lines:  [2]int{69, 69},
	},
}
