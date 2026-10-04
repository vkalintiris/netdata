// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

// healthSilencersFile is side i's silencers file as the checks compare it: its bytes, quoted, or that it is missing
// (`<varlib>/health.silencers.json`, health_silencers.c:417-459).
func (h *healthPair) silencersFile(i int) string {
	b, err := os.ReadFile(filepath.Join(h.p.Each()[i].Daemon.Opts.RunDir, "lib", "health.silencers.json"))
	if err != nil {
		return "the silencers file: " + h.n[i].paths(err.Error())
	}
	return "the silencers file: " + strconv.Quote(string(b))
}

// healthFlags is a view of /api/v1/alarms?all as each alert's flags: `name silenced=… disabled=…`.
func (h *healthPair) flags(i int) string {
	v := h.get(i, "/api/v1/alarms?all")
	out := []string{strings.SplitN(v, "\n", 2)[0]}
	for _, m := range healthAlarmFlagsRe.FindAllStringSubmatch(healthBody(v), -1) {
		out = append(out, fmt.Sprintf("%s disabled=%s silenced=%s", m[1], m[2], m[3]))
	}
	return strings.Join(out, "\n")
}

// healthSilencersList is what the oracle must answer LIST with (health_silencers.c:237-288, the text the silencers
// file holds too): whether everything is silenced or disabled, how, and the selectors, the last added first
// (:47-58).
func healthSilencersList(all, kind string, selectors ...string) string {
	list := "[]"
	if len(selectors) > 0 {
		var items []string
		for _, s := range selectors {
			items = append(items, "\t\t{\n\t\t\t"+s+"\n\t\t}")
		}
		list = "[\n" + strings.Join(items, ",\n") + "\n\t]"
	}
	return "HTTP 200, application/json; charset=utf-8\n{\n\t\"all\": " + all + ",\n\t\"type\": \"" + kind + "\",\n\t\"silencers\": " + list + "\n}\n"
}

// TestHealthSilencers (check `health.silencers`, M9 commit 0, D183): the management API (`/api/v1/manage/health`,
// health_silencers.c:316-409) driven with the same fixed key on both agents: a request without a token and one with a
// wrong token, LIST, SILENCE ALL, RESET, a SILENCE and a DISABLE with selectors, and the two paths the endpoint
// refuses. Each request is sent once to each side (a command changes the silencers) and its status, content type and
// body compared, then the silencers file's bytes; after the commands that change what an alert shows, the alerts'
// `silenced` and `disabled` flags.
func TestHealthSilencers(t *testing.T) {
	token := "X-Auth-Token: " + healthKey
	runHealthCases(t, map[string]healthCase{
		"api": {
			conf: healthCalcConf,
			sc:   healthValues("hsig.values", "hsig.ctx", []string{"a"}, map[string]int64{"a": 70}),
			play: func(t *testing.T, h *healthPair) {
				h.create(t)
				h.waitOracle(t, "the chart's alert", func() (string, error) {
					v := h.get(0, "/api/v1/alarms?all")
					return v, healthAll("WARNING", "hs_calc")(v)
				})
				// each step: the request, what the oracle must answer, whether the oracle writes the file
				mgmt := "/api/v1/manage/health?"
				for _, s := range []struct {
					label, path string
					headers     []string
					want        string
					flags       string
				}{
					{"no token", mgmt + "cmd=LIST", nil, "HTTP 403, text/plain; charset=utf-8\nAuth Error\n", ""},
					{"a wrong token", mgmt + "cmd=LIST", []string{"X-Auth-Token: " + strings.Repeat("0", 36)}, "HTTP 403, text/plain; charset=utf-8\nAuth Error\n", ""},
					{"LIST at the start", mgmt + "cmd=LIST", []string{token}, healthSilencersList("false", "None"), ""},
					{"SILENCE ALL", mgmt + "cmd=" + url.QueryEscape("SILENCE ALL"), []string{token}, "HTTP 200, text/plain; charset=utf-8\nAll alarm notifications are silenced\n", "hs_calc disabled=false silenced=true"},
					{"LIST after SILENCE ALL", mgmt + "cmd=LIST", []string{token}, healthSilencersList("true", "SILENCE"), ""},
					{"RESET", mgmt + "cmd=RESET", []string{token}, "HTTP 200, text/plain; charset=utf-8\nAll health checks and notifications are enabled\n", "hs_calc disabled=false silenced=false"},
					{"SILENCE with a selector", mgmt + "cmd=SILENCE&alarm=hs_calc", []string{token}, "HTTP 200, text/plain; charset=utf-8\nAlarm notifications silenced for alarms matching the selectors\nAlarm selector added\n", "hs_calc disabled=false silenced=true"},
					{"DISABLE with a selector", mgmt + "cmd=DISABLE&context=hsig.ctx", []string{token}, "HTTP 200, text/plain; charset=utf-8\nHealth checks disabled for alarms matching the selectors\nAlarm selector added\n", "hs_calc disabled=true silenced=false"},
					{"LIST with selectors", mgmt + "cmd=LIST", []string{token}, healthSilencersList("false", "DISABLE", `"context": "hsig.ctx"`, `"alarm": "hs_calc"`), ""},
					{"RESET with selectors", mgmt + "cmd=RESET", []string{token}, "HTTP 200, text/plain; charset=utf-8\nAll health checks and notifications are enabled\n", "hs_calc disabled=false silenced=false"},
					{"another path under manage", "/api/v1/manage/other?cmd=LIST", []string{token}, "HTTP 404, text/plain; charset=utf-8\nInvalid management request. Curently only 'health' is supported.", ""},
					{"a longer path", "/api/v1/manage/health/more?cmd=LIST", []string{token}, "HTTP 404, text/plain; charset=utf-8\nInvalid management request. Currently only 'health' is supported.", ""},
				} {
					var got [2]string
					for i := range got {
						got[i] = h.plain(i, s.path, s.headers...) + "\n" + h.silencersFile(i)
					}
					if !strings.HasPrefix(got[0], s.want) {
						t.Fatalf("oracle: %s: %s answered\n%s\nwant\n%s", s.label, s.path, got[0], s.want)
					}
					if got[0] != got[1] {
						t.Fatalf("%s: %s differs\noracle:\n%s\ncandidate:\n%s", s.label, s.path, got[0], got[1])
					}
					t.Logf("%s, both sides:\n%s", s.label, got[0])
					if s.flags != "" {
						h.compareNow(t, s.label+": the alerts' flags", h.flags, func(oracle string) error {
							if !strings.Contains(oracle, s.flags) {
								return fmt.Errorf("want %q", s.flags)
							}
							return nil
						})
					}
				}
				// the oracle wrote its file (every authorized command but LIST rewrites it)
				if f := h.silencersFile(0); !strings.HasPrefix(f, `the silencers file: "`) {
					t.Fatalf("oracle: %s", f)
				}
			},
		},
	})
}
