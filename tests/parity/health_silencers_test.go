// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"cmp"
	"fmt"
	"maps"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/notify"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The management API's route, and what a step's token may be instead of the side's own key (healthManage.token).
const (
	healthManagePath = "/api/v1/manage/health"
	healthNoToken    = "(no token)"
	healthBadToken   = "00000000-0000-0000-0000-000000000000"
)

// The texts the management API answers with (health_silencers.c:12-20).
const (
	healthMsgAuth       = "Auth Error\n"
	healthMsgSilenceAll = "All alarm notifications are silenced\n"
	healthMsgDisableAll = "All health checks are disabled\n"
	healthMsgReset      = "All health checks and notifications are enabled\n"
	healthMsgDisable    = "Health checks disabled for alarms matching the selectors\n"
	healthMsgSilence    = "Alarm notifications silenced for alarms matching the selectors\n"
	healthMsgAdded      = "Alarm selector added\n"
	healthMsgNoType     = "WARNING: Added alarm selector to silence/disable alarms without a SILENCE or DISABLE command.\n"
	healthMsgNoSelector = "WARNING: SILENCE or DISABLE command is ineffective without defining any alarm selectors.\n"
	// the two texts of a path the route refuses (api_v1_manage.c:136-144), the first with C's spelling
	healthMsgNoHealth = "Invalid management request. Curently only 'health' is supported."
	healthMsgLonger   = "Invalid management request. Currently only 'health' is supported."
)

// silencersPath is side i's silencers file (`<varlib>/health.silencers.json`, health_silencers.c:417-423, or the
// file the case names).
func (h *healthPair) silencersPath(i int) string {
	return filepath.Join(h.p.Each()[i].Daemon.Opts.RunDir, cmp.Or(h.c.silencers, healthSilencersFile))
}

// healthSilencersFile is side i's silencers file as the checks compare it: its bytes, quoted, or that it is missing
// (health_silencers.c:417-459).
func (h *healthPair) silencersFile(i int) string {
	b, err := os.ReadFile(h.silencersPath(i))
	if err != nil {
		return "the silencers file: " + h.n[i].paths(err.Error())
	}
	return "the silencers file: " + strconv.Quote(string(b))
}

// healthOldTime is the modification time manage gives a silencers file before a request.
var healthOldTime = time.Unix(1_000_000_000, 0)

// How the two lines about the silencers file begin in a view of a request (healthPair.manage).
const (
	healthFileLine = "\nthe silencers file, "
	healthModeLine = "\nthe silencers file's mode: "
)

// healthManageBody is the answer's body in a view of a request: without the status line and the file's lines.
func healthManageBody(view string) string {
	answer, _, _ := strings.Cut(view, healthFileLine)
	return healthBody(answer)
}

// manage sends one request to side i, without `Accept-Encoding` (healthPlainClient: an answer with an empty body is
// then one a client can read), and returns its answer (as plain: status, content type, body) and, on two last
// lines, what the request did to the side's silencers file: `the silencers file, written: "<bytes>"`, `not written`
// with the bytes it still holds, or with why it cannot be read; then the file's mode (`none` without a file). The
// file gets an old modification time before the request, so a request after which the agent wrote the same bytes
// again shows as written (C opens the file with `wb`, which sets its time: health_silencers.c:290-303), and the agent
// writes before it answers (:403-406). A file the agent makes takes its mode from the agent's umask; one that was
// there keeps its own.
func (h *healthPair) manage(i int, path string, headers ...string) string {
	after := h.watchFile(i)
	r := healthGetBy(healthPlainClient, h.p.Each()[i].Daemon, path, headers...)
	return healthView(r, h.n[i].paths(string(r.Body))) + "\n" + after()
}

// watchFile gives side i's silencers file, when there is one, an old modification time, and returns what renders
// the file once a request was answered: whether it was written since, and its bytes (healthPair.manage).
func (h *healthPair) watchFile(i int) func() string {
	file := h.silencersPath(i)
	_, missing := os.Stat(file)
	if missing == nil {
		if err := os.Chtimes(file, healthOldTime, healthOldTime); err != nil {
			return func() string { return "harness: " + err.Error() }
		}
	}
	return func() string {
		written, mode := "not written", "none"
		if st, err := os.Stat(file); err == nil {
			if mode = fmt.Sprintf("%04o", st.Mode().Perm()); missing != nil || !st.ModTime().Equal(healthOldTime) {
				written = "written"
			}
		}
		return strings.Replace(h.silencersFile(i), "the silencers file: ", healthFileLine[1:]+written+": ", 1) + healthModeLine + mode
	}
}

// healthFlags is a view of /api/v1/alarms?all as each alert's flags: `name silenced=… disabled=…`.
func (h *healthPair) flags(i int) string { return h.flagsOf(h.n[i], i, "") }

// flagsOf is flags for another host's alerts (`/host/<name>`), with that host's normalizer.
func (h *healthPair) flagsOf(n *healthNorm, i int, prefix string) string {
	v := h.getAs(n, i, prefix+"/api/v1/alarms?all")
	out := []string{strings.SplitN(v, "\n", 2)[0]}
	for _, m := range healthAlarmFlagsRe.FindAllStringSubmatch(healthBody(v), -1) {
		out = append(out, fmt.Sprintf("%s disabled=%s silenced=%s", m[1], m[2], m[3]))
	}
	return strings.Join(out, "\n")
}

// healthFlagsWant is a guard on a view of the alerts' flags (healthPair.flags): the answer is 200 and its alerts are
// those of `want`, each `name disabled=… silenced=…`, in any order (the order is the agent's, and is compared).
func healthFlagsWant(want ...string) func(string) error {
	return func(view string) error {
		lines := strings.Split(view, "\n")
		if !strings.HasPrefix(lines[0], "HTTP 200, ") {
			return fmt.Errorf("answered %q", lines[0])
		}
		if got := slices.Sorted(slices.Values(lines[1:])); !slices.Equal(got, slices.Sorted(slices.Values(want))) {
			return fmt.Errorf("the alerts' flags are %q, want %q", got, want)
		}
		return nil
	}
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

// healthSel is a selector as LIST prints it: its members' names and values in turn. C prints alarm, chart, context,
// hosts, in that order, each when it is set, and the values as they are (health_silencers.c:237-244, :262-272): a
// selector with nothing set prints as an empty object.
type healthSel []string

// healthSilencersJSON is the text C makes of the silencers' state, for LIST and for the file
// (health_silencers2json_unsafe, health_silencers.c:253-275): the selectors from the list's head, the last added
// first.
func healthSilencersJSON(all bool, kind string, selectors ...healthSel) string {
	var b strings.Builder
	fmt.Fprintf(&b, "{\n\t\"all\": %t,\n\t\"type\": \"%s\",\n\t\"silencers\": [", all, kind)
	for k, s := range selectors {
		if k > 0 {
			b.WriteString(",")
		}
		b.WriteString("\n\t\t{")
		for m := 0; m+1 < len(s); m += 2 {
			if m > 0 {
				b.WriteString(",")
			}
			fmt.Fprintf(&b, "\n\t\t\t\"%s\": \"%s\"", s[m], s[m+1])
		}
		b.WriteString("\n\t\t}")
	}
	if len(selectors) > 0 {
		b.WriteString("\n\t")
	}
	b.WriteString("]\n}\n")
	return b.String()
}

// healthText is a text/plain answer as the checks print it; healthJSONAnswer a 200 with application/json.
func healthText(status int, body string) string {
	return fmt.Sprintf("HTTP %d, text/plain; charset=utf-8\n%s", status, body)
}

func healthJSONAnswer(body string) string {
	return "HTTP 200, application/json; charset=utf-8\n" + body
}

// healthQuery is a query as a request line carries it, for the text C's handler is to receive once the web server
// decoded it (web_client.c:2085-2130): every byte percent-encoded but letters, digits and `-._~*!&=`. The pieces'
// separators stay as they are: the server cuts the decoded text at them.
func healthQuery(decoded string) string {
	var b strings.Builder
	for _, c := range []byte(decoded) {
		switch {
		case c >= 'a' && c <= 'z', c >= 'A' && c <= 'Z', c >= '0' && c <= '9', strings.IndexByte("-._~*!&=", c) >= 0:
			b.WriteByte(c)
		default:
			fmt.Fprintf(&b, "%%%02X", c)
		}
	}
	return b.String()
}

// healthManage is one request a case sends to both sides, with what the oracle must do with it.
type healthManage struct {
	// label names the step; path is the request: a path, or `?<query>` for a query of the management route
	label, path string
	// token is the X-Auth-Token header's value: empty, each side's own key (healthPair.token); healthNoToken, no
	// header. headers are more header lines.
	token   string
	headers []string
	// want is the answer the oracle must give (healthText, healthJSONAnswer); saves, that the oracle writes its
	// silencers file; file, when set, the bytes the oracle's file must hold afterwards, written or not; mode, when
	// set, the file's mode then
	want  string
	saves bool
	file  string
	mode  string
	// flags, when set, are the alerts' flags once the request took effect (healthFlagsWant): compared after it
	flags []string
}

// healthSaved is a step whose request the oracle answers with a text and saves: `state` is the file then.
func healthSaved(query, body, state string) healthManage {
	return healthManage{label: cmp.Or(query, "an empty query"), path: "?" + healthQuery(query), want: healthText(200, body), saves: true, file: state}
}

// healthListed is `cmd=LIST`: the state's JSON, and what C appends to it; nothing is saved.
func healthListed(state string, after ...string) healthManage {
	return healthManage{label: "cmd=LIST", path: "?cmd=LIST", want: healthJSONAnswer(state + strings.Join(after, ""))}
}

// healthNotSaved is a request with `cmd=LIST` among its pieces: a JSON answer, nothing saved.
func healthNotSaved(query, body string) healthManage {
	return healthManage{label: query, path: "?" + healthQuery(query), want: healthJSONAnswer(body)}
}

// healthDenied is a request the oracle refuses for its token: 403, nothing saved.
func healthDenied(query, token string) healthManage {
	how := "a wrong token"
	if token == healthNoToken {
		how = "no token"
	}
	return healthManage{label: cmp.Or(query, "an empty query") + ", " + how, path: "?" + healthQuery(query), token: token,
		want: healthText(403, healthMsgAuth)}
}

// with returns the step with the flags its request must lead to.
func (s healthManage) with(flags ...string) healthManage {
	s.flags = flags
	return s
}

// guard is the step's guard on the oracle's view (healthPair.manage): the answer (when the step names one), whether
// the file was written, and the file's bytes (when the step names them).
func (s healthManage) guard(view string) error {
	cut := strings.LastIndex(view, healthFileLine)
	if cut < 0 {
		return fmt.Errorf("harness: the view has no line about the silencers file")
	}
	answer := view[:cut]
	file, mode, _ := strings.Cut(view[cut+len(healthFileLine):], healthModeLine)
	written := map[bool]string{true: "written", false: "not written"}[s.saves] + ": "
	switch {
	case s.want != "" && answer != s.want:
		return fmt.Errorf("want the answer\n%s", s.want)
	case !strings.HasPrefix(file, written):
		return fmt.Errorf("want the silencers file %s", strings.TrimSuffix(written, ": "))
	case s.file != "" && file != written+strconv.Quote(s.file):
		return fmt.Errorf("want the silencers file to hold %q", s.file)
	case s.mode != "" && mode != s.mode:
		return fmt.Errorf("want the silencers file's mode %s", s.mode)
	}
	return nil
}

// manageStep sends a step's request once to each side (a command changes the silencers) and compares the two
// answers with what each did to its silencers file (healthPair.manage), after the oracle's passed the step's guard;
// then, for a step with flags, the alerts' flags once the oracle shows the step's. It returns the oracle's view.
func (h *healthPair) manageStep(t *testing.T, s healthManage) string {
	t.Helper()
	path := s.path
	if strings.HasPrefix(path, "?") {
		path = healthManagePath + strings.TrimSuffix(path, "?")
	}
	var got [2]string
	for i := range got {
		headers := slices.Clone(s.headers)
		if s.token != healthNoToken {
			headers = append(headers, "X-Auth-Token: "+cmp.Or(s.token, h.token(i)))
		}
		got[i] = h.manage(i, path, headers...)
	}
	if err := s.guard(got[0]); err != nil {
		t.Fatalf("oracle: %s: %s answered\n%s\n%v", s.label, path, got[0], err)
	}
	if got[0] != got[1] {
		t.Fatalf("%s: %s differs\noracle:\n%s\ncandidate:\n%s", s.label, path, got[0], got[1])
	}
	t.Logf("%s, both sides:\n%s", s.label, got[0])
	if s.flags != nil {
		h.compareNow(t, s.label+": the alerts' flags", h.flags, healthFlagsWant(s.flags...))
	}
	return got[0]
}

// manageSteps plays steps in turn.
func (h *healthPair) manageSteps(t *testing.T, steps ...healthManage) {
	t.Helper()
	for _, s := range steps {
		h.manageStep(t, s)
	}
}

// The agents' records about the silencers and the management key, by how their message starts: the file's read at
// health's start and its write after a request (health_silencers.c:290-303, :425-459; libnetdata/json/json.c:30-34),
// the flags an alert got in a pass and the iteration's record (health_silencers.c:510-519,
// health_event_loop.c:936-943), and the key file's (api_v1_manage.c:26-121, registry_internals.c:83-86).
var healthSilencerRecordRe = regexp.MustCompile(`msg="(Cannot open the file |Health silencers file |Cannot read the data from health silencers file |` +
	`JSON: |Parsed health silencers file |Silencer changes |Alarm silencing changed for host |Skipping health checks, |` +
	`Management API key file |Failed to read management API key |Failed to validate management API key |Registry: GUID |` +
	`Cannot create unique management API key file |Cannot truncate management API key file |` +
	`Cannot write the unique management API key file |You can still continue to use the alarm management API )`)

var healthConnRe = regexp.MustCompile(` conn=\d+`)

// the key an agent could not save, which it tells its log (api_v1_manage.c:121)
var healthSessionKeyRe = regexp.MustCompile(`(using the authorization token )([0-9a-f-]{36})( during this Netdata session only)`)

// healthSilencerRecordsOf renders the records about the silencers and the key of a daemon.log: by thread (the main
// thread's, the web threads', HEALTH's: each thread's in file order; a web worker's write and HEALTH's next pass are
// not ordered against each other), the run directory, the clocks, the thread ids and a web record's connection count
// masked, and the key of a session as `<KEY>`. A web thread's record carries its request (the method, the URL as it
// was received, the client's role and permissions), which is compared. The `errno` field stays: C's records carry the
// errno their thread had, and the checks compare it.
func healthSilencerRecordsOf(lines []string, runDir string) []string {
	var by [4][]string
	for _, l := range lines {
		if !healthSilencerRecordRe.MatchString(l) {
			continue
		}
		l = strings.ReplaceAll(l, runDir, "<RUN>")
		// a record's text is UTF-8 in the Rust agent: a byte of the request that is not (the `request=` field of a
		// web thread's record) is written as U+FFFD there and as it is by C, a standing difference of the log
		l = strings.ToValidUTF8(l, "\uFFFD")
		for _, m := range logMasks {
			l = m.re.ReplaceAllString(l, m.with)
		}
		l = healthSessionKeyRe.ReplaceAllString(l, "${1}<KEY>${3}")
		// a web record's `conn=` counts the web-client cache's allocations: how many connections were open at once
		l = healthConnRe.ReplaceAllString(l, " conn=N")
		k := 3
		switch th := threadOf(l); {
		case th == "":
			k = 0
		case strings.HasPrefix(th, "WEB["):
			k = 1
		case th == "HEALTH":
			k = 2
		}
		by[k] = append(by[k], l)
	}
	return slices.Concat(by[0], by[1], by[2], by[3])
}

// silencerRecords are side i's records about the silencers and the key (healthSilencerRecordsOf).
func (h *healthPair) silencerRecords(t *testing.T, i int) []string {
	t.Helper()
	d := h.p.Each()[i].Daemon
	return healthSilencerRecordsOf(logLines(t, d.Opts.RunDir, "daemon.log"), d.Opts.RunDir)
}

// healthRecordsAre is a guard on a side's records about the silencers and the key: there are as many as `want`
// names, and the k-th holds each part of want[k] (parts separated by ` … `; a part that begins with `!` is one the
// record must not hold: `!errno=`).
func healthRecordsAre(want ...string) func([]string) error {
	return func(records []string) error {
		if len(records) != len(want) {
			return fmt.Errorf("%d records, want %d", len(records), len(want))
		}
		for k, w := range want {
			for _, part := range strings.Split(w, " … ") {
				if not, negative := strings.CutPrefix(part, "!"); negative && strings.Contains(records[k], not) {
					return fmt.Errorf("record %d holds %q", k+1, not)
				} else if !negative && !strings.Contains(records[k], part) {
					return fmt.Errorf("record %d does not hold %q", k+1, part)
				}
			}
		}
		return nil
	}
}

// compareRecords compares both sides' records about the silencers and the key, once both stopped.
func (h *healthPair) compareRecords(t *testing.T, want ...string) {
	t.Helper()
	h.compareLines(t, "the records about the silencers and the key", func(i int) []string { return h.silencerRecords(t, i) },
		healthRecordsAre(want...))
}

// The records as the guards name them, with the errno C's carry: the failed open's on the record of a file that is
// not there, none on the others (each record clears its thread's errno, libnetdata/log/nd_log.c:410, and nothing
// fails between two of these).
const (
	healthRecWritten  = `level=info … !errno= … thread=WEB[n] src_transport=http role=none permissions=0x8 src_ip=localhost … request= … msg="Silencer changes written to <RUN>/lib/health.silencers.json"`
	healthRecNoFile   = `level=info errno="2, No such file or directory" … msg="Cannot open the file <RUN>/lib/health.silencers.json, so Netdata will work with the default health configuration."`
	healthRecParsed   = `level=info … !errno= … msg="Parsed health silencers file <RUN>/lib/health.silencers.json"`
	healthRecSkipping = `level=debug … !errno= … thread=HEALTH msg="Skipping health checks, because all alarms are disabled via API command."`
)

// healthRecChanged is HEALTH's record of an alert whose flags a pass changed (health_silencers.c:510-519).
func healthRecChanged(host, alert string, disabled, silenced [2]bool) string {
	return fmt.Sprintf(`level=info … !errno= … thread=HEALTH msg="Alarm silencing changed for host '%s' alarm '%s': Disabled %t->%t Silenced %t->%t"`,
		host, alert, disabled[0], disabled[1], silenced[0], silenced[1])
}

// healthTimesOf repeats a text n times (the records of n requests that saved).
func healthTimesOf(n int, text string) []string {
	return slices.Repeat([]string{text}, n)
}

// healthTwoCharts is the fake plugin's scenario of a case with two collected charts: `second`, whose one dimension
// counts from 1 (its context is its id), and the Values chart of healthScenario. The second chart's definition is
// written first, in the scenario's own lines, as the engine's Collect step writes it again when it starts in the
// background: both sides create the charts in one order, so their alerts are linked, numbered and listed in one
// order.
func healthTwoCharts(second, dim, chart, context string, dims []string, phases ...map[string]int64) *plugin.Scenario {
	sc := healthScenario(fmt.Sprintf("CHART %s '' 'difftest' 'units' 'family' '' line 1000 1 '' '' ''\nDIMENSION %s '' absolute 1 1\n", second, dim),
		chart, context, dims, phases...)
	steps := sc.Starts[0].Steps
	last := len(steps) - 1
	sc.Starts[0].Steps = slices.Concat(steps[:last],
		[]plugin.Step{{Collect: &plugin.Collect{Chart: second, Dims: []string{dim}, N: 1 << 20, Background: true}}}, steps[last:])
	return sc
}

// healthGoldensConf are the three alerts C's own test of the management API reads the flags of
// (tests/health_mgmtapi/health-cmdapi-test.sh.in: stock alerts of system.cpu and system.load on a real agent), here
// on the fake plugin's charts of those names (D210 F9). None ever leaves CLEAR.
const healthGoldensConf = `# the alerts tests/health_mgmtapi/health-cmdapi-test.sh.in reads, on the fake system.cpu and system.load
 alarm: 10min_cpu_usage
    on: system.cpu
  calc: $user
 every: 1s
  warn: $this > 500
 units: %
  info: the stock alert's name, on the fake chart's user

 alarm: 10min_cpu_iowait
    on: system.cpu
  calc: $iowait
 every: 1s
  warn: $this > 500
 units: %
  info: the stock alert's name, on the fake chart's iowait

 alarm: load_trigger
    on: system.load
  calc: $load1
 every: 1s
  warn: $this > 1000000
 units: load
  info: the stock alert's name, on the fake system.load
`

// the three alerts in the order the script's `check` prints their flags: disabled and silenced of each
var healthScriptAlerts = []string{"10min_cpu_usage", "10min_cpu_iowait", "load_trigger"}

// healthScriptFlags are the alerts' flags the script's `check` wants, from its six words (`True False …`: Python's
// booleans, disabled then silenced of each alert).
func healthScriptFlags(check string) []string {
	words := strings.Fields(strings.ToLower(check))
	var out []string
	for k, name := range healthScriptAlerts {
		out = append(out, fmt.Sprintf("%s disabled=%s silenced=%s", name, words[2*k], words[2*k+1]))
	}
	return out
}

// healthGolden is one of the LIST answers C's own test keeps (tests/health_mgmtapi/expected_list/<name>-list.json).
func healthGolden(t *testing.T, name string) string {
	t.Helper()
	b, err := os.ReadFile(filepath.Join("..", "health_mgmtapi", "expected_list", name+"-list.json"))
	if err != nil {
		t.Fatalf("harness: C's golden file: %v", err)
	}
	return string(b)
}

// healthCollapse is an answer's body as the script's `echo $RESPONSE > file` writes it: its words, one space
// between them, and a newline.
func healthCollapse(body string) string {
	return strings.Join(strings.Fields(body), " ") + "\n"
}

// healthScriptStep is one step of C's script: `cmd` (the query as the script writes it, sent here with its spaces
// percent-encoded) with the token, the answer `cmd` expects, the flags `check` expects, and the golden file
// `check_list` compares LIST with.
type healthScriptStep struct {
	what, query string
	// wrong: with the script's wrong token
	wrong bool
	// answer is the body the oracle must give: the script's expected text and the newline its `$(curl)` drops
	answer string
	// check are the six flags (empty: the script has no `check` here, and the default state is checked: both sides
	// then go through the same states, so HEALTH's records of the changes are the same)
	check string
	// list is the golden file LIST must equal once collapsed; not, when set, a golden file it must differ from (what
	// the script still expects, and C no longer answers)
	list, not string
}

// healthScriptDefault are the flags of the default state.
const healthScriptDefault = "False False False False False False"

// healthScript is C's script, step for step (health-cmdapi-test.sh.in:104-215), with two changes, both about the
// `families` selectors C lost with "Remove family from alerts" (netdata/netdata #16025, 2023-10-06):
//   - "Add silence command" is as the script has it, but what it expects (the SILENCE text alone, load_trigger
//     silenced, the list SILENCE_2 with a `families` selector) is what C answered while the step before it added
//     that selector; the step here names what C answers now (the warning, nothing silenced, the list of SILENCE_3);
//   - the step that commit removed from the script (`families=load`, the list FAMILIES_LOAD) is played before it:
//     the key makes no selector any more, so nothing changes, and its golden file is not what LIST answers.
//
// So 14 of the 16 golden files are C's answers, and the script itself fails on current C at "Add silence command".
var healthScript = []healthScriptStep{
	{"Test default state", "cmd=RESET", false, healthMsgReset, healthScriptDefault, "RESET", ""},
	{"Test auth failure", "cmd=DISABLE ALL", true, healthMsgAuth, healthScriptDefault, "DISABLE_ALL_ERROR", ""},
	{"Test disable", "cmd=DISABLE ALL", false, healthMsgDisableAll, "True False True False True False", "DISABLE_ALL", ""},
	{"Reset", "cmd=RESET", false, healthMsgReset, healthScriptDefault, "RESET", ""},
	{"Test silence", "cmd=SILENCE ALL", false, healthMsgSilenceAll, "False True False True False True", "SILENCE_ALL", ""},
	{"Reset", "cmd=RESET", false, healthMsgReset, healthScriptDefault, "RESET", ""},
	{"Add silencer by name", "cmd=SILENCE&alarm=*10min_cpu_usage *load_trigger", false, healthMsgSilence + healthMsgAdded,
		"False True False False False True", "SILENCE_ALARM_CPU_USAGE_LOAD_TRIGGER", ""},
	{"Convert to disable health checks", "cmd=DISABLE", false, healthMsgDisable, "True False False False True False", "DISABLE", ""},
	{"Convert back to silence notifications", "cmd=SILENCE", false, healthMsgSilence, "False True False False False True", "SILENCE", ""},
	{"Add second silencer by name", "alarm=*10min_cpu_iowait", false, healthMsgAdded, "False True False True False True", "ALARM_CPU_IOWAIT", ""},
	{"Reset", "cmd=RESET", false, healthMsgReset, "", "RESET", ""},
	{"Add silencer by chart", "cmd=DISABLE&chart=system.load", false, healthMsgDisable + healthMsgAdded,
		"False False False False True False", "DISABLE_SYSTEM_LOAD", ""},
	{"Add silencer by context", "context=system.cpu", false, healthMsgAdded, "True False True False True False", "CONTEXT_SYSTEM_CPU", ""},
	{"Reset", "cmd=RESET", false, healthMsgReset, "", "RESET", ""},
	{"Add second condition to a selector (AND)", "cmd=SILENCE&alarm=*10min_cpu_usage *load_trigger&chart=system.load", false,
		healthMsgSilence + healthMsgAdded, "False False False False False True", "SILENCE_ALARM_CPU_USAGE", ""},
	{"Add second selector with two conditions", "alarm=*10min_cpu_usage *load_trigger&context=system.cpu", false, healthMsgAdded,
		"False True False False False True", "ALARM_CPU_USAGE", ""},
	{"Reset", "cmd=RESET", false, healthMsgReset, "", "RESET", ""},
	{"Add silencer without a command (the step the script lost)", "families=load", false, "", healthScriptDefault, "RESET", "FAMILIES_LOAD"},
	{"Add silence command (stale in the script)", "cmd=SILENCE", false, healthMsgSilence + healthMsgNoSelector, healthScriptDefault,
		"SILENCE_3", "SILENCE_2"},
	{"Reset", "cmd=RESET", false, healthMsgReset, "", "RESET", ""},
	{"Add command without silencers", "cmd=SILENCE", false, healthMsgSilence + healthMsgNoSelector, healthScriptDefault, "SILENCE_3", ""},
	{"Add hosts silencer", "hosts=*", false, healthMsgAdded, "False True False True False True", "HOSTS", ""},
	{"Reset", "cmd=RESET", false, healthMsgReset, "", "RESET", ""},
}

// healthScriptChanges counts the records HEALTH writes while the script plays: one for each alert whose flags a step
// changed (every step is followed by a look at the flags, so every state is one a pass saw on both sides).
func healthScriptChanges() int {
	n, last := 0, healthScriptFlags(healthScriptDefault)
	for _, s := range healthScript {
		now := healthScriptFlags(cmp.Or(s.check, healthScriptDefault))
		for k := range now {
			if now[k] != last[k] {
				n++
			}
		}
		last = now
	}
	return n
}

// healthGoldensCase is `goldens`: C's own test of the management API (tests/health_mgmtapi), replayed step for step
// on the fake plugin's system.cpu and system.load with the script's three alert names (D210 F9). Each command's
// answer and what it did to the silencers file are compared, then the alerts' flags, then LIST, whose answer the
// oracle must give as C's golden file has it (the script's `echo`: the words, a space between them).
func healthGoldensCase() healthCase {
	return healthCase{
		conf: healthGoldensConf,
		sc: healthTwoCharts("system.load", "load1", "system.cpu", "system.cpu", []string{"user", "iowait"},
			map[string]int64{"user": 10, "iowait": 10}),
		play: func(t *testing.T, h *healthPair) {
			h.create(t)
			h.compareNow(t, "the three alerts", h.flags, healthFlagsWant(healthScriptFlags(healthScriptDefault)...))
			saved := 0
			for k, s := range healthScript {
				at := fmt.Sprintf("step %d, %s", k+1, s.what)
				step := healthManage{label: at, path: "?" + healthQuery(s.query), want: healthText(200, s.answer), saves: true,
					flags: healthScriptFlags(cmp.Or(s.check, healthScriptDefault))}
				list := healthManage{label: at + ": LIST", path: "?cmd=LIST"}
				if s.wrong {
					step.token, step.want, step.saves = "Wrong token", healthText(403, s.answer), false
					list.token = "Wrong token"
				} else {
					saved++
				}
				h.manageStep(t, step)
				// the answer as the script keeps it
				body := healthCollapse(healthManageBody(h.manageStep(t, list)))
				if want := healthGolden(t, s.list); body != want {
					t.Fatalf("oracle: %s: LIST is %q, C's golden file %s holds %q", at, body, s.list, want)
				}
				if s.not != "" && body == healthGolden(t, s.not) {
					t.Fatalf("oracle: %s: LIST is %q, as the golden file %s: the step is no longer stale", at, body, s.not)
				}
			}
			h.saved = saved
		},
		after: func(t *testing.T, h *healthPair) {
			// each saving request's record, then HEALTH's record of each alert whose flags a step changed
			h.compareLines(t, "the records about the silencers and the key", func(i int) []string { return h.silencerRecords(t, i) },
				func(oracle []string) error {
					counts := healthRecordsWant(map[string]int{`msg="Cannot open the file `: 1, ` msg="Silencer changes written to `: h.saved,
						`msg="Alarm silencing changed for host 'parity-parent' alarm '`: healthScriptChanges()})
					if err := counts(oracle); err != nil {
						return err
					}
					if want := 1 + h.saved + healthScriptChanges(); len(oracle) != want {
						return fmt.Errorf("%d records, want %d", len(oracle), want)
					}
					return nil
				})
		},
	}
}

// TestHealthSilencers (check `health.silencers`; M9 commit 0, D183, the case `api`; M9 commit 7, D210, the others):
// the silencers and the management API (`/api/v1/manage/health`, health_silencers.c, api_v1_manage.c).
//   - `api`: the same fixed key on both agents: a request without a token and one with a wrong token, LIST, SILENCE
//     ALL, RESET, a SILENCE and a DISABLE with selectors, and the two paths the endpoint refuses. Each request is sent
//     once to each side (a command changes the silencers) and its status, content type and body compared, then the
//     silencers file's bytes; after the commands that change what an alert shows, the alerts' `silenced` and
//     `disabled` flags.
//   - `goldens`: C's own test script and its golden LIST answers (healthGoldensCase).
//   - `odd`: the requests C's handler answered in the unit oracle, through the web server, and what only the web
//     server decides: the URL's decoding, the token header, the paths, the methods (healthOddCase).
//   - `selectors`: which alerts a selector takes, by alarm, chart, context and host (healthSelectorsCase).
//   - `disabled`: a disabled alert keeps its status while its value changes; a disabled repeating alert goes on
//     repeating beside a runnable one; with everything disabled nothing runs (healthDisabledCase).
//   - `off`: with health off the route answers, with a key the agent made, and overwrites a file it never read
//     (healthOffCase).
//   - `restart`, `c-after-rust`, `rust-after-c`: the silencers a restart finds in the file (healthSilencersRestart).
//   - `key-*`: the key file's states at the start (healthKeyCases); `file-*`: the silencers file's (healthFileCases).
func TestHealthSilencers(t *testing.T) {
	token := "X-Auth-Token: " + healthKey
	cases := map[string]healthCase{
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
		"goldens":      healthGoldensCase(),
		"odd":          healthOddCase(),
		"selectors":    healthSelectorsCase(),
		"disabled":     healthDisabledCase(),
		"off":          healthOffCase(),
		"restart":      healthSilencersRestart([2][2]Role{{Oracle, Candidate}, {Oracle, Candidate}}),
		"c-after-rust": healthSilencersRestart([2][2]Role{{Oracle, Candidate}, {Oracle, Oracle}}),
		"rust-after-c": healthSilencersRestart([2][2]Role{{Oracle, Oracle}, {Oracle, Candidate}}),
	}
	maps.Copy(cases, healthKeyCases())
	maps.Copy(cases, healthFileCases())
	runHealthCases(t, cases)
}

// healthOddCase is `odd`: no alert, the fixed key. First the request sequences C's own handler answered in the unit
// oracle (health/tests/vectors/manage.tsv: each step below is a row of it, in its order, a RESET between two
// sequences where the table starts a new process; not its `script`, which `goldens` replays, its `unwritable`, which
// is `file-unwritable`, and its two values with control bytes and with a byte that is no UTF-8, which the web server
// does not let through percent-encoded: healthOddRaw has them), sent through the web server: the answers must be
// the table's. Then what the table cannot hold: the web server's decoding of the URL, the paths, and, by raw
// requests, the answers' headers, an answer with no body, the token's header, the methods and a URL with bytes that
// are not encoded.
func healthOddCase() healthCase {
	J, S, L, N, D := healthSilencersJSON, healthSaved, healthListed, healthNotSaved, healthDenied
	none := J(false, "None")
	reset := S("cmd=RESET", healthMsgReset, none)
	sequences := [][]healthManage{
		// no-token
		{D("cmd=SILENCE ALL", healthNoToken), D("cmd=LIST", healthNoToken), D("", healthNoToken), L(none)},
		// bad-token
		{D("cmd=SILENCE ALL", healthBadToken), D("alarm=a", healthBadToken), D("", healthBadToken), L(none)},
		// empty
		{S("", "", none), L(none)},
		// separators
		{S("&&", "", none), S("&=&", "", none), S("=", "", none), S("==x", "", none), S("cmd", "", none), S("cmd=", "", none), L(none)},
		// unknown-cmd
		{S("cmd=FOO", "", none), S("cmd=list", "", none), S("cmd=Silence", "", none), S("cmd=SILENCE ", "", none),
			S("cmd= SILENCE", "", none), S("cmd=SILENCE  ALL", "", none), L(none)},
		// cmd-case
		{S("CMD=SILENCE ALL", "", none), S("Cmd=LIST", "", none), L(none)},
		// unknown-key
		{S("foo=bar", "", none), L(none),
			S("foo=bar&alarm=a", healthMsgAdded+healthMsgNoType, J(false, "None", healthSel{"alarm", "a"})),
			L(J(false, "None", healthSel{"alarm", "a"})),
			S("alarm=b&foo=bar", healthMsgAdded+healthMsgNoType, J(false, "None", healthSel{"alarm", "b"}, healthSel{"alarm", "a"})),
			L(J(false, "None", healthSel{"alarm", "b"}, healthSel{"alarm", "a"}))},
		// list-first
		{N("cmd=LIST&cmd=SILENCE ALL", none+healthMsgSilenceAll), L(J(true, "SILENCE"))},
		// list-last
		{N("cmd=SILENCE ALL&cmd=LIST", healthMsgSilenceAll+J(true, "SILENCE")), L(J(true, "SILENCE"))},
		// list-selector
		{N("alarm=a&cmd=LIST", none+healthMsgAdded+healthMsgNoType),
			N("cmd=LIST&alarm=b", J(false, "None", healthSel{"alarm", "a"})+healthMsgAdded+healthMsgNoType),
			L(J(false, "None", healthSel{"alarm", "b"}, healthSel{"alarm", "a"}))},
		// list-twice
		{S("cmd=SILENCE&alarm=a", healthMsgSilence+healthMsgAdded, J(false, "SILENCE", healthSel{"alarm", "a"})),
			N("cmd=LIST&cmd=LIST", J(false, "SILENCE", healthSel{"alarm", "a"})+J(false, "SILENCE", healthSel{"alarm", "a"}))},
		// list-warning
		{S("cmd=SILENCE", healthMsgSilence+healthMsgNoSelector, J(false, "SILENCE")), L(J(false, "SILENCE"), healthMsgNoSelector),
			N("cmd=DISABLE&cmd=LIST", healthMsgDisable+J(false, "DISABLE")+healthMsgNoSelector)},
		// commands
		{S("cmd=DISABLE ALL&cmd=SILENCE", healthMsgDisableAll+healthMsgSilence, J(true, "SILENCE")), L(J(true, "SILENCE")),
			S("cmd=SILENCE ALL&cmd=RESET", healthMsgSilenceAll+healthMsgReset, none), L(none),
			S("cmd=RESET&cmd=SILENCE ALL", healthMsgReset+healthMsgSilenceAll, J(true, "SILENCE")), L(J(true, "SILENCE")),
			S("cmd=SILENCE&cmd=DISABLE", healthMsgSilence+healthMsgDisable, J(true, "DISABLE")), L(J(true, "DISABLE"))},
		// all-stays
		{S("cmd=SILENCE ALL", healthMsgSilenceAll, J(true, "SILENCE")), S("cmd=DISABLE", healthMsgDisable, J(true, "DISABLE")),
			L(J(true, "DISABLE")), S("cmd=SILENCE", healthMsgSilence, J(true, "SILENCE")), L(J(true, "SILENCE"))},
		// selector-reset
		{S("cmd=SILENCE&alarm=old", healthMsgSilence+healthMsgAdded, J(false, "SILENCE", healthSel{"alarm", "old"})),
			S("alarm=x&cmd=RESET", healthMsgReset+healthMsgAdded+healthMsgNoType, J(false, "None", healthSel{"alarm", "x"})),
			L(J(false, "None", healthSel{"alarm", "x"})),
			S("cmd=RESET&alarm=y", healthMsgReset+healthMsgAdded+healthMsgNoType, J(false, "None", healthSel{"alarm", "y"})),
			L(J(false, "None", healthSel{"alarm", "y"}))},
		// keys
		{S("cmd=SILENCE&hosts=h&context=x&chart=c&alarm=a", healthMsgSilence+healthMsgAdded,
			J(false, "SILENCE", healthSel{"alarm", "a", "chart", "c", "context", "x", "hosts", "h"})),
			L(J(false, "SILENCE", healthSel{"alarm", "a", "chart", "c", "context", "x", "hosts", "h"}))},
		// key-case
		{S("cmd=SILENCE&ALARM=a&Chart=c&CONTEXT=x&hOsTs=h", healthMsgSilence+healthMsgAdded,
			J(false, "SILENCE", healthSel{"alarm", "a", "chart", "c", "context", "x", "hosts", "h"})),
			L(J(false, "SILENCE", healthSel{"alarm", "a", "chart", "c", "context", "x", "hosts", "h"}))},
		// key-repeat
		{S("cmd=SILENCE&alarm=a&alarm=b", healthMsgSilence+healthMsgAdded, J(false, "SILENCE", healthSel{"alarm", "b"})),
			L(J(false, "SILENCE", healthSel{"alarm", "b"}))},
		// key-empty
		{S("cmd=SILENCE&ALARM=", healthMsgSilence+healthMsgNoSelector, J(false, "SILENCE")), L(J(false, "SILENCE"), healthMsgNoSelector),
			S("alarm=&chart=c", healthMsgAdded, J(false, "SILENCE", healthSel{"chart", "c"})), L(J(false, "SILENCE", healthSel{"chart", "c"}))},
		// template
		{S("cmd=SILENCE&template=x", healthMsgSilence+healthMsgAdded, J(false, "SILENCE", healthSel{})), L(J(false, "SILENCE", healthSel{}))},
		// host
		{S("cmd=SILENCE&host=h", healthMsgSilence+healthMsgNoSelector, J(false, "SILENCE")), L(J(false, "SILENCE"), healthMsgNoSelector)},
		// families
		{S("cmd=SILENCE&families=load", healthMsgSilence+healthMsgNoSelector, J(false, "SILENCE")), L(J(false, "SILENCE"), healthMsgNoSelector)},
		// value-equals
		{S("alarm=a=b", healthMsgAdded+healthMsgNoType, J(false, "None", healthSel{"alarm", "a=b"})),
			S("chart==c", healthMsgAdded+healthMsgNoType, J(false, "None", healthSel{"chart", "=c"}, healthSel{"alarm", "a=b"})),
			L(J(false, "None", healthSel{"chart", "=c"}, healthSel{"alarm", "a=b"}))},
		// value-quote
		{S("cmd=SILENCE&alarm=a\"b", healthMsgSilence+healthMsgAdded, J(false, "SILENCE", healthSel{"alarm", "a\"b"})),
			L(J(false, "SILENCE", healthSel{"alarm", "a\"b"}))},
		// value-backslash
		{S("cmd=SILENCE&alarm=a\\b", healthMsgSilence+healthMsgAdded, J(false, "SILENCE", healthSel{"alarm", "a\\b"})),
			L(J(false, "SILENCE", healthSel{"alarm", "a\\b"}))},
		// value-space
		{S("cmd=SILENCE&alarm= ", healthMsgSilence+healthMsgAdded, J(false, "SILENCE", healthSel{"alarm", " "})),
			L(J(false, "SILENCE", healthSel{"alarm", " "}))},
		// value-negative
		{S("cmd=SILENCE&alarm=!", healthMsgSilence+healthMsgAdded, J(false, "SILENCE", healthSel{"alarm", "!"})),
			L(J(false, "SILENCE", healthSel{"alarm", "!"})),
			S("alarm=!a *", healthMsgAdded, J(false, "SILENCE", healthSel{"alarm", "!a *"}, healthSel{"alarm", "!"})),
			L(J(false, "SILENCE", healthSel{"alarm", "!a *"}, healthSel{"alarm", "!"}))},
		// order
		{S("alarm=1", healthMsgAdded+healthMsgNoType, J(false, "None", healthSel{"alarm", "1"})),
			S("alarm=2", healthMsgAdded+healthMsgNoType, J(false, "None", healthSel{"alarm", "2"}, healthSel{"alarm", "1"})),
			S("alarm=3", healthMsgAdded+healthMsgNoType, J(false, "None", healthSel{"alarm", "3"}, healthSel{"alarm", "2"}, healthSel{"alarm", "1"})),
			L(J(false, "None", healthSel{"alarm", "3"}, healthSel{"alarm", "2"}, healthSel{"alarm", "1"}))},
		// warnings
		{S("alarm=a", healthMsgAdded+healthMsgNoType, J(false, "None", healthSel{"alarm", "a"})),
			S("cmd=SILENCE", healthMsgSilence, J(false, "SILENCE", healthSel{"alarm", "a"})),
			S("cmd=RESET", healthMsgReset, none),
			S("cmd=DISABLE", healthMsgDisable+healthMsgNoSelector, J(false, "DISABLE")),
			S("alarm=a", healthMsgAdded, J(false, "DISABLE", healthSel{"alarm", "a"})),
			S("cmd=SILENCE ALL&cmd=RESET&cmd=SILENCE", healthMsgSilenceAll+healthMsgReset+healthMsgSilence+healthMsgNoSelector, J(false, "SILENCE")),
			L(J(false, "SILENCE"), healthMsgNoSelector)},
	}
	// What the web server makes of a URL before the handler sees it (web_client.c:2085-2130, url.c:200-248): the whole
	// URL is decoded, then cut at its first `?`, and the handler cuts the decoded query at `&` and `=`.
	a := func(kv ...string) string { return J(false, "SILENCE", healthSel(kv)) }
	decoding := []healthManage{
		// a `+` is a space
		{label: "a plus for a space", path: "?cmd=SILENCE+ALL", want: healthText(200, healthMsgSilenceAll), saves: true, file: J(true, "SILENCE")},
		reset,
		// an encoded `&` and an encoded `=` are separators as the plain ones are
		{label: "an encoded ampersand", path: "?cmd=SILENCE&alarm=a%26chart=c", want: healthText(200, healthMsgSilence+healthMsgAdded),
			saves: true, file: a("alarm", "a", "chart", "c")},
		reset,
		{label: "an encoded equals sign", path: "?cmd%3DSILENCE&alarm%3Da%3Db", want: healthText(200, healthMsgSilence+healthMsgAdded),
			saves: true, file: a("alarm", "a=b")},
		reset,
		// an encoded `?` in the path begins the query, an encoded `/` is a `/`
		{label: "an encoded question mark", path: healthManagePath + "%3Fcmd=SILENCE&alarm=a", want: healthText(200, healthMsgSilence+healthMsgAdded),
			saves: true, file: a("alarm", "a")},
		reset,
		{label: "an encoded slash", path: "/api/v1/manage%2Fhealth?cmd=SILENCE&alarm=a", want: healthText(200, healthMsgSilence+healthMsgAdded),
			saves: true, file: a("alarm", "a")},
		reset,
	}
	paths := []healthManage{
		{label: "no path under manage", path: "/api/v1/manage?cmd=LIST", want: healthText(404, healthMsgNoHealth)},
		{label: "a slash after manage", path: "/api/v1/manage/?cmd=LIST", want: healthText(404, healthMsgNoHealth)},
		{label: "another name under manage", path: "/api/v1/manage/healt?cmd=LIST", want: healthText(404, healthMsgNoHealth)},
		{label: "a longer name", path: "/api/v1/manage/healthy?cmd=LIST", want: healthText(404, healthMsgLonger)},
		{label: "a slash after health", path: "/api/v1/manage/health/?cmd=LIST", want: healthText(404, healthMsgLonger)},
		{label: "the route under another path of manage", path: "/api/v1/manage/x/manage/health?cmd=LIST", want: healthJSONAnswer(none)},
		{label: "the route by its host", path: "/host/parity-parent/api/v1/manage/health?cmd=LIST", want: healthJSONAnswer(none)},
		{label: "the refused path without a token", path: "/api/v1/manage/other?cmd=LIST", token: healthNoToken, want: healthText(404, healthMsgNoHealth)},
	}
	return healthCase{
		play: func(t *testing.T, h *healthPair) {
			saved := 0
			play := func(steps ...healthManage) {
				t.Helper()
				for _, s := range steps {
					if s.saves {
						saved++
					}
					h.manageStep(t, s)
				}
			}
			for k, steps := range sequences {
				if k > 0 {
					play(reset)
				}
				play(steps...)
			}
			play(reset)
			play(decoding...)
			play(paths...)
			h.saved = saved + healthOddRaw(t, h)
		},
		after: func(t *testing.T, h *healthPair) {
			h.compareRecords(t, slices.Concat([]string{healthRecNoFile}, healthTimesOf(h.saved, healthRecWritten))...)
		},
	}
}

// healthOddRaw are the requests of `odd` that a client library would not send: each is written to a connection of
// its own as it stands, and the answers are compared whole, with their headers (maskRaw: the date, the expiry and the
// transaction's id masked), then the silencers file. `{key}` is the side's key, `{KEY}` the key in upper case. It
// returns how many of the requests the oracle saved after.
func healthOddRaw(t *testing.T, h *healthPair) int {
	t.Helper()
	saved := 0
	const get = "GET " + healthManagePath
	ok, json := "HTTP/1.1 200 OK\r\n", "Content-Type: application/json; charset=utf-8\r\n"
	denied := []string{"HTTP/1.1 403 Forbidden\r\n", "Content-Type: text/plain; charset=utf-8\r\n", "\r\n\r\n" + healthMsgAuth}
	listed := func(state string) []string { return []string{ok, json, "\r\n\r\n" + state} }
	said := func(text string) []string {
		return []string{ok, "Content-Type: text/plain; charset=utf-8\r\n", "\r\n\r\n" + text}
	}
	none := healthSilencersJSON(false, "None")
	for _, r := range []struct {
		label, request string
		// want are parts the oracle's answer must hold, the first at its start and the last at its end; one that begins
		// with `!` is a part it must not hold
		want []string
		// file, when set, are the bytes the oracle's silencers file must hold afterwards
		file string
	}{
		// the answers' headers: every one says it may not be cached
		{"LIST, with its headers", get + "?cmd=LIST HTTP/1.1\r\nX-Auth-Token: {key}\r\n\r\n",
			[]string{ok, json, "Cache-Control: no-cache, no-store, must-revalidate\r\n", "\r\n\r\n" + none}, ""},
		{"a command, with its headers", get + "?cmd=RESET HTTP/1.1\r\nX-Auth-Token: {key}\r\n\r\n",
			[]string{ok, "Content-Type: text/plain; charset=utf-8\r\n", "Cache-Control: no-cache, no-store, must-revalidate\r\n", "\r\n\r\n" + healthMsgReset}, none},
		{"no token, with its headers", get + "?cmd=LIST HTTP/1.1\r\n\r\n", append(slices.Clone(denied[:2]),
			"Cache-Control: no-cache, no-store, must-revalidate\r\n", denied[2]), ""},
		{"a refused path, with its headers", "GET /api/v1/manage/other HTTP/1.1\r\nX-Auth-Token: {key}\r\n\r\n",
			[]string{"HTTP/1.1 404 Not Found\r\n", "Content-Type: text/plain; charset=utf-8\r\n", "\r\n\r\n" + healthMsgNoHealth}, ""},
		// an answer with an empty body: no length, and the connection closes; to a client that accepts gzip, a chunked
		// gzip body is announced and no chunk follows
		{"an empty body, with its headers", get + "?cmd=FOO HTTP/1.1\r\nX-Auth-Token: {key}\r\n\r\n",
			[]string{ok, "Connection: close\r\n", "Content-Type: text/plain; charset=utf-8\r\n", "!Content-Length", "!Transfer-Encoding",
				"X-Transaction-ID: <masked>\r\n\r\n"}, none},
		{"an empty body for a client that accepts gzip", get + "?cmd=FOO HTTP/1.1\r\nX-Auth-Token: {key}\r\nAccept-Encoding: gzip\r\n\r\n",
			[]string{ok, "Content-Type: text/plain; charset=utf-8\r\n", "!Content-Length", "Content-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n",
				"X-Transaction-ID: <masked>\r\n\r\n"}, none},
		// the token's header: its name in any case, its value as sent
		{"the header's name in lower case", get + "?cmd=LIST HTTP/1.1\r\nx-auth-token: {key}\r\n\r\n", listed(none), ""},
		{"no space before the token", get + "?cmd=LIST HTTP/1.1\r\nX-Auth-Token:{key}\r\n\r\n", listed(none), ""},
		{"spaces before the token", get + "?cmd=LIST HTTP/1.1\r\nX-Auth-Token:    {key}\r\n\r\n", listed(none), ""},
		{"a space after the token", get + "?cmd=LIST HTTP/1.1\r\nX-Auth-Token: {key} \r\n\r\n", denied, ""},
		{"an empty token", get + "?cmd=LIST HTTP/1.1\r\nX-Auth-Token: \r\n\r\n", denied, ""},
		{"the key in upper case", get + "?cmd=LIST HTTP/1.1\r\nX-Auth-Token: {KEY}\r\n\r\n", denied, ""},
		{"two tokens, the second the key", get + "?cmd=LIST HTTP/1.1\r\nX-Auth-Token: no\r\nX-Auth-Token: {key}\r\n\r\n", listed(none), ""},
		{"two tokens, the first the key", get + "?cmd=LIST HTTP/1.1\r\nX-Auth-Token: {key}\r\nX-Auth-Token: no\r\n\r\n", denied, ""},
		{"the key as a bearer token", get + "?cmd=LIST HTTP/1.1\r\nAuthorization: Bearer {key}\r\n\r\n", denied, ""},
		// the methods: the handler looks at none
		{"POST", "POST " + healthManagePath + "?cmd=SILENCE%20ALL HTTP/1.1\r\nX-Auth-Token: {key}\r\nContent-Length: 0\r\n\r\n",
			said(healthMsgSilenceAll), healthSilencersJSON(true, "SILENCE")},
		{"PUT", "PUT " + healthManagePath + "?cmd=RESET HTTP/1.1\r\nX-Auth-Token: {key}\r\nContent-Length: 0\r\n\r\n", said(healthMsgReset), none},
		{"DELETE", "DELETE " + healthManagePath + "?cmd=LIST HTTP/1.1\r\nX-Auth-Token: {key}\r\n\r\n", listed(none), ""},
		// what a URL that is not encoded brings: a space, a tab, bytes that are no UTF-8 (the unit table's rows
		// `value-control` and `value-utf8` reach the handler this way only; a newline cannot be sent at all)
		{"a space as it is", get + "?cmd=SILENCE ALL HTTP/1.1\r\nX-Auth-Token: {key}\r\n\r\n", said(healthMsgSilenceAll),
			healthSilencersJSON(true, "SILENCE")},
		{"a byte that is no UTF-8, as it is", get + "?cmd=RESET&alarm=\xc3\xa9\xff HTTP/1.1\r\nX-Auth-Token: {key}\r\n\r\n",
			said(healthMsgReset + healthMsgAdded + healthMsgNoType), healthSilencersJSON(false, "None", healthSel{"alarm", "\xc3\xa9\xff"})},
		{"a tab as it is", get + "?cmd=RESET&alarm=a\tb HTTP/1.1\r\nX-Auth-Token: {key}\r\n\r\n",
			said(healthMsgReset + healthMsgAdded + healthMsgNoType), healthSilencersJSON(false, "None", healthSel{"alarm", "a\tb"})},
		// what an encoded byte the decoder refuses brings: the URL ends there
		{"an encoded tab", get + "?cmd=RESET&alarm=a%09b&chart=c HTTP/1.1\r\nX-Auth-Token: {key}\r\n\r\n",
			said(healthMsgReset + healthMsgAdded + healthMsgNoType), healthSilencersJSON(false, "None", healthSel{"alarm", "a"})},
		{"an encoded NUL", get + "?cmd=RESET&alarm=a%00b&chart=c HTTP/1.1\r\nX-Auth-Token: {key}\r\n\r\n",
			said(healthMsgReset + healthMsgAdded + healthMsgNoType), healthSilencersJSON(false, "None", healthSel{"alarm", "a"})},
		{"an encoded byte that is no UTF-8", get + "?cmd=RESET&alarm=%C3%A9%FF&chart=c HTTP/1.1\r\nX-Auth-Token: {key}\r\n\r\n",
			said(healthMsgReset + healthMsgAdded + healthMsgNoType), healthSilencersJSON(false, "None", healthSel{"alarm", "\xc3\xa9"})},
	} {
		var got [2]string
		for i, s := range h.p.Each() {
			after := h.watchFile(i)
			request := strings.NewReplacer("{key}", h.token(i), "{KEY}", strings.ToUpper(h.token(i))).Replace(r.request)
			b, err := rawExchange(s.Daemon.Addr, []byte(request), 3*time.Second)
			if err != nil {
				t.Fatalf("%s: %s: %v", s.Role, r.label, err)
			}
			got[i] = strconv.Quote(h.n[i].paths(string(maskRaw(b)))) + "\n" + after()
		}
		if r.file != "" {
			saved++
		}
		answer, _ := strconv.Unquote(strings.SplitN(got[0], "\n", 2)[0])
		_, file, _ := strings.Cut(got[0], healthFileLine)
		file, _, _ = strings.Cut(file, healthModeLine)
		for k, part := range r.want {
			if not, negative := strings.CutPrefix(part, "!"); negative {
				if strings.Contains(answer, not) {
					t.Fatalf("oracle: %s: the answer holds %q:\n%s", r.label, not, got[0])
				}
				continue
			}
			if !strings.Contains(answer, part) || (k == 0 && !strings.HasPrefix(answer, part)) || (k == len(r.want)-1 && !strings.HasSuffix(answer, part)) {
				t.Fatalf("oracle: %s: the answer does not hold %q (the first part at its start, the last at its end):\n%s", r.label, part, got[0])
			}
		}
		if want := map[bool]string{true: "written: " + strconv.Quote(r.file), false: "not written: "}[r.file != ""]; !strings.HasPrefix(file, want) ||
			(r.file != "" && file != want) {
			t.Fatalf("oracle: %s: want the silencers file %s:\n%s", r.label, want, got[0])
		}
		if got[0] != got[1] {
			t.Fatalf("%s: the raw answer differs\noracle:\n%s\ncandidate:\n%s", r.label, got[0], got[1])
		}
		t.Logf("%s, both sides:\n%s", r.label, got[0])
	}
	return saved
}

// healthSelectorsConf are the `selectors` case's alerts: two on each of two collected charts. None leaves CLEAR.
const healthSelectorsConf = `# the selectors case's alerts
 alarm: sa_one
    on: hsel.a
  calc: $a
 every: 1s
  warn: $this > 500
 units: things
  info: the first chart's first alert

 alarm: sa_two
    on: hsel.a
  calc: $a
 every: 1s
  warn: $this > 500
 units: things
  info: the first chart's second alert

 alarm: sb_one
    on: hsel.b
  calc: $b
 every: 1s
  warn: $this > 1000000
 units: things
  info: the second chart's first alert

 alarm: sb_two
    on: hsel.b
  calc: $b
 every: 1s
  warn: $this > 1000000
 units: things
  info: the second chart's second alert
`

// healthSelectorsCase is `selectors`: four alerts on two charts of two contexts (hsel.a in `hsel.ctx`; hsel.b, whose
// context is its id) and, after each request, which of them a pass silenced or disabled (health_silencers.c:461-487):
// by alarm, chart, context and host, with wildcards and negative words, two fields in one selector, two selectors,
// a selector that names nothing, and a selector with no command. The type is the silencers', not a selector's: one
// command changes what every selector does.
func healthSelectorsCase() healthCase {
	J, S := healthSilencersJSON, healthSaved
	names := []string{"sa_one", "sa_two", "sb_one", "sb_two"}
	// flags are the four alerts' flags: `silenced` (or `disabled`) for those named, neither for the others
	flags := func(flag string, set ...string) []string {
		var out []string
		for _, n := range names {
			on := slices.Contains(set, n)
			out = append(out, fmt.Sprintf("%s disabled=%t silenced=%t", n, on && flag == "disabled", on && flag == "silenced"))
		}
		return out
	}
	silenced := func(set ...string) []string { return flags("silenced", set...) }
	one := func(kind string, kv ...string) string { return J(false, kind, healthSel(kv)) }
	reset := S("cmd=RESET", healthMsgReset, J(false, "None")).with(silenced()...)
	added := healthMsgSilence + healthMsgAdded
	steps := []healthManage{
		// by alarm: a name, a pattern, a negative word before a pattern, a negative word alone before a name's pattern
		S("cmd=SILENCE&alarm=sa_one", added, one("SILENCE", "alarm", "sa_one")).with(silenced("sa_one")...), reset,
		S("cmd=SILENCE&alarm=*_one", added, one("SILENCE", "alarm", "*_one")).with(silenced("sa_one", "sb_one")...), reset,
		S("cmd=SILENCE&alarm=!sa_one *", added, one("SILENCE", "alarm", "!sa_one *")).with(silenced("sa_two", "sb_one", "sb_two")...), reset,
		S("cmd=SILENCE&alarm=!*_two sa_*", added, one("SILENCE", "alarm", "!*_two sa_*")).with(silenced("sa_one")...), reset,
		// by chart (its id) and by context
		S("cmd=DISABLE&chart=hsel.b", healthMsgDisable+healthMsgAdded, one("DISABLE", "chart", "hsel.b")).with(flags("disabled", "sb_one", "sb_two")...), reset,
		S("cmd=SILENCE&context=hsel.ctx", added, one("SILENCE", "context", "hsel.ctx")).with(silenced("sa_one", "sa_two")...), reset,
		S("cmd=SILENCE&context=hsel.*", added, one("SILENCE", "context", "hsel.*")).with(silenced(names...)...), reset,
		// by host: the hostname, and a name that is not it
		S("cmd=SILENCE&hosts=parity-parent", added, one("SILENCE", "hosts", "parity-parent")).with(silenced(names...)...), reset,
		S("cmd=SILENCE&hosts=parity-other", added, one("SILENCE", "hosts", "parity-other")).with(silenced()...), reset,
		S("cmd=SILENCE&hosts=!parity-parent *", added, one("SILENCE", "hosts", "!parity-parent *")).with(silenced()...), reset,
		// a selector that names nothing takes every alert: `template` makes one and sets nothing, and a text with no
		// word in it is no criterion
		S("cmd=SILENCE&template=sa_one", added, one("SILENCE")).with(silenced(names...)...), reset,
		S("cmd=DISABLE&alarm=!", healthMsgDisable+healthMsgAdded, one("DISABLE", "alarm", "!")).with(flags("disabled", names...)...), reset,
		// a key in upper case is the key; one with no value is not there
		S("cmd=SILENCE&ALARM=sa_two", added, one("SILENCE", "alarm", "sa_two")).with(silenced("sa_two")...), reset,
		S("cmd=SILENCE&ALARM=", healthMsgSilence+healthMsgNoSelector, J(false, "SILENCE")).with(silenced()...), reset,
		// two fields in one selector: both must match; then a second selector: either may
		S("cmd=SILENCE&alarm=*_one&chart=hsel.b", added, one("SILENCE", "alarm", "*_one", "chart", "hsel.b")).with(silenced("sb_one")...),
		S("alarm=sa_two&context=hsel.ctx", healthMsgAdded, J(false, "SILENCE", healthSel{"alarm", "sa_two", "context", "hsel.ctx"},
			healthSel{"alarm", "*_one", "chart", "hsel.b"})).with(silenced("sa_two", "sb_one")...),
		// the command changes what both selectors do
		S("cmd=DISABLE", healthMsgDisable, J(false, "DISABLE", healthSel{"alarm", "sa_two", "context", "hsel.ctx"},
			healthSel{"alarm", "*_one", "chart", "hsel.b"})).with(flags("disabled", "sa_two", "sb_one")...),
		reset,
		// a selector with no command does nothing, until the command comes
		S("alarm=sa_one", healthMsgAdded+healthMsgNoType, one("None", "alarm", "sa_one")).with(silenced()...),
		S("cmd=SILENCE", healthMsgSilence, one("SILENCE", "alarm", "sa_one")).with(silenced("sa_one")...),
		// everything, whatever the selectors say; and a selector's alert stays silenced when `all` could not be taken back
		S("cmd=DISABLE ALL", healthMsgDisableAll, J(true, "DISABLE", healthSel{"alarm", "sa_one"})).with(flags("disabled", names...)...),
		S("cmd=SILENCE", healthMsgSilence, J(true, "SILENCE", healthSel{"alarm", "sa_one"})).with(silenced(names...)...),
		reset,
	}
	return healthCase{
		conf: healthSelectorsConf,
		sc:   healthTwoCharts("hsel.b", "b", "hsel.a", "hsel.ctx", []string{"a"}, map[string]int64{"a": 10}),
		play: func(t *testing.T, h *healthPair) {
			h.create(t)
			h.compareNow(t, "the four alerts", h.flags, healthFlagsWant(silenced()...))
			h.manageSteps(t, steps...)
		},
		after: func(t *testing.T, h *healthPair) {
			// a record for each alert whose flags a step changed
			changes, last := 0, silenced()
			for _, s := range steps {
				for k := range s.flags {
					if s.flags[k] != last[k] {
						changes++
					}
				}
				last = s.flags
			}
			h.compareLines(t, "the records about the silencers and the key", func(i int) []string { return h.silencerRecords(t, i) },
				func(oracle []string) error {
					if want := 1 + len(steps) + changes; len(oracle) != want {
						return fmt.Errorf("%d records, want %d", len(oracle), want)
					}
					return healthRecordsWant(map[string]int{`msg="Cannot open the file `: 1, `msg="Silencer changes written to `: len(steps),
						`msg="Alarm silencing changed for host 'parity-parent' alarm '`:                                                    changes,
						`msg="Alarm silencing changed for host 'parity-parent' alarm 'sb_one': Disabled false->true Silenced true->false"`: -1})(oracle)
				})
		},
	}
}

// healthDisabledConf are the `disabled` case's alerts on hd.values: hd_calc and hd_wit read `a` and go WARNING above
// 50; hd_rep reads `b` and repeats every 4 s while it is raised.
const healthDisabledConf = `# the disabled case's alerts on hd.values
 alarm: hd_calc
    on: hd.values
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: disabled while it is raised

 alarm: hd_wit
    on: hd.values
  calc: $a
 every: 1s
  warn: $this > 50
 units: things
  info: what hd_calc would do, were it not disabled

 alarm: hd_rep
    on: hd.values
  calc: $b
 every: 1s
  warn: $this > 50
repeat: warning 4s critical 4s
 units: things
  info: repeats while it is raised
`

// healthCallsHead is a transcript cut after its n-th call.
func healthCallsHead(transcript string, n int) string {
	var out []string
	for _, l := range strings.Split(transcript, "\n") {
		if m := healthCallLineRe.FindStringSubmatch(l); m != nil {
			if k, _ := strconv.Atoi(m[1]); k > n {
				break
			}
		}
		out = append(out, l)
	}
	return strings.Join(out, "\n")
}

// healthLinesHead is a view of one line per entry (execView) cut after the n-th line that holds `part`.
func healthLinesHead(view, part string, n int) string {
	lines := strings.Split(view, "\n")
	for k, l := range lines {
		if strings.Contains(l, part) {
			if n--; n == 0 {
				return strings.Join(lines[:k+1], "\n")
			}
		}
	}
	return view
}

// healthDisabledCase is `disabled` (debug level): what DISABLE does to alerts that are raised.
//   - hd_calc is disabled by a selector while WARNING, and the value returns to 10: a disabled alert's turn in a pass
//     ends before anything is looked up (health_event_loop.c:478-483), so it stays WARNING with its last value, in
//     `/api/v1/alarms?all` and in the counts, while hd_wit, which reads the same value, clears. No entry, no call.
//     After RESET its next pass clears it, and that is notified.
//   - hd_rep is disabled by a selector while WARNING: the walk that repeats notifications does not look at the flag
//     (:800-873) and runs whenever the host had a runnable alert in the pass (:637), so its repeats go on, each with
//     an entry that is not silenced and a call.
//   - DISABLE ALL: no alert is runnable, the walk does not run, nothing is called; HEALTH writes once that it skips
//     the checks (:936-943), and goes on running its passes.
//
// The count of repeats before each request is each side's timing, so the transcript is compared up to the second
// repeat that began after the oracle showed hd_rep disabled, and after DISABLE ALL the calls each side began in 9 s
// (two repeat intervals and a second) are counted. A repeat's call carries the alert's old value, which a disabled
// alert no longer renews: hd_rep is disabled once both sides ran it raised for two passes or more.
func healthDisabledCase() healthCase {
	J, S := healthSilencersJSON, healthSaved
	set := func(a, b int64) map[string]int64 { return map[string]int64{"a": a, "b": b} }
	fl := func(calc, wit, rep bool) []string {
		return []string{fmt.Sprintf("hd_calc disabled=%t silenced=false", calc), fmt.Sprintf("hd_wit disabled=%t silenced=false", wit),
			fmt.Sprintf("hd_rep disabled=%t silenced=false", rep)}
	}
	sequence := func(lines []string, name string, want ...string) error {
		if got := healthSequence(lines, name); !slices.Equal(got, append([]string{"REMOVED", "UNINITIALIZED", "REMOVED", "UNINITIALIZED"}, want...)) {
			return fmt.Errorf("%s went through %v after its links, want %v", name, got, want)
		}
		return nil
	}
	return healthCase{
		conf: healthDisabledConf, logs: healthLogsDebug,
		sc: healthValues("hd.values", "hd.ctx", []string{"a", "b"}, set(10, 10), set(70, 10), set(10, 10), set(10, 70)),
		play: func(t *testing.T, h *healthPair) {
			transcript := h.transcript(t)
			all := func(i int) string { return h.get(i, "/api/v1/alarms?all") }
			log := func(i int) string { return h.transitions(i, "") }
			statuses := func(what string, calc, wit, rep string) {
				t.Helper()
				h.compareNow(t, what+": /api/v1/alarms?all", all, healthWant(map[string]string{"hd_calc": calc, "hd_wit": wit, "hd_rep": rep}))
			}
			h.create(t)
			for _, name := range []string{"hd_calc", "hd_wit", "hd_rep"} {
				h.processed(t, "phase 0", name, "CLEAR")
			}
			// hd_calc and hd_wit rise; hd_calc is disabled while it is WARNING
			h.release(t, "p1", 1, healthCalcHold)
			h.processed(t, "raised", "hd_calc", "WARNING")
			h.processed(t, "raised", "hd_wit", "WARNING")
			h.compareNow(t, "raised: the notifier's calls", transcript, healthCallsWant(2,
				healthCallFor("hd_calc", "WARNING", 1, nil), healthCallFor("hd_wit", "WARNING", 1, nil), healthCallsEnded(0)))
			h.manageStep(t, S("cmd=DISABLE&alarm=hd_calc", healthMsgDisable+healthMsgAdded,
				J(false, "DISABLE", healthSel{"alarm", "hd_calc"})).with(fl(true, false, false)...))
			// the value returns to 10: hd_wit clears, hd_calc does not move
			h.release(t, "p2", 2, healthCalcHold)
			h.processed(t, "disabled", "hd_wit", "CLEAR")
			statuses("disabled", "WARNING", "CLEAR", "CLEAR")
			h.compareNow(t, "disabled: the alerts' flags", h.flags, healthFlagsWant(fl(true, false, false)...))
			h.compareNow(t, "disabled: the alert log's transitions", log, func(oracle string) error {
				lines := strings.Split(oracle, "\n")
				return cmp.Or(sequence(lines, "hd_calc", "CLEAR", "WARNING"), sequence(lines, "hd_wit", "CLEAR", "WARNING", "CLEAR"))
			})
			h.compareNow(t, "disabled: the notifier's calls", transcript, healthCallsWant(3,
				healthCallFor("hd_wit", "CLEAR", 1, nil), healthCallsEnded(0)))
			// the reset: hd_calc's next pass finds 10
			h.manageStep(t, S("cmd=RESET", healthMsgReset, J(false, "None")).with(fl(false, false, false)...))
			h.processed(t, "after the reset", "hd_calc", "CLEAR")
			statuses("after the reset", "CLEAR", "CLEAR", "CLEAR")
			h.compareNow(t, "after the reset: the alert log's transitions", log, func(oracle string) error {
				return sequence(strings.Split(oracle, "\n"), "hd_calc", "CLEAR", "WARNING", "CLEAR")
			})
			h.compareNow(t, "after the reset: the notifier's calls", transcript, healthCallsWant(4,
				healthCallFor("hd_calc", "CLEAR", 1, map[int]string{10: "WARNING"}), healthCallsEnded(0)))

			// hd_rep rises and is disabled while it is WARNING, beside two alerts that stay runnable
			h.release(t, "p3", 3, healthCalcHold)
			h.processed(t, "repeating", "hd_rep", "WARNING")
			// A disabled alert's turn ends before the pass renews its old value (health_event_loop.c:478-483, :537), so
			// the repeats of a disabled alert carry the old value of the last pass that ran it: 10 when the DISABLE
			// comes right after the pass that raised it, 70 from the next pass on. Both sides get passes to spare first.
			time.Sleep(healthDisabledSettle)
			h.manageStep(t, S("cmd=DISABLE&alarm=hd_rep", healthMsgDisable+healthMsgAdded,
				J(false, "DISABLE", healthSel{"alarm", "hd_rep"})).with(fl(false, false, true)...))
			// the oracle's calls for hd_rep so far: its transition's, and the repeats that began before the pass that
			// showed it disabled; two more repeats must follow
			before := 0
			for _, c := range healthCallsOf(transcript(0)) {
				if c.is("hd_rep", "WARNING") {
					before++
				}
			}
			if before == 0 {
				t.Fatalf("oracle: no call for hd_rep WARNING:\n%s", healthBrief(transcript(0)))
			}
			head := func(i int) string { return healthCallsHead(transcript(i), 4+before+2) }
			h.compareNow(t, "disabled and repeating: the notifier's calls up to the second repeat after the DISABLE", head,
				healthCallsWant(4+before+2, healthCallFor("hd_rep", "WARNING", before+2, nil), healthCallsEnded(0)))
			h.compareNow(t, "disabled and repeating: the alerts' flags", h.flags, healthFlagsWant(fl(false, false, true)...))
			// the repeats' entries are not silenced
			repeat := "hd_rep: CLEAR->WARNING exec_run=T exec_code=0 exec_failed=false processed=true updated=false silenced=false last_repeat=T"
			h.compareNow(t, "disabled and repeating: the alert log's notification state up to that repeat",
				func(i int) string { return healthLinesHead(h.execView(i), " last_repeat=T", before-1+2) }, healthTimes(repeat, before-1+2))

			// everything disabled: no alert is runnable, so nothing repeats
			h.manageStep(t, S("cmd=DISABLE ALL", healthMsgDisableAll, J(true, "DISABLE", healthSel{"alarm", "hd_rep"})).with(fl(true, true, true)...))
			var mark [2]int64
			for i := range mark {
				mark[i] = time.Now().UnixMicro()
			}
			time.Sleep(healthDisabledHold)
			h.compareNow(t, "everything disabled: the calls begun since", func(i int) string {
				calls, err := notify.Calls(h.p.Each()[i].Daemon.Opts.RunDir)
				if err != nil {
					return err.Error()
				}
				n := 0
				for _, c := range calls {
					if c.StartUt > mark[i] {
						n++
					}
				}
				return fmt.Sprintf("%d calls began in the %v after every alert showed disabled", n, healthDisabledHold)
			}, healthIsText(fmt.Sprintf("0 calls began in the %v after every alert showed disabled", healthDisabledHold)))
			h.compareNow(t, "everything disabled: the alerts' flags", h.flags, healthFlagsWant(fl(true, true, true)...))
		},
		after: func(t *testing.T, h *healthPair) {
			host := h.p.Oracle.Hostname
			no, on, off := [2]bool{false, false}, [2]bool{false, true}, [2]bool{true, false}
			h.compareRecords(t, healthRecNoFile,
				healthRecWritten, healthRecWritten, healthRecWritten, healthRecWritten,
				healthRecChanged(host, "hd_calc", on, no), healthRecChanged(host, "hd_calc", off, no),
				healthRecChanged(host, "hd_rep", on, no),
				healthRecSkipping,
				healthRecChanged(host, "hd_calc", on, no), healthRecChanged(host, "hd_wit", on, no))
		},
	}
}

// healthDisabledHold is how long the `disabled` case watches for calls once everything is disabled: two of hd_rep's
// repeat intervals and a second. healthDisabledSettle is how long it lets hd_rep run raised before it disables it:
// two passes or more on each side, and less than the 4 s to its first repeat on most runs (a repeat that began
// before the DISABLE is counted with the transition's call).
const (
	healthDisabledHold   = 9 * time.Second
	healthDisabledSettle = 2500 * time.Millisecond
)

// healthIsText is a guard on a view that is one text.
func healthIsText(want string) func(string) error {
	return func(view string) error {
		if view != want {
			return fmt.Errorf("want %q", want)
		}
		return nil
	}
}

// healthOffFile is the silencers file the `off` case lays out before the start: what C would have written for
// everything disabled and one selector.
var healthOffFile = healthSilencersJSON(true, "DISABLE", healthSel{"alarm", "laid"})

// healthOffCase is `off`: health off, no key file, a silencers file laid out before the start. C makes the key and
// its file whatever health's switch says (api_v1_manage.c:125-127 is called from rrd_init), reads the silencers file
// only when health is on (health.c:172-176), and serves the route all the same: the state starts empty, LIST says so
// beside a file that says otherwise, and the first request that saves overwrites that file.
func healthOffCase() healthCase {
	J, S, L := healthSilencersJSON, healthSaved, healthListed
	return healthCase{
		off: true, key: healthKeyNone,
		files: map[string]string{healthSilencersFile: healthOffFile},
		play: func(t *testing.T, h *healthPair) {
			h.compareKeyFile(t, healthKeyMade)
			list := L(J(false, "None"))
			list.file = healthOffFile
			h.manageSteps(t, list, healthDenied("cmd=LIST", healthNoToken), healthDenied("cmd=LIST", healthKey))
			h.otherKey(t)
			h.manageSteps(t, S("cmd=SILENCE ALL", healthMsgSilenceAll, J(true, "SILENCE")), L(J(true, "SILENCE")),
				S("cmd=DISABLE&alarm=x", healthMsgDisable+healthMsgAdded, J(true, "DISABLE", healthSel{"alarm", "x"})))
			// health is still off, and no alert is listed
			h.compareNow(t, "/api/v1/alarms?all", func(i int) string { return h.get(i, "/api/v1/alarms?all") },
				healthHolds("\"latest_alarm_log_unique_id\": 0,\n\t\"status\": false,\n", "\"alarms\": {\n\n\t}\n}\n"))
		},
		// the file's read left no record: it was not read
		after: func(t *testing.T, h *healthPair) { h.compareRecords(t, healthRecWritten, healthRecWritten) },
	}
}

// otherKey asks each side for LIST with the other side's key: each agent made its own, so the oracle must refuse
// the candidate's. (With one binary on both sides too: the keys are random.)
func (h *healthPair) otherKey(t *testing.T) {
	t.Helper()
	if h.token(0) == h.token(1) {
		t.Fatalf("harness: both sides carry the key %q: each agent was to make its own", h.token(0))
	}
	var got [2]string
	for i := range got {
		got[i] = h.manage(i, healthManagePath+"?cmd=LIST", "X-Auth-Token: "+h.token(1-i))
	}
	if !strings.HasPrefix(got[0], healthText(403, healthMsgAuth)+healthFileLine+"not written: ") {
		t.Fatalf("oracle: LIST with the other side's key answered\n%s", got[0])
	}
	if got[0] != got[1] {
		t.Fatalf("the other side's key: LIST differs\noracle:\n%s\ncandidate:\n%s", got[0], got[1])
	}
	t.Logf("the other side's key, both sides:\n%s", got[0])
}

// healthSilencersRestart is a case whose agents stop with silencers set and start again in the same run directories
// (`restart`: each side again on the file it wrote; `c-after-rust`: the second run is C's on both sides;
// `rust-after-c`: the first run is C's on both sides). hs_calc alone, localhost in `alloc` mode (HEALTH's first pass
// waits for the chart's data in both runs).
//   - The first run: hs_calc CLEAR; `cmd=SILENCE&alarm=hs_calc`, then `chart=hsig.none`: the file names the chart's
//     selector first (a new selector goes to the list's head, health_silencers.c:47-50). Then 70: the alert goes
//     WARNING, silenced, and nothing is called. (The run ends on a transition of the alert, so that the second run
//     loads its last entry and the alert log's ids go on: an alert whose saved row names a link's entry is not
//     loaded, and C then counts the second run's unique ids from a new seed, which each side takes from its clock.)
//   - The second run reads the file at health's start (:425-459): each selector goes to the head again, so LIST now
//     names the alarm's first, while the file still holds the first run's bytes. The chart comes back at 70: the
//     alert's first pass finds it silenced with no request made, its WARNING entry is silenced and nothing is called.
//     A request that saves writes the list as it is now. RESET clears the flags, and calls nothing for an entry that
//     was processed while silenced.
func healthSilencersRestart(bins [2][2]Role) healthCase {
	const chart, context = "hsig.values", "hsig.ctx"
	J, S, L := healthSilencersJSON, healthSaved, healthListed
	sc := healthValues(chart, context, []string{"a"}, map[string]int64{"a": 10}, map[string]int64{"a": 70})
	sc.Starts = append(sc.Starts, plugin.Start{Steps: []plugin.Step{{WaitFile: "again"},
		{Values: &plugin.Values{Chart: chart, Context: context, Dims: []string{"a"}, Phases: []plugin.Phase{{Set: map[string]int64{"a": 70}}}}}}})
	alarm, none, ctx := healthSel{"alarm", "hs_calc"}, healthSel{"chart", "hsig.none"}, healthSel{"context", "hsig.ctx"}
	first := J(false, "SILENCE", none, alarm)
	silenced, clear := "hs_calc disabled=false silenced=true", "hs_calc disabled=false silenced=false"
	return healthCase{
		conf: healthCalcConf, dbMode: "alloc", sc: sc, bins: bins,
		play: func(t *testing.T, h *healthPair) {
			h.create(t)
			h.processed(t, "the first run, CLEAR", "hs_calc", "CLEAR")
			h.manageSteps(t,
				S("cmd=SILENCE&alarm=hs_calc", healthMsgSilence+healthMsgAdded, J(false, "SILENCE", alarm)).with(silenced),
				S("chart=hsig.none", healthMsgAdded, first).with(silenced),
				L(first))
			h.release(t, "p1", 1, healthCalcHold)
			h.processed(t, "the first run, WARNING", "hs_calc", "WARNING")
			h.compareNow(t, "the first run: the notifier's calls", h.transcript(t), healthCallsWant(0))
			h.compareNow(t, "the first run: the alert log's notification state", h.execView, healthBoth(healthLogLines(5),
				healthHas("hs_calc: CLEAR->WARNING exec_run=0 exec_code=0 exec_failed=false processed=true updated=false silenced=true")))
		},
		again: func(t *testing.T, h *healthPair) {
			// before any chart: the selectors in the other order, the file as the first run left it
			list := L(J(false, "SILENCE", alarm, none))
			list.label, list.file = "LIST after the restart", first
			h.manageStep(t, list)
			time.Sleep(2 * time.Second)
			h.release(t, "again", 0, 0)
			h.settle(t, h.n, "")
			h.processed(t, "the second run, WARNING", "hs_calc", "WARNING")
			h.compareNow(t, "the second run: the alerts' flags", h.flags, healthFlagsWant(silenced))
			h.compareNow(t, "the second run: /api/v1/alarms?all", func(i int) string { return h.get(i, "/api/v1/alarms?all") },
				healthAll("WARNING", "hs_calc"))
			h.compareNow(t, "the second run: the notifier's calls", h.transcript(t), healthCallsWant(0))
			// the first run's five entries, the load's (it takes the silenced WARNING's place and its flags), the three
			// links, and the first status, silenced as the first run's was
			h.compareNow(t, "the second run: the alert log's notification state", h.execView, healthBoth(healthLogLines(10), healthHas(
				"hs_calc: CLEAR->WARNING exec_run=0 exec_code=0 exec_failed=false processed=true updated=true silenced=true",
				"hs_calc: WARNING->REMOVED exec_run=0 exec_code=0 exec_failed=false processed=true updated=true silenced=true",
				"hs_calc: UNINITIALIZED->WARNING exec_run=0 exec_code=0 exec_failed=false processed=true updated=false silenced=true")))
			// a request that saves: the file takes the list's order of now
			h.manageSteps(t,
				S("context=hsig.ctx", healthMsgAdded, J(false, "SILENCE", ctx, alarm, none)).with(silenced),
				S("cmd=RESET", healthMsgReset, J(false, "None")).with(clear))
			h.compareNow(t, "after the reset: the notifier's calls", h.transcript(t), healthCallsWant(0))
		},
		after: func(t *testing.T, h *healthPair) {
			host := h.p.Oracle.Hostname
			no, on, off := [2]bool{false, false}, [2]bool{false, true}, [2]bool{true, false}
			h.compareRecords(t, healthRecNoFile, healthRecParsed,
				healthRecWritten, healthRecWritten, healthRecWritten, healthRecWritten,
				healthRecChanged(host, "hs_calc", no, on), healthRecChanged(host, "hs_calc", no, on), healthRecChanged(host, "hs_calc", no, off))
		},
	}
}
