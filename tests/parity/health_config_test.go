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

// healthAdvRules is the `adversarial` case's file but its three long lines (healthAdvConf): about one rule per record
// the reader, its sub-parsers and the validator write (health/health_config.c, health_prototypes.c:418-456) and per
// shape of an alert_hash row (database/sqlite/sqlite_health.c:901-1010). No rule can run its `exec`: no chart exists.
const healthAdvRules = `# the adversarial file: a key before the first rule, then three lines that are no key and value
   every: 1s
a line without a colon
: a value without a key
novalue:

 alarm: adv_plain
    on: hadv.chart
  calc: $a
 every: 1s

template: adv_full
      on: hadv.ctx
   class: Errors
    type: System
component: Parity
  lookup: average -10s unaligned absolute null2zero match-ids match-names of a,b
   every: 5s
    calc: $this * 2
   units: things
    warn: $this > 50
    crit: $this > 90
    exec: /dev/null/adv-exec
      to: sysadmin
 summary: a full rule
    info: every key a rule can set
   delay: up 1m down 2m multiplier 1.5 max 1h
 options: no-clear-notification
  repeat: warning 2m critical 30s
      os: linux
   hosts: parity-*
host labels: tier=prod
  plugin: difftest
  module: m1
chart labels: kind=x

 alarm: adv renamed
    on: hadv.chart
  calc: $a
 every: 1s

 alarm: adv_twice
    on: hadv.chart
    on: hadv.other
  calc: $a
 every: 1s
  info: first
  info: first
  info: second

template: adv_labels
      on: hadv.ctx
    calc: $a
   every: 1s
      os: *
   hosts: !* *
host labels: nolabel
chart labels: a=b
chart labels: c=d
  plugin: p1

template: adv_off
      on: hadv.ctx
    calc: $a
   every: 1s
  module: !*

 alarm: adv_quotes
    on: hadv.chart
  calc: $a
 every: 1s
 class: "Errors"
  type: 'System'
component: "Par'ity"
 units: "things"
  exec: "/dev/null/adv-exec" 'arg'
    to: "a" 'b'
summary: "quoted"
  info: "x"

 alarm: adv_every
    on: hadv.chart
  calc: $a
 every: 1s
 every: soon

 alarm: adv_green
    on: hadv.chart
  calc: $a + $green
 every: 1s
 green: 10
   red: 20abc
  warn: $this > $green
  crit: $this > $red

template: adv_badexpr
      on: hadv.ctx
  lookup: max -5s
    calc: $this +
    warn: ($this > 1
    crit: $this > > 2

 alarm: adv_badgood
    on: hadv.chart
  calc: $a +
  calc: $a
 every: 1s

 alarm: adv_unknown
    on: hadv.chart
  calc: $a
 every: 1s
 bogus: nothing
families: *
charts: x
Families: *

 ALARM: adv_case
    On: hadv.chart
  CALC: $a
 Every: 1s
 UNITS: things

 alarm: adv_delay_bad
    on: hadv.chart
  calc: $a
 every: 1s
 delay: up soon down 5s multiplier -1 max never jitter 3

 alarm: adv_delay_up
    on: hadv.chart
  calc: $a
 every: 1s
 delay: up 10s multiplier 2.25

 alarm: adv_options
    on: hadv.chart
  calc: $a
 every: 1s
options: no-clear bogus-option NO-CLEAR-NOTIFICATION

 alarm: adv_repeat_bad
    on: hadv.chart
  calc: $a
 every: 1s
repeat: warning soon critical -5s other 7

 alarm: adv_repeat_off
    on: hadv.chart
  calc: $a
 every: 1s
repeat: warning 5s off

template: adv_lk_nothing
      on: hadv.ctx
  lookup: average
   every: 1s
    calc: $a

template: adv_lk_method
      on: hadv.ctx
  lookup: bogusmethod -5s
   every: 1s
    calc: $a

template: adv_lk_noclose_num
      on: hadv.ctx
  lookup: countif(>5 -5s
   every: 1s
    calc: $a

template: adv_lk_badchar
      on: hadv.ctx
  lookup: countif(x) -5s
   every: 1s
    calc: $a

template: adv_lk_noclose
      on: hadv.ctx
  lookup: countif(>
   every: 1s
    calc: $a

template: adv_lk_after
      on: hadv.ctx
  lookup: average soon
   every: 1s
    calc: $a

template: adv_lk_at
      on: hadv.ctx
  lookup: average -5s at soon
   every: 1s

template: adv_lk_every
      on: hadv.ctx
  lookup: average -5s every soon
   every: 1s

template: adv_lk_keyword
      on: hadv.ctx
  lookup: average -5s bogusword of a,b foreach c

template: adv_lk_countif
      on: hadv.ctx
  lookup: countif(>=5) -10s at -5s every 3s of all

template: adv_lk_ne
      on: hadv.ctx
  lookup: countif(!0) -10s percentage min of a

template: adv_lk_lt
      on: hadv.ctx
  lookup: countif( <-.5 ) -10s anomaly-bit max

template: adv_lk_default
      on: hadv.ctx
  lookup: countif() -10s min2max abs

template: adv_lk_pctile
      on: hadv.ctx
  lookup: percentile -1m average sum

template: adv_v_context
    calc: $a
   every: 1s

 alarm: adv_v_instance
  calc: $a
 every: 1s

 alarm: adv_v_every
    on: hadv.chart
 every: 5s
lookup: bogusmethod -5s
  calc: $a

 alarm: adv_v_nothing
    on: hadv.chart
 every: 1s

template: adv_chain
      on: hadv.ctx
    calc: $a
   every: 1s

template: adv_chain
    calc: $a
   every: 2s

template: adv_chain
      on: hadv.other
    calc: $a
   every: 3s

 alarm: adv_same
    on: hadv.chart
  calc: $a
 every: 1s

 alarm: adv_between
    on: hadv.chart
  calc: $a
 every: 1s

 alarm: adv_same
    on: hadv.chart
  calc: $a
 every: 1s

 alarm: adv_cont
    on: hadv.chart
  calc: $a \
        + 1
 every: 1s
  info: one \
# two \
        three

# a comment that ends in a backslash \
 alarm: adv_after_comment
    on: hadv.chart
  calc: $a
 every: 1s

 alarm: adv_long
    on: hadv.chart
  calc: $a
 every: 1s
  info: {long}

 alarm: adv_join
    on: hadv.chart
  calc: $a
 every: 1s
summary: {join}\
{joined}

 alarm: adv_eof
    on: hadv.chart
  calc: $a
 every: 1s
eofkey: at the end \`

// What C makes of the adversarial file (observed on the oracle, H16's probes):
//   - healthAdvRows rows: one per rule the validator accepts, and one for adv_same's two rules, which differ only in
//     their source and so share a hash (INSERT OR REPLACE: the later rule's row, after adv_between's);
//   - healthAdvRecords records, each family of which healthAdvTexts names once, with the rule or the value that drew
//     it. Two records of C cannot be drawn from a file: `too long multi-line` (health_config.c:668-673: a read ends
//     at byte 4,095 of the buffer, so the join never reaches 4,096) and the validator's `non-finite delay
//     multiplier` (health_config.c:47-57: the reader replaces such a multiplier with 1). `cannot read file` is
//     `walk`'s.
const (
	healthAdvRows    = 41
	healthAdvRecords = 46
)

var healthAdvTexts = []string{
	// the reader's lines (health_config.c:677-747, :885-889)
	"has unknown key 'every'. Expected either 'alarm' or 'template'.",
	"line 3 of file '<RUN>/etc/health.d/parity.conf'. It does not contain a ':'. Ignoring it.",
	// the rest of adv_long's line of 5,000 bytes and of adv_join's joined line, each a line of its own (a line is a
	// read: adv_long's `info` is the file's line 290 and is counted twice), and the pass at the end of the file
	"line 291 of file '<RUN>/etc/health.d/parity.conf'. It does not contain a ':'. Ignoring it.",
	"line 299 of file '<RUN>/etc/health.d/parity.conf'. It does not contain a ':'. Ignoring it.",
	"at line 306 of file '<RUN>/etc/health.d/parity.conf' for alarm/template 'adv_eof' has unknown key 'eofkey'.",
	"Keyword is empty. Ignoring it.",
	"value is empty. Ignoring it.",
	"Health configuration renamed alarm 'adv renamed' to 'adv_renamed'",
	"for alarm/template 'adv_unknown' has unknown key 'bogus'.",
	"for alarm/template 'adv_unknown' has unknown key 'Families'.",
	// its keys (:517-571, :787-849)
	"for alarm 'adv_twice' has key 'on' twice, once with value 'hadv.chart' and later with value 'hadv.other'. Using ('hadv.other').",
	"for alarm 'adv_twice' has key 'info' twice, once with value 'first' and later with value 'second'. Using ('second').",
	"for alarm 'adv_labels' has key 'host labels' with value 'nolabel' that does not match label=pattern. Ignoring it.",
	"for alarm 'adv_every' at key 'every' cannot parse duration: 'soon'.",
	"for alarm 'adv_green' at key 'red' leaves this string unmatched: 'abc'.",
	// an expression: the evaluator's record, then the reader's, at each of the three keys
	"failed to parse expression '$a +': remaining characters after expression at character 4 (i.e.: '+').",
	"for alarm 'adv_badgood' at key 'calc' has non-parseable expression '$a +': remaining characters after expression at '+'",
	"failed to parse expression '$this +': ",
	"for alarm 'adv_badexpr' at key 'calc' has non-parseable expression '$this +': ",
	"failed to parse expression '($this > 1': ",
	"for alarm 'adv_badexpr' at key 'warn' has non-parseable expression '($this > 1': ",
	"failed to parse expression '$this > > 2': ",
	"for alarm 'adv_badexpr' at key 'crit' has non-parseable expression '$this > > 2': ",
	// delay, options, repeat (:6-185)
	"invalid value 'soon' for 'up' keyword",
	"invalid value '-1' for 'multiplier' keyword",
	"unknown keyword 'jitter'",
	"Ignoring unknown alarm option 'bogus-option'",
	"invalid value 'soon' for 'warning' keyword",
	"negative value '-5s' for 'critical' keyword",
	// lookup (:187-418)
	"expected group method followed by the 'after' time, but got 'average'",
	"invalid group method 'bogusmethod'",
	"missing closing parenthesis after number in aggregation method on 'countif'",
	"invalid character 'x' in aggregation method options on 'countif'",
	"missing closing parenthesis after aggregation method on 'countif'",
	"invalid duration 'soon' after group method",
	"invalid duration 'soon' for 'at' keyword",
	"invalid duration 'soon' for 'every' keyword",
	"unknown keyword 'bogusword'",
	"unknown keyword 'foreach'",
	// the validator (health_prototypes.c:418-456); adv_chain's is the second of three rules of one name
	"HEALTH: alert 'adv_v_context' rule 0 is invalid: missing match 'on' parameter for context. Source: line=",
	"HEALTH: alert 'adv_v_instance' rule 0 is invalid: missing match 'on' parameter for instance. Source: line=",
	"HEALTH: alert 'adv_v_every' rule 0 is invalid: missing update frequency. Source: line=",
	"HEALTH: alert 'adv_v_nothing' rule 0 is invalid: no db lookup, calculation and warning/critical conditions. Source: line=",
	"HEALTH: alert 'adv_chain' rule 0 is invalid: missing match 'on' parameter for context. Source: line=",
}

// healthAdvConf is the `adversarial` case's file: healthAdvRules with a line of 5,000 bytes (C reads 4,095 bytes at a
// time: the rest is a line of its own, health_config.c:612, :658), a continuation whose second line passes the
// buffer's end (the same cut, after the bytes already held), and no newline after its last line, which ends in a
// backslash (the pass at the end of the file counts as one more line, health_config.c:658-660).
func healthAdvConf() string {
	return strings.NewReplacer("{long}", strings.Repeat("a", 5000), "{join}", strings.Repeat("s", 3000),
		"{joined}", strings.Repeat("t", 2000)).Replace(healthAdvRules)
}

// healthWalkRule is a file of the `walk` case: one rule, named after where the file is.
func healthWalkRule(name string) string {
	return " alarm: " + name + "\n    on: hwalk.chart\n  calc: $a\n every: 1s\n"
}

// healthWalkFiles are the `walk` case's two trees (libnetdata/paths/paths.c:217-325): the user's under etc/health.d,
// the stock one under stock/health.d.
var healthWalkFiles = map[string]string{
	// a user file, and one that shadows the stock file of its name
	"etc/health.d/user.conf":   healthWalkRule("w_user"),
	"etc/health.d/shadow.conf": healthWalkRule("w_user_shadow"),
	// read: a name that starts with a dot. Not read: the suffix is not the name's end, there is no name before it,
	// its case differs
	"etc/health.d/.h.conf":     healthWalkRule("w_hidden"),
	"etc/health.d/x.conf.bak":  healthWalkRule("w_bak"),
	"etc/health.d/.conf":       healthWalkRule("w_no_name"),
	"etc/health.d/upper.CONF":  healthWalkRule("w_upper"),
	"etc/health.d/closed.conf": healthUnreadable,
	// links: to a file and to a directory outside the tree (both followed), and to nothing (the stock file of its
	// name is read: only a regular file shadows)
	"etc/health.d/link-file.conf":   healthLink + "../elsewhere/file.rules",
	"etc/health.d/link-dir":         healthLink + "../elsewhere/dir",
	"etc/health.d/dangling.conf":    healthLink + "../elsewhere/missing",
	"etc/elsewhere/file.rules":      healthWalkRule("w_link_file"),
	"etc/elsewhere/dir/linked.conf": healthWalkRule("w_link_dir"),
	// a directory in both trees (one pass over both, with the shadowing), in the user tree only, and four levels
	// down: the third level's files are read, the fourth is refused
	"etc/health.d/both/user.conf":            healthWalkRule("w_both_user"),
	"etc/health.d/both/shadow.conf":          healthWalkRule("w_both_user_shadow"),
	"etc/health.d/user-dir/user.conf":        healthWalkRule("w_user_dir"),
	"etc/health.d/d1/d2/d3/third.conf":       healthWalkRule("w_third"),
	"etc/health.d/d1/d2/d3/d4/fourth.conf":   healthWalkRule("w_fourth"),
	"stock/health.d/stock.conf":              healthWalkRule("w_stock"),
	"stock/health.d/shadow.conf":             healthWalkRule("w_stock_shadowed"),
	"stock/health.d/dangling.conf":           healthWalkRule("w_stock_dangling"),
	"stock/health.d/both/stock.conf":         healthWalkRule("w_both_stock"),
	"stock/health.d/both/shadow.conf":        healthWalkRule("w_both_stock_shadowed"),
	"stock/health.d/stock-dir/stock.conf":    healthWalkRule("w_stock_dir"),
	"stock/health.d/d1/d2/d3/third-s.conf":   healthWalkRule("w_stock_third"),
	"stock/health.d/d1/d2/d3/d4/fourth.conf": healthWalkRule("w_stock_fourth"),
}

// healthWalkNames are the rules of the `walk` case's rows, sorted: the files read, each once. Not read: the stock file
// a user file shadows (in the top directory and in `both`), the three names the suffix rule leaves out, the file that
// cannot be opened and the fourth level's.
var healthWalkNames = []string{"w_both_stock", "w_both_user", "w_both_user_shadow", "w_hidden", "w_link_dir",
	"w_link_file", "w_stock", "w_stock_dangling", "w_stock_dir", "w_stock_third", "w_third", "w_user", "w_user_dir",
	"w_user_shadow"}

// healthWalkTexts are the `walk` case's records, each once: the file that cannot be opened, the fourth level (met in
// the user tree; the stock tree's is not entered: the user tree has the directory), and a directory of one tree that
// the other tree does not have (the linked one is the user tree's).
var healthWalkTexts = []string{
	"Health configuration cannot read file '<RUN>/etc/health.d/closed.conf'.",
	"CONFIG: Max directory depth reached while reading user path '<RUN>/etc/health.d/d1/d2/d3', stock path '<RUN>/stock/health.d/d1/d2/d3', subpath 'd4'",
	"CONFIG cannot open stock config directory '<RUN>/stock/health.d/link-dir'.",
	"CONFIG cannot open stock config directory '<RUN>/stock/health.d/user-dir'.",
	"CONFIG cannot open user-config directory '<RUN>/etc/health.d/stock-dir'.",
}

// healthConfigRecords are the main thread's records about the health configuration, in file order: what the reader and
// its sub-parsers refused or ignored (health/health_config.c; the options parser's record has no prefix, :114), the
// evaluator's own record of an expression it could not parse (libnetdata/eval, written before the reader's), what
// the validator refused (health_prototypes.c:447-450) and what the directory walk could not open or descend into
// (libnetdata/paths/paths.c:219, :234, :276).
func healthConfigRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		if threadOf(l) == "" && healthConfigRecordRe.MatchString(l) {
			out = append(out, normalizeLog(l, d.Opts.RunDir, ""))
		}
	}
	return out
}

var healthConfigRecordRe = regexp.MustCompile(`msg="(Health |HEALTH|failed to parse expression |Ignoring unknown alarm option |CONFIG cannot open |CONFIG: Max directory depth )`)

// conf is side i's view of sections of /netdata.conf (inicfg_generate(): each key with its annotations, commented
// when its value is the default the agent read it with): the status, the content type and the sections' lines, the
// side's directories replaced.
func (h *healthPair) conf(i int, sections ...string) string {
	r := healthGet(h.p.Each()[i].Daemon, "/netdata.conf")
	var out []string
	for _, s := range parseConfDump(h.n[i].paths(string(r.Body))).sections {
		if !slices.Contains(sections, s.name) {
			continue
		}
		out = append(out, "["+s.name+"]")
		for _, e := range s.entries {
			out = append(out, e.block...)
		}
	}
	return healthView(r, strings.Join(out, "\n"))
}

// healthConfValue is a key's value in a section of a conf view, and whether the section holds the key.
func healthConfValue(view, section, key string) (string, bool) {
	in := false
	for _, l := range strings.Split(view, "\n") {
		if strings.HasPrefix(l, "[") {
			in = l == "["+section+"]"
			continue
		}
		k, v, ok := strings.Cut(strings.TrimPrefix(strings.TrimPrefix(l, "\t"), "# "), " = ")
		if in && ok && k == key {
			return v, true
		}
	}
	return "", false
}

// dirs compares the [directories] and [health] sections of /netdata.conf: the keys in the order each agent first read
// them, each with its value. C reads `health config`, and `stock health config` when the stock rules are on, only
// when it loads the rules (health/health.c:148-158, health_prototypes.c:510-515): an agent that loaded nothing has
// neither key. The oracle must have read them, with their defaults under its configuration directories.
func (h *healthPair) dirs(t *testing.T) {
	t.Helper()
	h.compareNow(t, "the [directories] and [health] sections of /netdata.conf",
		func(i int) string { return h.conf(i, "directories", "health") },
		func(view string) error {
			if !strings.HasPrefix(view, "HTTP 200, ") {
				return fmt.Errorf("answered %q", strings.SplitN(view, "\n", 2)[0])
			}
			for _, k := range [][2]string{{"health config", "config"}, {"stock health config", "stock config"}} {
				parent, _ := healthConfValue(view, "directories", k[1])
				got, ok := healthConfValue(view, "directories", k[0])
				switch {
				case k[0] == "stock health config" && !h.c.stock:
					if ok {
						return fmt.Errorf("[directories] has `%s = %s` with the stock rules off", k[0], got)
					}
				case !ok || parent == "" || got != parent+"/health.d":
					return fmt.Errorf("[directories] `%s` is %q (read: %v), want %q", k[0], got, ok, parent+"/health.d")
				}
			}
			return nil
		})
}

// healthRows counts a table's rows in a running agent's metadata database (dumpLiveDB: the caller polls over an
// error).
func healthRows(t *testing.T, d *daemon.Daemon, table string) (int, error) {
	t.Helper()
	out, err := dumpLiveDB(t, filepath.Join(d.Opts.RunDir, "cache", "netdata-meta.db"), "--table", table)
	return strings.Count(out, "\nrow "+table+" "), err
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

// an alert_hash row's rule name: the alarm column, or the template column (sqlite_health.c:914-923)
var healthRowNameRe = regexp.MustCompile(` (?:alarm|template)="([^"]*)"`)

// healthRowNames are the rule names of alert_hash rows, sorted: one per row.
func healthRowNames(rows []string) []string {
	var out []string
	for _, r := range rows {
		if m := healthRowNameRe.FindStringSubmatch(r); m != nil {
			out = append(out, m[1])
		} else {
			out = append(out, "(a row without a name)")
		}
	}
	slices.Sort(out)
	return out
}

// TestHealthConfig (check `health.config`, M9 commit 0 as split for commit 2, D183, D189): what the health
// configuration alone produces, with no chart on either side (nothing is linked or evaluated). Cases:
//   - `user`: one user file of five kinds of rules, the stock rules off;
//   - `stock`: the installed stock rules, no user file;
//   - `adversarial`: one user file with about a rule per record of the reader and per shape of a row (healthAdvRules);
//   - `walk`: a user and a stock tree of one-rule files (healthWalkFiles): shadowing, subdirectories in one tree or
//     both, the depth limit, the `.conf` suffix, links, a file that cannot be opened.
//
// Compared, in this order: while both run, the [directories] and [health] sections of /netdata.conf (dirs); then,
// once the oracle's rows are stored and both stopped, the main thread's records about the configuration in file
// order (healthConfigRecords), then the alert_hash rows in rowid order, which is the order the rules were read in:
// every column, the hash too (it covers the rule but its source, health_dyncfg.c:359-363); `date_updated` is a clock
// and each side's run directory is `{run}`. The DynCfg nodes of the same rules are `health.dyncfg`'s, the alerts
// linked to a chart `health.api` `linked`'s (which rule is linked to which chart: `health.link`).
func TestHealthConfig(t *testing.T) {
	// the records comparison, with its guard on the oracle: exactly `count` records, each of `want` in one of them
	records := func(count int, want ...string) func(t *testing.T, h *healthPair) {
		return func(t *testing.T, h *healthPair) {
			h.compareLines(t, "the records about the health configuration",
				func(i int) []string { return healthConfigRecords(t, h.p.Each()[i].Daemon) },
				func(oracle []string) error {
					for _, w := range want {
						if !strings.Contains(strings.Join(oracle, "\n"), w) {
							return fmt.Errorf("no record holds %q", w)
						}
					}
					if len(oracle) != count {
						return fmt.Errorf("%d records, want %d", len(oracle), count)
					}
					return nil
				})
		}
	}
	// the rows comparison, with its guard on the oracle's rows
	rows := func(guard func(oracle []string) error) func(t *testing.T, h *healthPair) {
		return func(t *testing.T, h *healthPair) {
			h.compareLines(t, "the alert_hash rows", func(i int) []string { return h.hashRows(t, i) }, guard)
		}
	}
	count := func(want int) func([]string) error {
		return func(oracle []string) error {
			if len(oracle) != want {
				return fmt.Errorf("%d rows, want %d", len(oracle), want)
			}
			return nil
		}
	}
	// a case without a chart: the keys the load read, then the stored rows
	play := func(least int) func(t *testing.T, h *healthPair) {
		return func(t *testing.T, h *healthPair) {
			h.dirs(t)
			h.waitRows(t, "alert_hash", least)
		}
	}
	after := func(checks ...func(t *testing.T, h *healthPair)) func(t *testing.T, h *healthPair) {
		return func(t *testing.T, h *healthPair) {
			for _, c := range checks {
				c(t, h)
			}
		}
	}
	runHealthCases(t, map[string]healthCase{
		"user": {
			conf:  healthCfgConf,
			extra: healthCfgExtra,
			play:  play(6),
			// one row per rule read: hc_alarm, hc_tmpl, hc_chain's two, hc_bad, and hc_off's (a rule that is not
			// enabled is stored too)
			after: after(records(1, "has unknown key 'bogus'"), rows(count(6))),
		},
		"stock": {
			stock: true,
			play:  play(100),
			// the installed rules load without a record
			after: after(records(0), rows(func(oracle []string) error {
				if len(oracle) < 100 {
					return fmt.Errorf("%d rows, want at least 100", len(oracle))
				}
				return nil
			})),
		},
		"adversarial": {
			conf:  healthAdvConf(),
			play:  play(healthAdvRows),
			after: after(records(healthAdvRecords, healthAdvTexts...), rows(count(healthAdvRows))),
		},
		"walk": {
			stock:    true,
			stockDir: true,
			files:    healthWalkFiles,
			play:     play(len(healthWalkNames)),
			after: after(records(len(healthWalkTexts), healthWalkTexts...), rows(func(oracle []string) error {
				if got := healthRowNames(oracle); !slices.Equal(got, healthWalkNames) {
					return fmt.Errorf("the rows are of %v, want %v", got, healthWalkNames)
				}
				return nil
			})),
		},
	})
}
