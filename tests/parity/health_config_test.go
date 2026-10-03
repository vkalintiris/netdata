// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// healthCfgConf are the config checks' rules: an alarm on a chart, a template on a context, two rules of one name (a
// chain: one DynCfg job, two rules), a rule with a keyword C does not know (the rule still loads, with a record), and
// a rule `[health] enabled alarms` leaves out (healthCfgExtra).
const healthCfgConf = `# the config checks' rules
 alarm: hc_alarm
    on: hcfg.values
  calc: $a
 every: 1s
  warn: $this > 50
  crit: $this > 90
 units: things
  info: an alarm on a chart

template: hc_tmpl
      on: hcfg.ctx
  lookup: max -3s unaligned of a
   every: 1s
    warn: $this > 50
   units: things
    info: a template on a context

template: hc_chain
      on: hcfg.ctx
    calc: $a
   every: 1s
    warn: $this > 60
    info: the first rule of a chain

template: hc_chain
      on: hcfg.other
    calc: $a
   every: 1s
    warn: $this > 70
    info: the second rule of a chain

 alarm: hc_bad
    on: hcfg.values
  calc: $a
 every: 1s
 bogus: nothing
  warn: $this > 80
  info: a rule with an unknown keyword

 alarm: hc_off
    on: hcfg.values
  calc: $a
 every: 1s
  warn: $this > 85
  info: a rule the enabled alarms pattern leaves out
`

const healthCfgExtra = "    enabled alarms = !hc_off *\n"

// healthJobPrefix is the id of health's DynCfg template; a job's id adds `:<alert name>` (health_dyncfg.c:886-943).
const healthJobPrefix = "health:alert:prototype"

var healthJobRe = regexp.MustCompile(`"(` + healthJobPrefix + `[^"]*)":`)

// config is side i's view of a DynCfg answer (`/api/v1/config?<query>`, as the admin: health's nodes are not open to
// anonymous clients, dyncfg.c:395-399): the status, the content type and the body with the side's directories
// replaced and its wall-clock values ranked (dcClock).
func (h *healthPair) config(i int, query string, since time.Time) string {
	r := healthGet(h.p.Each()[i].Daemon, "/api/v1/config?"+query, dcAdmin)
	body := dcClock([]string{h.n[i].paths(string(r.Body))}, since.Add(-time.Minute), time.Now().Add(time.Minute))[0]
	return healthView(r, body)
}

// healthJobs are the ids of health's DynCfg nodes a tree view lists, in the tree's order.
func healthJobs(view string) []string {
	var out []string
	for _, m := range healthJobRe.FindAllStringSubmatch(healthBody(view), -1) {
		out = append(out, m[1])
	}
	return out
}

// healthConfigRecords are the main thread's records about the health configuration (health/health_config.c): what
// the reader refused or ignored, in file order.
func healthConfigRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if threadOf(l) == "" && (strings.Contains(l, `msg="Health configuration`) || strings.Contains(l, `msg="HEALTH`) ||
			strings.Contains(l, `msg="Health `)) {
			out = append(out, normalizeLog(l, d.Opts.RunDir, ""))
		}
	}
	return out
}

// healthRows counts a table's rows in a running agent's metadata database, through metadata-dump on a copy of its
// files. A copy taken while the agent writes may not open: that is an error, and the caller polls again.
func healthRows(t *testing.T, d *daemon.Daemon, table string) (int, error) {
	t.Helper()
	dir, err := os.MkdirTemp("", "health-rows-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(dir)
	db := filepath.Join(d.Opts.RunDir, "cache", "netdata-meta.db")
	for _, suffix := range []string{"", "-wal", "-shm"} {
		if b, err := os.ReadFile(db + suffix); err == nil {
			if err := os.WriteFile(filepath.Join(dir, "db"+suffix), b, 0o644); err != nil {
				t.Fatal(err)
			}
		}
	}
	out, err := exec.Command(metadataDump(t), filepath.Join(dir, "db"), "--table", table).CombinedOutput()
	if err != nil {
		return 0, fmt.Errorf("metadata-dump: %v: %s", err, out)
	}
	return strings.Count(string(out), "\nrow "+table+" "), nil
}

// waitRows waits until both agents stored a table's rows: the rules' alert_hash rows are written by the metadata
// thread some seconds after the start, and C drops what is still queued when it stops (sqlite_metadata.c:3043-3044),
// so a stop right after the start would compare how far each side got. The oracle must hold at least `least` rows,
// the same count twice in a row; the candidate gets the bounded wait to hold as many.
func (h *healthPair) waitRows(t *testing.T, table string, least int) {
	t.Helper()
	last, want := -1, 0
	h.waitOracle(t, "the stored "+table+" rows", func() (string, error) {
		n, err := healthRows(t, h.p.Oracle, table)
		stable := err == nil && n >= least && n == last
		last, want = n, n
		if !stable {
			return "", fmt.Errorf("%d rows (%v), want at least %d, the same twice", n, err, least)
		}
		return "", nil
	})
	for end := time.Now().Add(healthCandidateWait); time.Now().Before(end); time.Sleep(250 * time.Millisecond) {
		if n, err := healthRows(t, h.p.Candidate, table); err == nil && n == want {
			return
		}
	}
}

// healthHashRows are a side's alert_hash rows after the stop.
func (h *healthPair) hashRows(t *testing.T, i int) []string {
	t.Helper()
	var out []string
	for _, l := range h.n[i].healthDump(t, h.p.Each()[i].Daemon) {
		if strings.HasPrefix(l, "row alert_hash ") {
			out = append(out, l)
		}
	}
	return out
}

// TestHealthConfig (check `health.config`, M9 commit 0, D183): what the agents load from health.d. `user`: one user
// file of five kinds of rules, the stock rules off; `stock`: the installed stock rules, no user file and no chart they
// match. Compared: health's DynCfg nodes (the tree under /health, as the admin), the main thread's records about the
// configuration, `/api/v1/alarms?all` (`user`: the alerts linked to the fake plugin's chart, with their config hashes)
// and, both stopped, the alert_hash rows (one per rule; the hash covers the rule but its source,
// health_dyncfg.c:359-363).
func TestHealthConfig(t *testing.T) {
	started := time.Now()
	tree := func(h *healthPair) func(i int) string {
		return func(i int) string { return h.config(i, "action=tree&path=/health", started) }
	}
	jobs := func(least int, names ...string) func(string) error {
		return func(view string) error {
			if !strings.HasPrefix(view, fmt.Sprintf("HTTP %d, ", http.StatusOK)) {
				return fmt.Errorf("answered %q", strings.SplitN(view, "\n", 2)[0])
			}
			got := healthJobs(view)
			for _, n := range names {
				if !slices.Contains(got, healthJobPrefix+":"+n) {
					return fmt.Errorf("no job of %s among %v", n, got)
				}
			}
			if !slices.Contains(got, healthJobPrefix) || len(got) < least+1 {
				return fmt.Errorf("%d health nodes, want the template and at least %d jobs: %v", len(got), least, got)
			}
			return nil
		}
	}
	records := func(want ...string) func(t *testing.T, h *healthPair) {
		return func(t *testing.T, h *healthPair) {
			h.compareLines(t, "the records about the health configuration",
				func(i int) []string { return healthConfigRecords(t, h.p.Each()[i].Daemon) },
				func(oracle []string) error {
					for _, w := range want {
						if !strings.Contains(strings.Join(oracle, "\n"), w) {
							return fmt.Errorf("no record holds %q", w)
						}
					}
					return nil
				})
		}
	}
	runHealthCases(t, map[string]healthCase{
		"user": {
			conf:  healthCfgConf,
			extra: healthCfgExtra,
			sc:    healthValues("hcfg.values", "hcfg.ctx", []string{"a"}, map[string]int64{"a": 10}),
			play: func(t *testing.T, h *healthPair) {
				h.compareNow(t, "the DynCfg tree under /health", tree(h), jobs(4, "hc_alarm", "hc_tmpl", "hc_chain", "hc_bad"))
				h.create(t)
				// hc_off is not enabled, and hc_chain's second rule matches no chart
				h.compareNow(t, "/api/v1/alarms?all", func(i int) string { return h.get(i, "/api/v1/alarms?all") },
					healthWant(map[string]string{"hc_alarm": "CLEAR", "hc_tmpl": "CLEAR", "hc_chain": "CLEAR", "hc_bad": "CLEAR"}))
				h.waitRows(t, "alert_hash", 6)
			},
			after: func(t *testing.T, h *healthPair) {
				records("has unknown key 'bogus'")(t, h)
				h.compareLines(t, "the alert_hash rows", func(i int) []string { return h.hashRows(t, i) },
					func(oracle []string) error {
						// one row per rule read: hc_alarm, hc_tmpl, hc_chain's two, hc_bad, and hc_off's (a rule that is
						// not enabled is stored too)
						if len(oracle) != 6 {
							return fmt.Errorf("%d rows, want 6", len(oracle))
						}
						return nil
					})
			},
		},
		"stock": {
			stock: true,
			play: func(t *testing.T, h *healthPair) {
				h.compareNow(t, "the DynCfg tree under /health", tree(h), jobs(100))
				h.waitRows(t, "alert_hash", 100)
			},
			after: func(t *testing.T, h *healthPair) {
				records()(t, h)
				h.compareLines(t, "the alert_hash rows", func(i int) []string { return h.hashRows(t, i) },
					func(oracle []string) error {
						if len(oracle) < 100 {
							return fmt.Errorf("%d rows, want at least 100", len(oracle))
						}
						return nil
					})
			},
		},
	})
}
