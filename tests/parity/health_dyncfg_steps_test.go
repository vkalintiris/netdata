// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"fmt"
	"maps"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/notify"
)

// The steps of health's DynCfg cases (M9 commit 8, D212; plan evidence/2026-10-05-plan-m9-commit8.md §6.3): requests
// to `/api/v1/config` that change an agent (add, update, enable, disable, remove), each sent once to each side, and
// the views of what a request left: the tree under /health, the saved files, the records.
//
// SAFETY. A payload names its own notifier (`action.execute`, health_dyncfg.c:210), which the rails on netdata.conf
// and on the health.d files do not see. Every body a health case sends goes through healthPayloadFor, and every saved
// DynCfg file a case lays out through healthSavedFile: both refuse an `execute` that is not empty and not a file of
// the side's notifier directory, where only the recording stub lives (daemon.ValidateHealthPayload). A case that
// wants a notifier of its own names `{run}/notify/stub`; one that wants a program that does not exist names another
// file there (`{run}/notify/absent`). healthPair.send is the only function of the health checks that sends a body.

// healthCfgTx is the transaction the n-th DynCfg request of a health case carries (X-Transaction-Id, the call id,
// api_v1_config.c:38-39): fnTx's scheme, so the records' masks keep it (`sent:`).
func healthCfgTx(n int) string { return fnTx(0x9000 + n) }

// healthJob is the id of an alert name's DynCfg job.
func healthJob(name string) string { return healthJobPrefix + ":" + name }

// healthNobody is a healthCfgStep.auth value: the request carries no token.
const healthNobody = "(nobody)"

// healthPayloadFor is a body as side `runDir` gets it: `{run}` replaced by the side's run directory, then held to the
// rail. It returns an error for a body that may not be sent.
func healthPayloadFor(runDir, body string) ([]byte, error) {
	b := []byte(strings.ReplaceAll(body, "{run}", runDir))
	if err := daemon.ValidateHealthPayload(b, notify.Dir(runDir)+"/"); err != nil {
		return nil, fmt.Errorf("harness: refused before anything was sent: %w", err)
	}
	return b, nil
}

// healthSavedFile is a saved DynCfg file as side `runDir` gets it (healthCase.saved): `{run}` replaced, its payload
// held to the rail.
func healthSavedFile(runDir, name, text string) ([]byte, error) {
	b := []byte(strings.ReplaceAll(text, "{run}", runDir))
	if err := daemon.ValidateDynCfgFile(b, notify.Dir(runDir)+"/"); err != nil {
		return nil, fmt.Errorf("harness: the saved DynCfg file %s is not laid out: %w", name, err)
	}
	return b, nil
}

var healthContentLengthRe = regexp.MustCompile(`(?m)^Content-Length: \d+$`)

// answer renders side i's raw answer to a request of `/api/v1/config`: the status line and every header line
// (fnHTTPMask: Date, Expires as its distance from Date, a transaction the case did not send), then the body with the
// side's directories replaced and its wall-clock values ranked (dcClock). The length of a body that holds the side's
// run directory prints `N`: the two directories' names differ in length.
func (h *healthPair) answer(i int, raw []byte) string {
	run := h.p.Each()[i].Daemon.Opts.RunDir
	head, body, _ := strings.Cut(fnHTTPMask(raw), "\r\n\r\n")
	head = strings.ReplaceAll(head, "\r\n", "\n")
	if strings.Contains(body, run) {
		head = healthContentLengthRe.ReplaceAllString(head, "Content-Length: N")
	}
	now := time.Now()
	body = dcClock([]string{h.n[i].paths(body)}, now.Add(-10*time.Minute), now.Add(time.Minute))[0]
	return head + "\n\n" + body
}

// send sends one request to side i's `/api/v1/config?<query>` and returns its answer (healthPair.answer): a POST of
// `body` when it is not empty (application/json; `{run}` in it is the side's run directory), else a GET. It carries
// the transaction healthCfgTx(n) and `auth` (empty: the admin's bearer token; healthNobody: none). A body is held to
// the payload rail first: one that names a notifier outside the side's notifier directory fails the case before
// anything is sent.
func (h *healthPair) send(t *testing.T, i, n int, query, body, auth string) string {
	t.Helper()
	d := h.p.Each()[i].Daemon
	headers := []string{"X-Transaction-Id: " + healthCfgTx(n)}
	switch auth {
	case "":
		headers = append(headers, dcAdmin)
	case healthNobody:
	default:
		headers = append(headers, auth)
	}
	req := rawRequest("GET", "/api/v1/config?"+query, headers, nil)
	if body != "" {
		payload, err := healthPayloadFor(d.Opts.RunDir, body)
		if err != nil {
			t.Fatal(err)
		}
		req = rawRequest("POST", "/api/v1/config?"+query, append(headers, "Content-Type: application/json"), payload)
	}
	raw, err := rawExchange(d.Addr, req, fnWait)
	if err != nil {
		return "no answer: " + err.Error()
	}
	return h.answer(i, raw)
}

// healthCfgStep is one request of a DynCfg case.
type healthCfgStep struct {
	label string
	// query is the request's query (`action=…&id=…`); body its payload, sent with POST when not empty; auth as
	// healthPair.send takes it
	query, body, auth string
	// code is the status the oracle must answer with; parts are texts its answer must hold
	code  int
	parts []string
}

// healthCfgIs is a guard on an answer (healthPair.answer): its status is `code` and it holds each part.
func healthCfgIs(code int, parts ...string) func(string) error {
	return func(view string) error {
		line, _, _ := strings.Cut(view, "\n")
		if !strings.HasPrefix(line, fmt.Sprintf("HTTP/1.1 %d ", code)) {
			return fmt.Errorf("answered %q, want %d", line, code)
		}
		for _, p := range parts {
			if !strings.Contains(view, p) {
				return fmt.Errorf("the answer does not hold %q", p)
			}
		}
		return nil
	}
}

// healthMsg is the body of DynCfg's default answer (libnetdata/inicfg/dyncfg.c:262-273).
func healthMsg(code int, message string) string {
	return fmt.Sprintf(`{"status":%d,"message":"%s"}`, code, message)
}

// cfgStep sends a request once to each side, the oracle first, and compares the two answers: the oracle's must have
// the step's status and texts (the case fails as `oracle: …` otherwise). A request changes the agent, so it is never
// sent twice. It returns the oracle's answer.
func (h *healthPair) cfgStep(t *testing.T, s healthCfgStep) string {
	t.Helper()
	h.tx++
	var views [2]string
	for i := range views {
		views[i] = h.aliased(i, h.send(t, i, h.tx, s.query, s.body, s.auth))
	}
	if err := healthCfgIs(s.code, s.parts...)(views[0]); err != nil {
		t.Fatalf("oracle: %s: /api/v1/config?%s: %v:\n%s", s.label, s.query, err, views[0])
	}
	if views[0] != views[1] {
		t.Fatalf("%s: /api/v1/config?%s differs\noracle:\n%s\ncandidate:\n%s", s.label, s.query, views[0], views[1])
	}
	t.Logf("%s: /api/v1/config?%s, both sides:\n%s", s.label, s.query, views[0])
	return views[0]
}

// cfgSteps plays steps in turn.
func (h *healthPair) cfgSteps(t *testing.T, steps ...healthCfgStep) {
	t.Helper()
	for _, s := range steps {
		h.cfgStep(t, s)
	}
}

// A rule's hash in a job's JSON (health_dyncfg.c:360).
var healthRuleHashRe = regexp.MustCompile(`"hash":"([0-9a-f-]{36})"`)

// aliased renders the rule hashes of a view by the order the side first showed them (`<hash k>`), in a case that
// turned it on (healthPair.hashes not nil): a rule's hash covers its `execute` (health_prototypes.c:396-400), so the
// hash of a rule that names the side's own notifier directory differs side to side. Which rules share a hash, and
// that a hash changed, stay compared.
func (h *healthPair) aliased(i int, view string) string {
	if h.hashes[i] == nil {
		return view
	}
	return healthRuleHashRe.ReplaceAllStringFunc(view, func(m string) string {
		hash := healthRuleHashRe.FindStringSubmatch(m)[1]
		k := slices.Index(h.hashes[i], hash)
		if k < 0 {
			h.hashes[i] = append(h.hashes[i], hash)
			k = len(h.hashes[i]) - 1
		}
		return fmt.Sprintf(`"hash":"<hash %d>"`, k+1)
	})
}

// tree is side i's view of the tree under /health, as the admin (healthPair.config).
func (h *healthPair) tree(i int) string {
	return h.config(i, "action=tree&path=/health", time.Now().Add(-9*time.Minute))
}

// healthNodeOf is a node's object in a view of the tree (`"<id>":{…}`), and whether the tree lists it.
func healthNodeOf(view, id string) (string, bool) {
	key := `"` + id + `":{`
	at := strings.Index(view, key)
	if at < 0 {
		return "", false
	}
	start := at + len(key) - 1
	depth, quoted := 0, false
	for k := start; k < len(view); k++ {
		switch c := view[k]; {
		case quoted && c == '\\':
			k++
		case c == '"':
			quoted = !quoted
		case quoted:
		case c == '{':
			depth++
		case c == '}':
			if depth--; depth == 0 {
				return view[start : k+1], true
			}
		}
	}
	return view[start:], true
}

// healthNode is a guard on a view of the tree: it lists the node, whose object holds each part; a part that begins
// with `!` is one it must not hold.
func healthNode(id string, parts ...string) func(string) error {
	return func(view string) error {
		node, ok := healthNodeOf(view, id)
		if !ok {
			return fmt.Errorf("the tree does not list %s", id)
		}
		for _, p := range parts {
			if not, negative := strings.CutPrefix(p, "!"); negative && strings.Contains(node, not) {
				return fmt.Errorf("%s holds %q: %s", id, not, node)
			} else if !negative && !strings.Contains(node, p) {
				return fmt.Errorf("%s does not hold %q: %s", id, p, node)
			}
		}
		return nil
	}
}

// healthNoNode is a guard on a view of the tree: it answers 200 and lists none of the ids.
func healthNoNode(ids ...string) func(string) error {
	return func(view string) error {
		if !strings.HasPrefix(view, "HTTP 200, ") {
			return fmt.Errorf("answered %q", strings.SplitN(view, "\n", 2)[0])
		}
		for _, id := range ids {
			if _, ok := healthNodeOf(view, id); ok {
				return fmt.Errorf("the tree lists %s", id)
			}
		}
		return nil
	}
}

// The parts of a node's object the guards name (dyncfg-tree.c:20-60).
const (
	healthCmdsTemplate = `"cmds":["schema","add","enable","disable","userconfig"]`
	// a job of a rule file, and a job health registered for a DynCfg rule (health_dyncfg.c:886-907)
	healthCmdsFile   = `"cmds":["get","schema","update","enable","disable","userconfig"]`
	healthCmdsDynCfg = `"cmds":["get","schema","update","remove","enable","disable","userconfig"]`
	// a job a user added in this session: the template's commands and the core's (dyncfg-intercept.c:141-142)
	healthCmdsAdded = `"cmds":["get","schema","update","test","remove","enable","disable","userconfig"]`
	healthOrphan    = `"status":"orphan","cmds":["remove"]`
)

// saves are the lines of side i's DynCfg directory (dcFiles: each file's mode and lines), the side's directories
// replaced and the wall-clock values ranked within the listing.
func (h *healthPair) saves(t *testing.T, i int) []string {
	t.Helper()
	d := h.p.Each()[i].Daemon
	lines := dcFiles(t, "saved", dcConfigDir(d.Opts.RunDir))
	for k, l := range lines {
		lines[k] = h.n[i].paths(l)
	}
	now := time.Now()
	return dcClock(lines, now.Add(-10*time.Minute), now.Add(time.Minute))
}

// healthSavedName is the file a node is saved in (dyncfg-files.c:12-24: the id with its colons escaped).
func healthSavedName(id string) string { return strings.ReplaceAll(id, ":", "%3A") + ".dyncfg" }

// healthSavedAre is a guard on a listing of the DynCfg directory (saves): it holds exactly the files of `want` (by node
// id), and each file holds each of its parts in one of its lines; a part that begins with `!` is one no line of the
// file may hold.
func healthSavedAre(want map[string][]string) func([]string) error {
	return func(lines []string) error {
		got := map[string][]string{}
		for _, l := range lines[min(1, len(lines)):] {
			rest, _ := strings.CutPrefix(l, "saved: ")
			name, line, _ := strings.Cut(rest, " ")
			got[name] = append(got[name], line)
		}
		if len(got) != len(want) {
			return fmt.Errorf("%d files, want %d", len(got), len(want))
		}
		for id, parts := range want {
			file, ok := got[healthSavedName(id)]
			if !ok {
				return fmt.Errorf("no file %s", healthSavedName(id))
			}
			for _, p := range parts {
				text, negative := strings.CutPrefix(p, "!")
				switch held := slices.ContainsFunc(file, func(l string) bool { return strings.Contains(l, text) }); {
				case held && negative:
					return fmt.Errorf("%s holds %q", healthSavedName(id), text)
				case !held && !negative:
					return fmt.Errorf("%s does not hold %q", healthSavedName(id), text)
				}
			}
		}
		return nil
	}
}

// The records of a DynCfg case: the core's (every text starts `DYNCFG`), health's own about a call
// (health_dyncfg.c:639-645, health_prototypes.c:447-450), the evaluator's about an expression it refused
// (eval-parser-legacy.c:864) and the two readers' about a name they do not know (health_prototypes.c:32, :69, :109).
var healthCfgRecordRe = regexp.MustCompile(`msg="(DYNCFG|HEALTH DYNCFG: |HEALTH: alert '|failed to parse expression |` +
	`Alert lookup dimensions grouping |Alert data source )`)

// healthCfgRecordsOf renders the DynCfg records of a daemon.log (healthCfgRecordRe), in file order: the requests of
// a case are sent one after the other, so the order of their records is the requests'. The run directory, the
// clocks, the thread ids and a web record's connection count and client port are masked (normalizeLog, healthConnRe);
// the transaction a case sent prints `sent:`. A web thread's record carries its request, which is compared, and so is
// the `errno` field of every record but the main thread's (normalizeLog), with one exception (healthCfgPeekErrno).
func healthCfgRecordsOf(lines []string, runDir string) []string {
	var out []string
	for _, l := range lines {
		if !healthCfgRecordRe.MatchString(l) {
			continue
		}
		l = strings.ReplaceAll(l, " transaction="+fnTxPrefix, " transaction=sent:"+fnTxPrefix)
		l = normalizeLog(l, runDir, "")
		if strings.HasPrefix(threadOf(l), "WEB[") {
			l = strings.Replace(l, healthCfgPeekErrno, "", 1)
		}
		out = append(out, healthConnRe.ReplaceAllString(l, " conn=N"))
	}
	return out
}

// healthCfgPeekErrno is the errno a web thread's record of C carries by accident of C's inline DynCfg handler: before
// and after a node's callback it asks whether the web client is still there (dyncfg-inline.c dyncfg_inline_callback(),
// is_cancelled; api_v1_config.c:99 gives web_client_interrupt_callback()), which peeks the socket without blocking
// (socket.c is_socket_closed(): recv(MSG_PEEK | MSG_DONTWAIT)) and leaves EAGAIN in the thread's errno for the next
// record: every `DYNCFG USER ACTION` of a job, every `DYNCFG: plugin returned code`, and a `HEALTH DYNCFG: rejected`
// whose payload held no integer for json-c to parse (its integer reader clears errno). The Rust agent has no such
// check (D212 F3) and prints none. The field is dropped from web threads' records on both sides, for this value alone
// (D213): any other errno is compared.
const healthCfgPeekErrno = ` errno="11, Resource temporarily unavailable"`

// cfgRecords are side i's DynCfg records (healthCfgRecordsOf), with the side's short runtime directory replaced.
func (h *healthPair) cfgRecords(t *testing.T, i int) []string {
	t.Helper()
	d := h.p.Each()[i].Daemon
	out := healthCfgRecordsOf(logLines(t, d.Opts.RunDir, "daemon.log"), d.Opts.RunDir)
	for k, l := range out {
		out[k] = h.aliased(i, h.n[i].paths(l))
	}
	return out
}

// healthRecordsCount is a guard on a side's records: each text is held by exactly its count of records, and there are
// `total` records in all (-1: any number). A text may be parts separated by ` … `: a record holds it when it holds
// each part, in that order (`level=error … msg="…`).
func healthRecordsCount(total int, want map[string]int) func([]string) error {
	holds := func(record, text string) bool {
		for _, part := range strings.Split(text, " … ") {
			at := strings.Index(record, part)
			if at < 0 {
				return false
			}
			record = record[at+len(part):]
		}
		return true
	}
	return func(records []string) error {
		if total >= 0 && len(records) != total {
			return fmt.Errorf("%d records, want %d", len(records), total)
		}
		for _, text := range slices.Sorted(maps.Keys(want)) {
			n := 0
			for _, r := range records {
				if holds(r, text) {
					n++
				}
			}
			if n != want[text] {
				return fmt.Errorf("%d records hold %q, want %d", n, text, want[text])
			}
		}
		return nil
	}
}

// compareCfgRecords compares both sides' DynCfg records, once both stopped.
func (h *healthPair) compareCfgRecords(t *testing.T, guard func([]string) error) {
	t.Helper()
	h.compareLines(t, "the DynCfg records", func(i int) []string { return h.cfgRecords(t, i) }, guard)
}

// healthRuleDoc is one rule of a payload with every member health's parser wants of an `add` or an `update`
// (health_dyncfg.c:84-282): an alert on the chart `on` (a template on the context, when `set` says so) whose value is
// `calc` and whose warning is `warn`, run every second. `set` changes it, in pairs of a dotted path under the rule
// (`config.value.update_every`) and a value; a nil value removes the member. No `action`: the rule's notifier is the
// host's (health_prototypes.c:479-483), and its hash is the same on both sides.
func healthRuleDoc(on, calc, warn string, set ...any) map[string]any {
	rule := map[string]any{
		"enabled": true,
		"type":    "instance",
		"config": map[string]any{
			"match": map[string]any{"on": on, "host_labels": "*", "instance_labels": "*"},
			"info":  "a rule sent by the harness",
			"value": map[string]any{
				"database_lookup": map[string]any{"after": 0, "before": 0, "time_group": "average", "dims_group": "sum",
					"data_source": "samples", "options": []any{}, "dimensions": ""},
				"calculation":  calc,
				"units":        "things",
				"update_every": 1,
			},
			"conditions": map[string]any{"warning_condition": warn, "critical_condition": ""},
		},
	}
	for k := 0; k+1 < len(set); k += 2 {
		at := rule
		path := strings.Split(set[k].(string), ".")
		for _, name := range path[:len(path)-1] {
			next, ok := at[name].(map[string]any)
			if !ok {
				next = map[string]any{}
				at[name] = next
			}
			at = next
		}
		if last := path[len(path)-1]; set[k+1] == nil {
			delete(at, last)
		} else {
			at[last] = set[k+1]
		}
	}
	return rule
}

// healthPayload is a payload of the rules: `format_version` 1 (health_dyncfg.c:236-243). The members come in name
// order, which the parser does not read (every member is looked up by its name).
func healthPayload(rules ...map[string]any) string {
	return healthJSON(map[string]any{"format_version": 1, "rules": rules})
}

// healthJSON is a value's JSON as the cases send it: compact, `<` and `>` as they are.
func healthJSON(v any) string {
	var b bytes.Buffer
	enc := json.NewEncoder(&b)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		panic(err)
	}
	return strings.TrimSuffix(b.String(), "\n")
}

// healthSavedText is a saved file's text as C writes one for a health node (dyncfg-files.c:33-65; `sync=true`: the
// inline nodes are synchronous, dyncfg-inline.c:40-52): a job of health's template, or the template itself (an empty
// name). The host is the agents' own (dcHost). payload empty for none; `{run}` in a payload would change its length
// after `content_length` was written, so the cases' saved payloads name no directory.
func healthSavedText(name, sourceType, source string, userDisabled bool, saves int, cmds, payload string) string {
	var b strings.Builder
	id, kind := healthJobPrefix, "template"
	if name != "" {
		id, kind = healthJob(name), "job"
	}
	fmt.Fprintf(&b, "version=1\nid=%s\n", id)
	if name != "" {
		fmt.Fprintf(&b, "template=%s\n", healthJobPrefix)
	}
	fmt.Fprintf(&b, "host=%s\npath=/health/alerts/prototypes\ntype=%s\nsource_type=%s\nsource=%s\ncreated=%d\nmodified=%d\n"+
		"sync=true\nuser_disabled=%t\nsaves=%d\ncmds=%s\n", dcHost(), kind, sourceType, source, int64(1600000000000001),
		int64(1600000000000002), userDisabled, saves, cmds)
	if payload != "" {
		fmt.Fprintf(&b, "content_type=application/json\ncontent_length=%d\n---\n%s", len(payload), payload)
	}
	return b.String()
}

// The `cmds` line of a saved job of health's template (one a user added in this session; a rule file's that was
// disabled; a rule file's that was updated) and of the saved template (dyncfg-files.c:55-57).
const (
	healthSavedCmds         = "get schema update test remove enable disable userconfig "
	healthSavedFileCmds     = "get schema update enable disable userconfig "
	healthSavedDynCfgCmds   = "get schema update remove enable disable userconfig "
	healthSavedTemplateCmds = "schema add enable disable userconfig "
	// the source the core saves for a job the admin's token added (user-auth.c's text of a bearer client)
	healthSavedSource = "method=api-bearer,role=admin,permissions=0x7ff,user=fnhttp-admin,account=b6b6b6b666664666866600000000acc1,ip=localhost"
)

// A first status in a view of `/api/v1/alarm_log`, in a notifier's transcript and in health.log: the entry's duration
// before its statuses, the call's old status before its duration, the record's duration before its statuses.
var (
	healthFirstStatusRe    = regexp.MustCompile(`"duration":(?:T|0),(\s*"non_clear_duration":(?:T|0),\s*"status":"[A-Z]+",\s*"old_status":"UNINITIALIZED",)`)
	healthFirstStatusLogRe = regexp.MustCompile(` alert_duration=(?:T|0)( alert_value=\S+ alert_value_old=\S+ alert_status=[A-Z]+ alert_value_old=UNINITIALIZED )`)
	healthCallOldRe        = regexp.MustCompile(`(?m)^call (\d+): argv\[10\]=UNINITIALIZED$`)
	// an entry another entry replaced, of a link (to UNINITIALIZED) or of an unlink of an alert that had a status:
	// its `processed`, then `updated`, then its other members up to its statuses (no brace but a directory's)
	healthReplacedRe = regexp.MustCompile(`"processed":(?:true|false),(\s*"updated":true,(?:[^{}]|\{run\}|\{rt\})*?` +
		`"status":(?:"UNINITIALIZED",|"REMOVED",\s*"old_status":"(?:CLEAR|WARNING|CRITICAL|UNDEFINED)",))`)
)

// healthSinceLink renders, in a view of `/api/v1/alarm_log`, the duration of every entry that takes an alert from
// UNINITIALIZED to its first status as `D`, set or not: it is the seconds between the alert's link and the first
// pass that ran it. HEALTH links and runs an alert in one pass (0 s). An alert a DynCfg command linked on a web
// worker's thread is run once the metadata thread stored the link's entry, which is that thread's own five-second
// round (sqlite_metadata.c:2795-2798; health_event_loop.c:396-401): the first status came 1 to 7 s after the link C
// against C, 0.9 s once, so the same second on one side and the next on the other is a matter of milliseconds.
func healthSinceLink(view string) string {
	return healthFirstStatusRe.ReplaceAllString(view, `"duration":D,$1`)
}

// healthReplaced renders, in a view of `/api/v1/alarm_log`, the `processed` flag of every entry of a link or of an
// unlink that another entry replaced as `P`. The flag says HEALTH's notification scan saw the entry before it was
// replaced (health_notifications.c:562-566). For the entries of one of HEALTH's own passes that is the pass's
// order. An entry a DynCfg command made on a web worker's thread races the scan: an `update` unlinks the alert,
// asks the alert log's table for its id (the rule's hash changed: rrdcalc.c:130-169) and links it again, and a scan
// between the two marks the unlink's entry (seen C against C: `actions`, the update's entry `processed` on one side
// only). The entry that stands for each alert now (`updated` false) keeps its flag compared.
func healthReplaced(view string) string {
	return healthReplacedRe.ReplaceAllString(view, `"processed":P,$1`)
}

// healthCallsSinceLink is healthSinceLink for a notifier's transcript: argument 14 (the entry's duration) of every
// call whose old status (argument 10) is UNINITIALIZED.
func healthCallsSinceLink(transcript string) string {
	for _, m := range healthCallOldRe.FindAllStringSubmatch(transcript, -1) {
		re := regexp.MustCompile(`(?m)^(call ` + m[1] + `: argv\[14\]=).*$`)
		transcript = re.ReplaceAllString(transcript, "${1}D")
	}
	return transcript
}

// healthLogSinceLink is healthSinceLink for a health.log record (healthNorm.healthLog): `alert_duration` of a
// record whose old status is UNINITIALIZED (C names that field `alert_value_old` a second time).
func healthLogSinceLink(record string) string {
	return healthFirstStatusLogRe.ReplaceAllString(record, " alert_duration=D$1")
}

// healthLastChange is a guard on a view of the transitions (healthPair.transitions): the last change of the alert's
// status is `change` (`OLD->NEW`).
func healthLastChange(name, change string) func(string) error {
	return func(view string) error {
		last := ""
		for _, l := range strings.Split(view, "\n") {
			if rest, ok := strings.CutPrefix(l, name+": "); ok {
				last, _, _ = strings.Cut(rest, " ")
			}
		}
		if last != change {
			return fmt.Errorf("the last change of %s is %q, want %q", name, last, change)
		}
		return nil
	}
}

// The steps' and the views' units: what the payload rail refuses before a byte is sent, what a saved file may hold,
// the payloads the cases build, a node's object in a tree, the guards, the records' masks.
func TestHealthDynCfgNorm(t *testing.T) {
	const run = "/ndt/parity-oracle-1"
	// the rail of the sending helper: `{run}` is the side's directory, and only its notifier directory is allowed
	for name, c := range map[string]struct {
		body string
		ok   bool
	}{
		"a rule without an action":      {healthPayload(healthRuleDoc("c.a", "$a", "$this > 1")), true},
		"the side's stub":               {healthPayload(healthRuleDoc("c.a", "$a", "", "config.action.execute", "{run}/notify/stub")), true},
		"a program that does not exist": {healthPayload(healthRuleDoc("c.a", "$a", "", "config.action.execute", "{run}/notify/absent")), true},
		"an empty execute":              {healthPayload(healthRuleDoc("c.a", "$a", "", "config.action.execute", "")), true},
		"the other side's stub":         {healthPayload(healthRuleDoc("c.a", "$a", "", "config.action.execute", "/ndt/parity-candidate-2/notify/stub")), false},
		"the installed notifier":        {healthPayload(healthRuleDoc("c.a", "$a", "", "config.action.execute", "/usr/libexec/netdata/plugins.d/alarm-notify.sh")), false},
		"another program":               {healthPayload(healthRuleDoc("c.a", "$a", "", "config.action.execute", "/bin/true")), false},
		"the run directory itself":      {healthPayload(healthRuleDoc("c.a", "$a", "", "config.action.execute", "{run}/stub")), false},
		"the second rule's":             {healthPayload(healthRuleDoc("c.a", "$a", ""), healthRuleDoc("c.a", "$a", "", "config.action.execute", "/bin/true")), false},
		"no document":                   {"not json", true},
		"no document with the member":   {`{"execute":"{run}/notify/stub"`, false},
	} {
		b, err := healthPayloadFor(run, c.body)
		if (err == nil) != c.ok {
			t.Errorf("the rail, %s: %v, want sent: %v", name, err, c.ok)
		}
		if err == nil && strings.Contains(string(b), "{run}") {
			t.Errorf("the rail, %s: `{run}` is still in the body", name)
		}
		if err != nil && b != nil {
			t.Errorf("the rail, %s: a refused body is returned", name)
		}
	}
	// a saved file: the same rail on its payload
	stub := healthPayload(healthRuleDoc("c.a", "$a", "", "config.action.execute", "{run}/notify/stub"))
	other := healthPayload(healthRuleDoc("c.a", "$a", "", "config.action.execute", "/bin/true"))
	if _, err := healthSavedFile(run, "a.dyncfg", healthSavedText("a", "dyncfg", healthSavedSource, false, 1, healthSavedCmds, stub)); err != nil {
		t.Errorf("a saved file that names the stub: %v", err)
	}
	if _, err := healthSavedFile(run, "a.dyncfg", healthSavedText("a", "dyncfg", healthSavedSource, false, 1, healthSavedCmds, other)); err == nil {
		t.Errorf("a saved file that names another program is laid out")
	}
	if _, err := healthSavedFile(run, "a.dyncfg", healthSavedText("a", "user", "x", true, 1, healthSavedFileCmds, "")); err != nil {
		t.Errorf("a saved file without a payload: %v", err)
	}
	if got, want := healthSavedText("", "internal", "", true, 1, healthSavedTemplateCmds, ""),
		"version=1\nid=health:alert:prototype\nhost="+dcHost()+"\npath=/health/alerts/prototypes\ntype=template\n"+
			"source_type=internal\nsource=\ncreated=1600000000000001\nmodified=1600000000000002\nsync=true\n"+
			"user_disabled=true\nsaves=1\ncmds=schema add enable disable userconfig \n"; got != want {
		t.Errorf("the template's saved file:\n%q, want\n%q", got, want)
	}
	if got := healthSavedName(healthJob("a:b")); got != "health%3Aalert%3Aprototype%3Aa%3Ab.dyncfg" {
		t.Errorf("a saved file's name: %q", got)
	}

	// a payload: every member the parser wants, `>` as it is, a member changed and one removed
	doc := healthPayload(healthRuleDoc("c.a", "$a", "$this > 1", "config.value.update_every", 5, "enabled", nil, "config.action.recipient", "x"))
	for _, part := range []string{`"format_version":1`, `"warning_condition":"$this > 1"`, `"update_every":5`, `"recipient":"x"`,
		`"database_lookup":{"after":0,"before":0,"data_source":"samples","dimensions":"","dims_group":"sum","options":[],"time_group":"average"}`,
		`"match":{"host_labels":"*","instance_labels":"*","on":"c.a"}`} {
		if !strings.Contains(doc, part) {
			t.Errorf("the payload does not hold %s:\n%s", part, doc)
		}
	}
	if strings.Contains(doc, `"enabled"`) || strings.Contains(doc, "\n") {
		t.Errorf("the payload holds a removed member or a newline:\n%s", doc)
	}

	// a node's object in a tree: braces inside a text do not end it
	tree := `HTTP 200, x` + "\n" + `{"version":1,"tree":{"/health/alerts/prototypes":{"health:alert:prototype":{"type":"template","status":"accepted"},` +
		`"health:alert:prototype:a":{"type":"job","status":"running","source":"file={run}/x \"}\" y","payload":{"available":false},"saves":0},` +
		`"health:alert:prototype:ab":{"type":"job","status":"disabled"}}}}`
	if node, ok := healthNodeOf(tree, healthJob("a")); !ok || node != `{"type":"job","status":"running","source":"file={run}/x \"}\" y","payload":{"available":false},"saves":0}` {
		t.Errorf("a node's object: %q, %t", node, ok)
	}
	if node, ok := healthNodeOf(tree, healthJobPrefix); !ok || node != `{"type":"template","status":"accepted"}` {
		t.Errorf("the template's object: %q, %t", node, ok)
	}
	if _, ok := healthNodeOf(tree, healthJob("b")); ok {
		t.Errorf("a node the tree does not list was found")
	}
	for name, c := range map[string]struct {
		guard func(string) error
		ok    bool
	}{
		"a part held":                     {healthNode(healthJob("a"), `"status":"running"`, `"saves":0`), true},
		"a part of another node":          {healthNode(healthJob("a"), `"status":"disabled"`), false},
		"a part that must not be":         {healthNode(healthJob("a"), `!"status":"disabled"`), true},
		"a part that is, and must not be": {healthNode(healthJob("ab"), `!"status":"disabled"`), false},
		"a node that is not listed":       {healthNode(healthJob("b")), false},
		"none of the ids":                 {healthNoNode(healthJob("b"), healthJob("c")), true},
		"one of the ids is listed":        {healthNoNode(healthJob("b"), healthJob("ab")), false},
	} {
		if err := c.guard(tree); (err == nil) != c.ok {
			t.Errorf("a tree's guard, %s: %v, want %t", name, err, c.ok)
		}
	}
	if healthNoNode(healthJob("b"))("HTTP 404, x\n{}") == nil {
		t.Errorf("an answer that is no tree passes the guard that a node is gone")
	}

	// an answer's guard
	answer := "HTTP/1.1 202 Accepted\nContent-Type: application/json\n\n" + healthMsg(202, "accepted")
	for name, c := range map[string]struct {
		guard func(string) error
		ok    bool
	}{
		"the status and the text": {healthCfgIs(202, healthMsg(202, "accepted")), true},
		"another status":          {healthCfgIs(200, healthMsg(202, "accepted")), false},
		"another text":            {healthCfgIs(202, healthMsg(202, "updated")), false},
		"a status that begins so": {healthCfgIs(20), false},
	} {
		if err := c.guard(answer); (err == nil) != c.ok {
			t.Errorf("an answer's guard, %s: %v, want %t", name, err, c.ok)
		}
	}

	// an answer's rendering: the head's lines, the length of a body that holds the side's directory masked, another kept
	hp := &healthPair{p: &Pair{Oracle: &daemon.Daemon{Opts: daemon.Options{RunDir: run}}, Candidate: &daemon.Daemon{Opts: daemon.Options{RunDir: "/ndt/parity-candidate-22"}}}}
	hp.n[0], hp.n[1] = &healthNorm{run: run}, &healthNorm{run: "/ndt/parity-candidate-22"}
	raw := func(body string) []byte {
		return []byte("HTTP/1.1 200 OK\r\nDate: Mon, 05 Oct 2026 01:00:00 GMT\r\nExpires: Mon, 05 Oct 2026 01:00:00 GMT\r\nContent-Length: " +
			fmt.Sprint(len(body)) + "\r\nX-Transaction-ID: " + healthCfgTx(1) + "\r\n\r\n" + body)
	}
	if got, want := hp.answer(0, raw(`{"source":"file=`+run+`/x"}`)), "HTTP/1.1 200 OK\nDate: D\nExpires: Date+0\nContent-Length: N\nX-Transaction-ID: "+
		healthCfgTx(1)+"\n\n"+`{"source":"file={run}/x"}`; got != want {
		t.Errorf("an answer with the side's directory:\n%q, want\n%q", got, want)
	}
	if got := hp.answer(1, raw(`{"source":"file=/ndt/parity-candidate-22/x"}`)); got != hp.answer(0, raw(`{"source":"file=`+run+`/x"}`)) {
		t.Errorf("the two sides' answers with their directories differ:\n%q", got)
	}
	if got := hp.answer(0, raw(`{"status":200}`)); !strings.Contains(got, "\nContent-Length: 14\n") {
		t.Errorf("an answer without a directory lost its length:\n%q", got)
	}

	// the saved files' guard: the files by node, a part per line, a part no line may hold
	listing := []string{"saved: dir mode 0755", "saved: health%3Aalert%3Aprototype%3Aa.dyncfg mode 0644",
		`saved: health%3Aalert%3Aprototype%3Aa.dyncfg "user_disabled=true\n"`, `saved: health%3Aalert%3Aprototype%3Aa.dyncfg "saves=2\n"`}
	for name, c := range map[string]struct {
		want map[string][]string
		ok   bool
	}{
		"the file and its parts":   {map[string][]string{healthJob("a"): {`"user_disabled=true\n"`, `"saves=2\n"`, "!---"}}, true},
		"a part it does not hold":  {map[string][]string{healthJob("a"): {`"saves=3\n"`}}, false},
		"a part it must not hold":  {map[string][]string{healthJob("a"): {"!user_disabled=true"}}, false},
		"a file that is not there": {map[string][]string{healthJob("a"): nil, healthJob("b"): nil}, false},
		"a file too many":          {map[string][]string{}, false},
	} {
		if err := healthSavedAre(c.want)(listing); (err == nil) != c.ok {
			t.Errorf("the saved files' guard, %s: %v, want %t", name, err, c.ok)
		}
	}
	if err := healthSavedAre(map[string][]string{})([]string{"saved: dir mode 0755"}); err != nil {
		t.Errorf("an empty directory: %v", err)
	}

	// the records: by their text, the masks, a sent transaction kept
	lines := []string{
		`time=2026-10-05T01:00:00.123+00:00 comm=netdata source=daemon level=error errno="2, No such file or directory" tid=77 msg="HEALTH DYNCFG: rejected 'add' of alert prototype 'x'"`,
		`time=2026-10-05T01:00:00.124+00:00 comm=netdata source=daemon level=error tid=91 thread=WEB[2] src_transport=http src_ip=localhost src_port=40112 conn=7 transaction=` + healthCfgTx(3) + ` request="/api/v1/config?action=add" msg="DYNCFG: plugin returned code 400 to user initiated call: config health:alert:prototype add x"`,
		`time=2026-10-05T01:00:00.125+00:00 comm=netdata source=daemon level=info tid=77 msg="something else in ` + run + `"`,
	}
	got := healthCfgRecordsOf(lines, run)
	if len(got) != 2 || strings.Contains(got[0], "errno=") || !strings.Contains(got[1], " src_port=P conn=N transaction=sent:"+healthCfgTx(3)+" ") {
		t.Errorf("the records:\n%s", strings.Join(got, "\n"))
	}
	// a web thread's EAGAIN of C's peek at the client is dropped; another errno of a web thread is kept
	peeked := []string{
		`time=2026-10-05T01:00:00.126+00:00 comm=netdata source=daemon level=notice errno="11, Resource temporarily unavailable" tid=91 thread=WEB[2] msg="DYNCFG USER ACTION 'update' on job 'x'"`,
		`time=2026-10-05T01:00:00.127+00:00 comm=netdata source=daemon level=error errno="2, No such file or directory" tid=91 thread=WEB[2] msg="DYNCFG: plugin returned code 400"`,
		`time=2026-10-05T01:00:00.128+00:00 comm=netdata source=daemon level=error errno="11, Resource temporarily unavailable" tid=93 thread=HEALTH msg="DYNCFG: of another thread"`,
	}
	kept := healthCfgRecordsOf(peeked, run)
	if len(kept) != 3 || strings.Contains(kept[0], "errno=") || !strings.Contains(kept[1], `errno="2, `) ||
		!strings.Contains(kept[2], `errno="11, `) {
		t.Errorf("the peek's errno:\n%s", strings.Join(kept, "\n"))
	}
	if err := healthRecordsCount(2, map[string]int{"rejected 'add'": 1, "level=error … msg=\"DYNCFG: plugin returned code 400": 1, "nope": 0,
		"level=error … msg=\"": 2})(got); err != nil {
		t.Errorf("the records' guard: %v", err)
	}
	if healthRecordsCount(3, nil)(got) == nil || healthRecordsCount(-1, map[string]int{"rejected 'add'": 2})(got) == nil ||
		healthRecordsCount(-1, map[string]int{"level=notice … msg=\"DYNCFG": 1})(got) == nil ||
		healthRecordsCount(-1, map[string]int{"msg=\"DYNCFG … level=error": 1})(got) == nil {
		t.Errorf("the records' guard takes a wrong count, a wrong level or parts out of order")
	}

	// the hash alias: by first appearance, only when the case turned it on
	h := &healthPair{}
	view := `{"hash":"11111111-1111-4111-8111-111111111111"},{"hash":"22222222-2222-4222-8222-222222222222"},{"hash":"11111111-1111-4111-8111-111111111111"}`
	if got := h.aliased(0, view); got != view {
		t.Errorf("hashes aliased without the case asking: %s", got)
	}
	h.hashes = [2][]string{{}, {}}
	if got := h.aliased(0, view); got != `{"hash":"<hash 1>"},{"hash":"<hash 2>"},{"hash":"<hash 1>"}` {
		t.Errorf("the hash alias: %s", got)
	}
	if got := h.aliased(1, `{"hash":"22222222-2222-4222-8222-222222222222"}`); got != `{"hash":"<hash 1>"}` {
		t.Errorf("the other side's alias: %s", got)
	}

	// the seconds from a link to the first status: masked, set or not, for an entry from UNINITIALIZED alone
	logView := `"when":T,"duration":0,"non_clear_duration":0,"status":"WARNING","old_status":"UNINITIALIZED","delay":0},{` +
		`"when":T,"duration":T,` + "\n" + `  "non_clear_duration":0,"status":"CLEAR","old_status":"UNINITIALIZED","delay":0},{` +
		`"when":T,"duration":T,"non_clear_duration":T,"status":"REMOVED","old_status":"WARNING",` +
		`"when":T,"duration":0,"non_clear_duration":0,"status":"UNINITIALIZED","old_status":"REMOVED",`
	if got, want := healthSinceLink(logView), `"when":T,"duration":D,"non_clear_duration":0,"status":"WARNING","old_status":"UNINITIALIZED","delay":0},{`+
		`"when":T,"duration":D,`+"\n"+`  "non_clear_duration":0,"status":"CLEAR","old_status":"UNINITIALIZED","delay":0},{`+
		`"when":T,"duration":T,"non_clear_duration":T,"status":"REMOVED","old_status":"WARNING",`+
		`"when":T,"duration":0,"non_clear_duration":0,"status":"UNINITIALIZED","old_status":"REMOVED",`; got != want {
		t.Errorf("the alert log's first statuses:\n%s, want\n%s", got, want)
	}
	entry := func(processed, updated bool, status, old string) string {
		return fmt.Sprintf(`{"name":"a","processed":%t,`+"\n"+`  "updated":%t,"exec_run":0,"exec":"{run}/notify/stub","source":"line=1,file={run}/etc/x",`+
			`"when":T,"duration":T,"non_clear_duration":0,"status":"%s","old_status":"%s","delay":0}`, processed, updated, status, old)
	}
	masked := func(e string) string {
		e = strings.Replace(e, `"processed":true,`, `"processed":P,`, 1)
		return strings.Replace(e, `"processed":false,`, `"processed":P,`, 1)
	}
	for name, c := range map[string]struct {
		entry string
		mask  bool
	}{
		"an unlink a link replaced, seen by the scan": {entry(true, true, "REMOVED", "WARNING"), true},
		"an unlink a link replaced, not seen":         {entry(false, true, "REMOVED", "CLEAR"), true},
		"a link its status replaced":                  {entry(false, true, "UNINITIALIZED", "REMOVED"), true},
		"an unlink nothing replaced":                  {entry(true, false, "REMOVED", "WARNING"), false},
		"a status an unlink replaced":                 {entry(true, true, "WARNING", "UNINITIALIZED"), false},
		"the first pass's unlink of an alert not run": {entry(false, true, "REMOVED", "UNINITIALIZED"), false},
		"the status that stands for the alert":        {entry(true, false, "CLEAR", "UNINITIALIZED"), false},
		"a link nothing replaced yet":                 {entry(false, false, "UNINITIALIZED", "REMOVED"), false},
	} {
		want := c.entry
		if c.mask {
			want = masked(c.entry)
		}
		// between two other entries: a mask does not reach into a neighbour
		before, after := entry(true, false, "CLEAR", "UNINITIALIZED"), entry(false, true, "WARNING", "CLEAR")
		if got := healthReplaced("[" + before + "," + c.entry + "," + after + "]"); got != "["+before+","+want+","+after+"]" {
			t.Errorf("the processed flag of a replaced entry, %s:\n%s", name, got)
		}
	}
	calls := "call 1: argv[9]=WARNING\ncall 1: argv[10]=UNINITIALIZED\ncall 1: argv[14]=0\ncall 1: argv[15]=0\n" +
		"call 2: argv[9]=CLEAR\ncall 2: argv[10]=WARNING\ncall 2: argv[14]=duration\ncall 2: argv[15]=non_clear_duration\n" +
		"call 12: argv[10]=UNINITIALIZED\ncall 12: argv[14]=duration\ncall 12: end exit 0"
	if got, want := healthCallsSinceLink(calls), "call 1: argv[9]=WARNING\ncall 1: argv[10]=UNINITIALIZED\ncall 1: argv[14]=D\ncall 1: argv[15]=0\n"+
		"call 2: argv[9]=CLEAR\ncall 2: argv[10]=WARNING\ncall 2: argv[14]=duration\ncall 2: argv[15]=non_clear_duration\n"+
		"call 12: argv[10]=UNINITIALIZED\ncall 12: argv[14]=D\ncall 12: end exit 0"; got != want {
		t.Errorf("the transcript's first statuses:\n%s, want\n%s", got, want)
	}
	for record, want := range map[string]string{
		`time=T level=warning alert=a alert_duration=T alert_value=70 alert_value_old=null alert_status=WARNING alert_value_old=UNINITIALIZED alert_units=things msg="x"`: `time=T level=warning alert=a alert_duration=D alert_value=70 alert_value_old=null alert_status=WARNING alert_value_old=UNINITIALIZED alert_units=things msg="x"`,
		`time=T level=info alert=a alert_duration=0 alert_value=70 alert_value_old=null alert_status=CLEAR alert_value_old=UNINITIALIZED alert_units=things msg="x"`:      `time=T level=info alert=a alert_duration=D alert_value=70 alert_value_old=null alert_status=CLEAR alert_value_old=UNINITIALIZED alert_units=things msg="x"`,
		`time=T level=info alert=a alert_duration=T alert_value=70 alert_value_old=70 alert_status=CLEAR alert_value_old=WARNING alert_units=things msg="x"`:              `time=T level=info alert=a alert_duration=T alert_value=70 alert_value_old=70 alert_status=CLEAR alert_value_old=WARNING alert_units=things msg="x"`,
		`time=T level=debug alert=a alert_duration=0 alert_value=null alert_value_old=null alert_status=UNINITIALIZED alert_value_old=REMOVED alert_units=things msg="x"`: `time=T level=debug alert=a alert_duration=0 alert_value=null alert_value_old=null alert_status=UNINITIALIZED alert_value_old=REMOVED alert_units=things msg="x"`,
	} {
		if got := healthLogSinceLink(record); got != want {
			t.Errorf("health.log's first status:\n%s, want\n%s", got, want)
		}
	}

	// a pair's own bound on the two sides' event times: the runner's two seconds unless the case set one
	logOf := func(when int64) string { return fmt.Sprintf(`[{"unique_id":1,"when":%d,"duration":0}]`, when) }
	two, wide := &healthPair{raw: [2]map[string]string{{}, {}}, clocks: [2]map[string][]int64{{}, {}}},
		&healthPair{raw: [2]map[string]string{{}, {}}, clocks: [2]map[string][]int64{{}, {}}, bound: healthStoreBound}
	for apart, want := range map[int64][2]bool{0: {true, true}, 2: {true, true}, 3: {false, true}, healthStoreBound: {false, true},
		healthStoreBound + 1: {false, false}} {
		for k, pair := range []*healthPair{two, wide} {
			pair.raw[0]["view"], pair.raw[1]["view"] = logOf(1790000000), logOf(1790000000+apart)
			if err := pair.near("view", "view"); (err == nil) != want[k] {
				t.Errorf("two sides %d s apart, the pair's bound %d: %v, want within: %t", apart, pair.bound, err, want[k])
			}
			pair.raw[0]["view"], pair.raw[1]["view"] = logOf(1790000000+apart), logOf(1790000000)
			if err := pair.near("view", "view"); (err == nil) != want[k] {
				t.Errorf("two sides %d s apart the other way, the pair's bound %d: %v, want within: %t", apart, pair.bound, err, want[k])
			}
		}
	}

	// the last change of an alert
	changes := "a: UNINITIALIZED->WARNING 70 things\na: WARNING->REMOVED 70 things\nab: UNINITIALIZED->CLEAR 70 things"
	if err := healthLastChange("a", "WARNING->REMOVED")(changes); err != nil {
		t.Errorf("the last change: %v", err)
	}
	if healthLastChange("a", "UNINITIALIZED->CLEAR")(changes) == nil || healthLastChange("b", "x")(changes) == nil {
		t.Errorf("the last change's guard takes another alert's")
	}
	if got := filepath.Base(healthSavedName(healthJobPrefix)); got != "health%3Aalert%3Aprototype.dyncfg" {
		t.Errorf("the template's file: %q", got)
	}
}
