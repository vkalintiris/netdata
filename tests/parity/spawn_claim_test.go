// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"slices"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// The cloud.conf URLs of plugins.spawn's `env-claim` (reserved names: nothing resolves them, and an agent that is not
// claimed connects to none): the one the agents start with, and the one written before the claim reload.
const (
	spawnClaimStart  = "https://cloud-start.parity.invalid"
	spawnClaimReload = "https://cloud-reload.parity.invalid"
)

// spawnClaimURL is the line of NETDATA_REGISTRY_CLOUD_BASE_URL with url, as a child's environment shows it.
func spawnClaimURL(url string) string { return "NETDATA_REGISTRY_CLOUD_BASE_URL=" + url }

// spawnEnvClaim is `env-claim`: the one change the agent makes to its children's environment while it runs. Both
// agents start with cloud.conf's url spawnClaimStart (exported at config load, registry_init.c:87 →
// registry.c:158-166); then each side's cloud.conf gets spawnClaimReload and `netdatacli reload-claiming-state` runs
// (an agent that is not claimed: commands.c:222-241 → claim.c:197-205: cloud.conf loaded again, cloud-conf.c:71-92,
// then registry_update_cloud_base_url(), which exports the new URL with nd_setenv, registry.c:164). The labels reload
// that follows runs get-kubernetes-labels.sh again, and the spawn server's client sends the environment of that
// moment with the request (spawn_server_nofork.c:1789: `environ`), so that run alone shows the new URL; the two runs
// of system-info.sh and the first of get-kubernetes-labels.sh, all at startup, show the first. The rest of each
// child's environment and the variant's outputs are compared as in every variant (runSpawnEnv).
func spawnEnvClaim() spawnEnvVariant {
	start, reload := spawnClaimURL(spawnClaimStart), spawnClaimURL(spawnClaimReload)
	return spawnEnvVariant{
		adjust: func(t *testing.T, o *daemon.Options, probes string) {
			o.PluginsDir = fmt.Sprintf("%q", probes)
			cloudConfAt(t, o.RunDir, spawnClaimStart)
		},
		reload: func(t *testing.T, s spawnSide) {
			cloudConfAt(t, s.d.Opts.RunDir, spawnClaimReload)
			// the command answers once the registry has its new copy (claim.c:197-205, unclaimed: no wait); the
			// oracle's answer is regReloadAnswer's, and the candidate's must be the same
			if r := runCLI(t, s.d, "reload-claiming-state"); r != regReloadAnswer {
				if s.role == Oracle {
					t.Fatalf("oracle: netdatacli reload-claiming-state: %+v, want %+v", r, regReloadAnswer)
				}
				t.Errorf("candidate: netdatacli reload-claiming-state: %+v, the oracle's %+v", r, regReloadAnswer)
			}
		},
		runs: map[string][][]string{
			"system-info.sh":           {{start}, {start}},
			"get-kubernetes-labels.sh": {{start}, {reload}},
		},
	}
}

// spawnRunsProblems is what the oracle's runs of a script lack of a variant's per-run lines (want: run j's lines;
// envs: each run's environment as spawnEnvView shows it): fewer runs than want names, or a line a run does not hold.
func spawnRunsProblems(script string, want, envs [][]string) []string {
	var out []string
	if len(envs) < len(want) {
		out = append(out, fmt.Sprintf("%s ran %d times, want %d", script, len(envs), len(want)))
	}
	for j, lines := range want {
		if j >= len(envs) {
			break
		}
		for _, l := range lines {
			if !slices.Contains(envs[j], l) {
				out = append(out, fmt.Sprintf("%s run %d: no %q in its environment", script, j+1, l))
			}
		}
	}
	return out
}
