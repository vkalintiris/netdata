// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"cmp"
	"encoding/json"
	"fmt"
	"net/http"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The alert endpoints of the contexts v2 engine (milestone 10, D224; plan evidence/2026-10-06-plan-m10-commit0.md §3):
// `/api/v2/alerts` and `/api/v3/alerts` read the alerts' live state, `/api/v2/alert_transitions` and
// `/api/v3/alert_transitions` the alert log's transitions in SQLite (database/contexts/api_v2_contexts_alerts.c,
// api_v2_contexts_alert_transitions.c, database/sqlite/sqlite_health.c:1458-1652). They run on the health runner: both
// agents hold the same alerts and transitions, each with ids and clocks of its own, which the families render on each
// side, against that side's own alert log, before the answers are parsed (alertsV2Render).

var (
	// the members that hold a wall-clock second or a span between two events: an alert's last change and last evaluation
	// (`tr_t`, `t`: api_v2_contexts_alerts.c:362, :377), a transition's time, its notification's and the end of its delay
	// (`when`, `delay_up_to_time`: api_v2_contexts_alert_transitions.c:431, :454, :456), the spans before it (`duration`,
	// `raised_duration`: :447-448); and an entry's global id, the microseconds at its creation (`gi`:
	// api_v2_contexts_alerts.c:337, api_v2_contexts_alert_transitions.c:400; health/health_log.c:226). With
	// `options=long-json-keys` the first two and the global id have their long names (alertsV2LongKeys); with
	// `options=rfc3339` an alert's two seconds are dates (alertsV2Second)
	alertsV2ClockRe = regexp.MustCompile(`"(gi|global_id|t|last_updated_timestamp|tr_t|last_transition_timestamp|when|` +
		`delay_up_to_time|duration|raised_duration)":(\s*)(\d+|"[^"\\]*")`)
	// an alert's last transition id and a transition's id: random UUIDs (health/health_log.c:225;
	// api_v2_contexts_alerts.c:360, api_v2_contexts_alert_transitions.c:404)
	alertsV2TidRe = regexp.MustCompile(`"(tr_i|last_transition_id|transition_id)":(\s*)"(` + alertsV2UUID + `)"`)
	// an alert's or a transition's global id, the member each of them has once, before its other clocks
	// (api_v2_contexts_alerts.c:337, api_v2_contexts_alert_transitions.c:400)
	alertsV2ItemRe = regexp.MustCompile(`"(?:gi|global_id)":\s*\d+`)
	// a second as C prints it with `options=rfc3339`: UTC, and no fraction for a whole second
	// (buffer_json_member_add_time_t_formatted, libnetdata/buffer/buffer.h:1119-1128: a 0 prints null, and a second of
	// three years or less, a relative time, stays a number, :12, :1120; libnetdata/datetime/rfc3339.c:139-156)
	alertsV2DateRe = regexp.MustCompile(`^"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ"$`)
)

// alertsV2UUID is a UUID as C prints one: lower case, with its dashes (uuid_unparse_lower).
const alertsV2UUID = `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`

// alertsV2LongKeys are the long names of an alert's clock members, by which `options=long-json-keys` prints them
// (libnetdata/json/json-keys.c:125, :129, :131 against :56, :60, :62), with the short name the render reads each as.
var alertsV2LongKeys = map[string]string{"global_id": "gi", "last_updated_timestamp": "t", "last_transition_timestamp": "tr_t"}

// alertsV2Second is the second a clock member holds as written: a number, or a date in the one shape C prints
// (alertsV2DateRe; `date` then). Anything else is no second: it is left to the comparison as the agent wrote it.
func alertsV2Second(text string) (second int64, date, ok bool) {
	if alertsV2DateRe.MatchString(text) {
		t, err := time.Parse(`"2006-01-02T15:04:05Z"`, text)
		return t.Unix(), true, err == nil
	}
	v, err := strconv.ParseInt(text, 10, 64)
	return v, false, err == nil
}

// alertsV2Entry is what the alert endpoints' times and spans are read against: an entry of the side's own alert log,
// `/api/v1/alarm_log`. Both endpoints print the entry's stored columns (database/sqlite/sqlite_health.c:1063-1070,
// :1158-1185: the log; :1462-1466 and api_v2_contexts_alert_transitions.c:431-456: a transition), and an alert its
// last entry's id and the second of its last change (health/health_log.c:225, :238; api_v2_contexts_alerts.c:360-362).
// The members are the ones the render reads (healthEntry, the health checks' view of an entry, has no end of the
// delay).
type alertsV2Entry struct {
	Tid       string `json:"transition_id"`
	Status    string `json:"status"`
	OldStatus string `json:"old_status"`
	When      int64  `json:"when"`
	Duration  int64  `json:"duration"`
	NonClear  int64  `json:"non_clear_duration"`
	ExecRun   int64  `json:"exec_run"`
	DelayUpTo int64  `json:"delay_up_to_timestamp"`
	// as, when set, is the name the entry's transition id prints under: an entry of another host than the answering
	// agent's own, named by that host's normalizer (alertsV2Host)
	as string
}

// named is the name the entry's transition id prints under: its own host's (as), else what n, the normalizer of the
// answering agent's own host, calls it.
func (e alertsV2Entry) named(n *healthNorm) string {
	return cmp.Or(e.as, n.tid(e.Tid))
}

// alertsV2Dated ends the mark of a second that the agent wrote as a date (`options=rfc3339`): the shape stays
// compared, a number against a date differs.
const alertsV2Dated = " as a date"

// lastChange is the second an alert whose last entry is e changed status last, as C keeps it for the alert: the
// entry's `when` (a status change's entry and the alert take the pass's one clock, health/health_event_loop.c:738-741,
// :768), but for an alert that never had a status, whose last entry is its link's, from REMOVED to UNINITIALIZED
// (made for a new alert alone, health/rrdcalc.c:457): C takes one clock when it makes the alert (:413) and reads
// another for the link's entry, whose duration is the seconds between the two (:320, :325). An alert that left
// REMOVED by a status change has an entry from REMOVED too, to a status that is never UNINITIALIZED
// (health/health_event_loop.c:518-519, :668-690): its last change is that entry's `when`.
func (e alertsV2Entry) lastChange() int64 {
	if e.OldStatus == "REMOVED" && e.Status == "UNINITIALIZED" {
		return e.When - e.Duration
	}
	return e.When
}

// alertsV2Log is a side's alert log: its entries by transition id.
type alertsV2Log map[string]alertsV2Entry

// alertsV2LogOf reads an `/api/v1/alarm_log` body.
func alertsV2LogOf(body []byte) (alertsV2Log, error) {
	var entries []alertsV2Entry
	if err := json.Unmarshal(body, &entries); err != nil {
		return nil, err
	}
	log := alertsV2Log{}
	for _, e := range entries {
		if e.Tid != "" {
			log[e.Tid] = e
		}
	}
	return log, nil
}

// join adds another host's alert log to a side's (body: that host's `/api/v1/alarm_log`), each entry named
// `<host>:t+k` by the host's own normalizer n, which takes the log first (healthNorm.observe). An id both logs hold
// is an error: a transition id is one entry's.
func (log alertsV2Log) join(host string, n *healthNorm, body []byte) error {
	theirs, err := alertsV2LogOf(body)
	if err != nil {
		return err
	}
	n.observe(string(body))
	for id, e := range theirs {
		if _, both := log[id]; both {
			return fmt.Errorf("the transition id %s is in the agent's own log and in %s's", id, host)
		}
		e.as = host + ":" + n.tid(id)
		log[id] = e
	}
	return nil
}

// alertsV2Render renders one side's alerts or transitions body for the comparison, each id, time and span by what it
// is in the side's own alert log (log; n names the ids), and only where it is that. Each alert instance and each
// transition is read from its global id to the next one's (alertsV2ItemRe) against the log's entry its transition id
// names (`tr_i`, `transition_id`):
//   - the transition id reads `t+k`, the entry's unique id from the side's base (healthNorm.tid);
//   - a transition's `when` reads WHEN where it is the entry's `when`, and an alert's last change (`tr_t`) where it
//     is the second the entry says the alert changed last (alertsV2Entry.lastChange);
//   - a transition's `duration` and `raised_duration` read DURATION and NON_CLEAR where they are the entry's
//     `duration` and `non_clear_duration`; its notification's `when` and `delay_up_to_time` read EXEC_RUN and
//     DELAY_UP_TO where they are the entry's `exec_run` and `delay_up_to_timestamp`;
//   - the global id reads G where it is a microsecond of the entry's `when` or of the healthBound seconds after it:
//     C reads the clock for it as it makes the entry (health/health_log.c:226), after the one that gave `when`, with
//     no bound of its own; when the body has two or more, only if some global id is no whole second (one alone cannot
//     tell);
//   - an alert's last evaluation (`t`) reads T where it is a second of the request's flight or of the healthBound
//     seconds before it (an alert that is evaluated is evaluated each second in these cases; one that never was has
//     0).
//
// A 0 stays 0. Anything else is left as the agent wrote it, for the comparison: an id the log does not hold, a time
// that is not its entry's, a global id in seconds. It returns the seconds it replaced in the body's order (a global
// id's whole seconds), for the bound beside the render (healthClocksNear).
//
// With `options=long-json-keys` the members are read by their long names (alertsV2LongKeys), and with
// `options=rfc3339` an alert's two seconds as the dates C prints (alertsV2Second; the mark then ends alertsV2Dated).
// An entry of another host than n's is named by its own host (alertsV2Entry.named). The MCP form of the alerts
// answer has no keys and no global id: its instance rows are rendered by position (alertsV2RenderMCP). In either
// form a byte that is no part of a UTF-8 sequence is first written out (alertsV2Bytes).
func alertsV2Render(n *healthNorm, log alertsV2Log, flight [2]int64, body []byte) ([]byte, []int64) {
	body = alertsV2Bytes(body)
	if bytes.Contains(body, []byte(`"alert_instances_header"`)) {
		return alertsV2RenderMCP(n, log, flight, body)
	}
	micro, ids := false, alertsV2ItemRe.FindAll(body, -1)
	for _, m := range ids {
		_, digits, _ := strings.Cut(string(m), ":")
		if v, err := strconv.ParseInt(strings.TrimSpace(digits), 10, 64); err == nil && v%1_000_000 != 0 {
			micro = true
		}
	}
	micro = micro || len(ids) < 2
	var clocks []int64
	item := func(span string) string {
		// the item's one transition id (an alert's `tr_i`, a transition's `transition_id`) and its entry
		var entry alertsV2Entry
		known := false
		if g := alertsV2TidRe.FindStringSubmatch(span); g != nil {
			entry, known = log[g[3]]
		}
		whens := 0
		span = alertsV2ClockRe.ReplaceAllStringFunc(span, func(m string) string {
			g := alertsV2ClockRe.FindStringSubmatch(m)
			key := cmp.Or(alertsV2LongKeys[g[1]], g[1])
			v, date, ok := alertsV2Second(g[3])
			if key == "when" {
				whens++
			}
			if !ok || v == 0 {
				return m
			}
			mark, second := "", v
			switch {
			case key == "t":
				if v >= flight[0]-healthBound && v <= flight[1] {
					mark = "T"
				}
			case !known:
			case key == "gi":
				if second = v / 1_000_000; micro && second >= entry.When && second <= entry.When+healthBound {
					mark = "G"
				}
			case key == "tr_t":
				if v == entry.lastChange() {
					mark = "WHEN"
				}
			case key == "when" && whens == 1:
				if v == entry.When {
					mark = "WHEN"
				}
			case key == "when":
				if v == entry.ExecRun {
					mark = "EXEC_RUN"
				}
			case key == "duration":
				if v == entry.Duration {
					mark = "DURATION"
				}
			case key == "raised_duration":
				if v == entry.NonClear {
					mark = "NON_CLEAR"
				}
			case key == "delay_up_to_time":
				if v == entry.DelayUpTo {
					mark = "DELAY_UP_TO"
				}
			}
			if mark == "" {
				return m
			}
			if date {
				mark += alertsV2Dated
			}
			clocks = append(clocks, second)
			return `"` + g[1] + `":` + g[2] + `"` + mark + `"`
		})
		if !known {
			return span
		}
		return alertsV2TidRe.ReplaceAllStringFunc(span, func(m string) string {
			g := alertsV2TidRe.FindStringSubmatch(m)
			return `"` + g[1] + `":` + g[2] + `"` + entry.named(n) + `"`
		})
	}
	var out strings.Builder
	last := 0
	for k, loc := range alertsV2ItemRe.FindAllIndex(body, -1) {
		if k == 0 {
			out.Write(body[:loc[0]])
		} else {
			out.WriteString(item(string(body[last:loc[0]])))
		}
		last = loc[0]
	}
	if len(ids) == 0 {
		return body, nil
	}
	out.WriteString(item(string(body[last:])))
	return []byte(out.String()), clocks
}

// alertsV2Marker starts the text alertsV2Bytes writes for a byte that is no part of a UTF-8 sequence: a character of
// the private use area, which no text of the fixtures holds.
const alertsV2Marker = '\uf8ff'

// alertsV2Bytes is body with every byte that is no part of a UTF-8 sequence written as alertsV2Marker and the
// byte's two hex digits, and every alertsV2Marker the agent wrote itself written twice: so no bytes an agent can
// write read as a written-out byte, and the body still parses. (The one other spelling of that text, the marker as a
// JSON escape, parses to it and is told apart by its escape, which the comparison holds: v2Escapes.) The parser
// alone reads such a byte as U+FFFD, as it reads a U+FFFD the agent wrote: C cuts a chart's module at 127 bytes,
// inside a character too (api_v2_contexts_alerts.c:141-142; strncpyz), and prints the bytes it has
// (buffer_json_strcat, libnetdata/buffer/buffer.h:301-374).
func alertsV2Bytes(body []byte) []byte {
	if utf8.Valid(body) && !bytes.ContainsRune(body, alertsV2Marker) {
		return body
	}
	var out bytes.Buffer
	for len(body) > 0 {
		r, size := utf8.DecodeRune(body)
		switch {
		case r == utf8.RuneError && size == 1:
			fmt.Fprintf(&out, "%c%02x", alertsV2Marker, body[0])
		case r == alertsV2Marker:
			out.WriteString(strings.Repeat(string(alertsV2Marker), 2))
		default:
			out.Write(body[:size])
		}
		body = body[size:]
	}
	return out.Bytes()
}

var (
	// an MCP instance row's last transition, three items in a row: its id as text, its value and its second
	// (api_v2_contexts_alerts.c:577-583); the row's configuration hash follows a second and is followed by a text
	alertsV2McpChangeRe = regexp.MustCompile(`"(` + alertsV2UUID + `)",(\s*)(null|-?[0-9][0-9.eE+-]*),(\s*)(\d+|"[^"\\]*")`)
	// the last item of a row or list where it is a number or a text: with `values` an instance row ends with its last
	// evaluation's second (api_v2_contexts_alerts.c:596-599)
	alertsV2McpLastRe = regexp.MustCompile(`,(\s*)(\d+|"[^"\\]*")(\s*)\]`)
)

// alertsV2RenderMCP is alertsV2Render for the MCP form of the alerts answer (`options=mcp`: headers and rows,
// api_v2_contexts_alerts.c:434-619), whose instance rows hold what the plain form's instances do, but the global id,
// the two indexes and the chart's id, and with the host's name, by position:
//   - a row's last transition id reads `t+k` (alertsV2Entry.named) where the side's log holds it, and the second two
//     items after it WHEN where it is the second that entry says the alert changed last (alertsV2Entry.lastChange);
//   - a row's last item reads T where it is a second of the request's flight or of the healthBound seconds before it:
//     the alert's last evaluation, the one item of the answer that can hold such a second there (a header ends with
//     a text, a summary row with a count).
//
// A second written as a date has its mark end alertsV2Dated. Anything else is left as the agent wrote it.
func alertsV2RenderMCP(n *healthNorm, log alertsV2Log, flight [2]int64, body []byte) ([]byte, []int64) {
	var clocks []int64
	text := alertsV2McpChangeRe.ReplaceAllStringFunc(string(body), func(m string) string {
		g := alertsV2McpChangeRe.FindStringSubmatch(m)
		entry, known := log[g[1]]
		if !known {
			return m
		}
		second := g[5]
		if v, date, ok := alertsV2Second(g[5]); ok && v != 0 && v == entry.lastChange() {
			second = `"WHEN"`
			if date {
				second = `"WHEN` + alertsV2Dated + `"`
			}
			clocks = append(clocks, v)
		}
		return `"` + entry.named(n) + `",` + g[2] + g[3] + `,` + g[4] + second
	})
	text = alertsV2McpLastRe.ReplaceAllStringFunc(text, func(m string) string {
		g := alertsV2McpLastRe.FindStringSubmatch(m)
		v, date, ok := alertsV2Second(g[2])
		if !ok || v < flight[0]-healthBound || v > flight[1] {
			return m
		}
		mark := "T"
		if date {
			mark += alertsV2Dated
		}
		clocks = append(clocks, v)
		return `,` + g[1] + `"` + mark + `"` + g[3] + `]`
	})
	return []byte(text), clocks
}

// alertsV2LogReader reads side i's alert log as the health pair h serves it now.
func alertsV2LogReader(h *healthPair) func(i int) ([]byte, error) {
	return alertsV2LogReaderOf(h, "")
}

// alertsV2LogReaderOf is alertsV2LogReader for the alert log of the host at prefix (a child's: `/host/<name>`;
// empty: the agent's own).
func alertsV2LogReaderOf(h *healthPair, prefix string) func(i int) ([]byte, error) {
	return func(i int) ([]byte, error) {
		r := healthGet(h.p.Each()[i].Daemon, prefix+"/api/v1/alarm_log")
		if r.Status != http.StatusOK {
			return nil, fmt.Errorf("%s/api/v1/alarm_log answered %d: %s", prefix, r.Status, truncateBytes(r.Body))
		}
		return r.Body, nil
	}
}

// alertsV2Host is another host of the pair's agents whose alerts an answer lists beside the agents' own (a child):
// its name, under which its transition ids print (`<name>:t+k`), each side's normalizer of its ids (they count from
// seeds of its own) and a reader of its alert log on side i.
type alertsV2Host struct {
	name string
	n    [2]*healthNorm
	log  func(i int) ([]byte, error)
}

// alertsV2Sets are the five sets of names of a summary entry, by their short and long keys, and the five items of
// an MCP summary row that hold them when they have two names or more (api_v2_contexts_alerts.c:658-677, :479-483):
// the contexts, classifications, components, types and recipients of the alerts of one name. C keeps each in a label
// set made for the request, whose names it walks in the order of the addresses they were interned at
// (database/rrdlabels.c:267, :455-465, :467-495), which is no order of the answer's: two names compare as a set. Each
// is a list of strings, so the answer's layout does not change with their order (v2Family.flat).
var alertsV2Sets = []string{
	"alerts.[].ctx[]", "alerts.[].cls[]", "alerts.[].cp[]", "alerts.[].ty[]", "alerts.[].to[]",
	"alerts.[].contexts[]", "alerts.[].classifications[]", "alerts.[].components[]", "alerts.[].types[]",
	"alerts.[].recipients[]",
	"all_alerts.[].[2][]", "all_alerts.[].[3][]", "all_alerts.[].[4][]", "all_alerts.[].[5][]", "all_alerts.[].[6][]",
}

// alertsV2Family compares the alert endpoints' answers, `/api/v2|v3/alerts` and `/api/v2|v3/alert_transitions`. With
// the health runner's normalizers (n[i] side i's) and a reader of each side's alert log (log, read again after every
// answer: an entry's notification is stored after the entry): each side's body rendered against its own log
// (alertsV2Render), the seconds the render replaced held within healthBound side to side, and healthCandidateWait for
// the candidate to show the oracle's answer; the names of a summary entry's five sets compared as sets (alertsV2Sets).
// A side whose log cannot be read has its body left unparsable, with the reason. `more` are the other hosts whose
// alerts the answers list (alertsV2Host): their entries join the side's log, each named by its own host. With the
// zero pair (health off: the dashboard replay, whose agents hold no alert) the v2 envelope's masks alone, without a
// render, a bound or a wait.
func alertsV2Family(n [2]*healthNorm, log func(i int) ([]byte, error), more ...alertsV2Host) v2Family {
	if n[0] == nil || n[1] == nil {
		return v2Family{masks: infoV2Volatile}
	}
	var clocks [2][]int64
	return v2Family{
		masks:     infoV2Volatile,
		unordered: alertsV2Sets,
		flat:      true,
		settle:    healthCandidateWait,
		render: func(i int, flight [2]int64, body []byte) []byte {
			raw, err := log(i)
			var entries alertsV2Log
			if err == nil {
				entries, err = alertsV2LogOf(raw)
			}
			for _, host := range more {
				if err != nil {
					break
				}
				var theirs []byte
				if theirs, err = host.log(i); err == nil {
					err = entries.join(host.name, host.n[i], theirs)
				}
			}
			if err != nil {
				return append([]byte("the side's alert log: "+err.Error()+"\n"), body...)
			}
			n[i].observe(string(raw))
			out, c := alertsV2Render(n[i], entries, flight, body)
			clocks[i] = c
			return out
		},
		check: func(t *testing.T, name string, _, _ Value) {
			t.Helper()
			if err := healthClocksNear(clocks[0], clocks[1]); err != nil {
				t.Errorf("%s: %v", name, err)
				return
			}
			apart := int64(0)
			for k := range clocks[0] {
				apart = max(apart, clocks[0][k]-clocks[1][k], clocks[1][k]-clocks[0][k])
			}
			t.Logf("%s: the two sides' %d times and spans are at most %d s apart (the bound is %d)", name,
				len(clocks[0]), apart, healthBound)
		},
	}
}

// alertsV2Rows is what a guard reads of one member of an answer: for an array, the members `keys` of each item, one
// line per item (the values joined by spaces; a key with dots names a nested member, a key `[i]` the i-th item of an
// item that is a list, an MCP row; a key that ends `{}` names a list read as a set, its items sorted: `{a,b}`; a key
// that ends `?` reads `-` where the item has no such member), compared as a sorted set when `sorted` (the order is
// the comparison's to judge); for an object, one line of its members `keys`. A nil `items` wants the answer without
// the member.
type alertsV2Rows struct {
	member string
	keys   []string
	items  []string
	sorted bool
}

// alertsV2Guard is a guard on a v2 answer: each of rows as it says.
func alertsV2Guard(rows ...alertsV2Rows) func(Value) error {
	return func(v Value) error {
		for _, r := range rows {
			m, err := dashMember(v, r.member)
			if r.items == nil {
				if err == nil {
					return fmt.Errorf("the answer has %s", r.member)
				}
				continue
			}
			if err != nil {
				return err
			}
			items := m.Items
			switch m.Kind {
			case KindArray:
			case KindObject:
				items = []Value{m}
			default:
				return fmt.Errorf("%s is %s, neither a list nor an object", r.member, m)
			}
			var got []string
			for _, item := range items {
				var fields []string
				for _, k := range r.keys {
					path, optional := strings.CutSuffix(k, "?")
					path, set := strings.CutSuffix(path, "{}")
					f, err := dashAt(item, strings.Split(path, ".")...)
					if err != nil && !optional {
						return fmt.Errorf("%s: %v", r.member, err)
					}
					switch {
					case err != nil:
						fields = append(fields, "-")
					case set && f.Kind == KindArray:
						names := make([]string, len(f.Items))
						for i, name := range f.Items {
							names[i] = name.Text
						}
						slices.Sort(names)
						fields = append(fields, "{"+strings.Join(names, ",")+"}")
					case f.Kind == KindString:
						fields = append(fields, f.Text)
					default:
						fields = append(fields, f.String())
					}
				}
				got = append(got, strings.Join(fields, " "))
			}
			want := r.items
			if r.sorted {
				slices.Sort(got)
				want = slices.Sorted(slices.Values(r.items))
			}
			if !slices.Equal(got, want) {
				return fmt.Errorf("%s (%s) is %q, want %q", r.member, strings.Join(r.keys, " "), got, want)
			}
		}
		return nil
	}
}

// alertsV2AccessCase runs a case's access rows (each configuration's pair has no health, D224 F1) once its health
// pair stopped: the routes' ACL is the dashboard's (HTTP_ACL_ALERTS, libnetdata/user-auth/http-access.h:103;
// web/api/web_api.c:82-96: 451 from a client the list leaves out) and their access anonymous data (412 for an
// anonymous client under bearer protection, web/server/web_client.c:57-70): accessRoutes.
func alertsV2AccessCase(routes ...string) func(t *testing.T, h *healthPair) {
	return func(t *testing.T, _ *healthPair) {
		t.Run("access", func(t *testing.T) { accessRows(t, []accessConf{accessACL, accessBearer}, accessRoutes(routes...)) })
	}
}

// TestAlertsV2 (checks `api.v2-alerts` = `TestAlertsV2/alerts`, `/sets` and `/off`, and `api.v2-alert-transitions` =
// `TestAlertsV2/transitions`, milestone 10 commit 0, D224; the rows of D234 F2 and F3 since commit 4): the alert
// endpoints of the contexts v2 engine, on the health runner. Each case compares a v1 answer of the same state first
// (the green anchor), reads each side's alert log for its ids' aliases, then asks the v2 endpoints (compareV2: the
// oracle's status and guard, then the candidate's answer within healthCandidateWait), and, for `alerts` and
// `transitions`, once both agents stopped, its routes' access rows (`access`). Cases:
//   - `alerts`: `health.api` `endpoints`' three alerts, one WARNING, and a fourth on a chart never collected
//     (alertsV2Conf): `/api/v3/alerts` with `status=raised`, with the summary alone, with `alert=` of two names, and
//     `/api/v2/alerts` pretty; then the status words, the long keys with the request's echo, the MCP form, dates, the
//     host prefilter under a window, the selectors that filter nothing or everything (alertsV2AlertRows),
//     `transition=` with each side's own ids (alertsV2TransitionRows) and its 404 as the agents write it
//     (alertsV2NotFoundRows);
//   - `sets`: one alert name on two charts of two contexts by two rules, a template on two charts, texts with
//     variables, a module cut inside a character, and at its end a child with a chart on the template's context
//     (alertsV2SetsConf, alertsV2PlaySets): what a summary entry holds of several alerts, and of two hosts;
//   - `off`: health off and one collected chart: a window lists no host (alertsV2PlayOff);
//   - `transitions`: `health.transitions`' three alerts through CLEAR, WARNING, CRITICAL and CLEAR:
//     `/api/v2/alert_transitions` over the last ten minutes, its newest four after the second switch (`anchor_gi`), and
//     `/api/v3/alert_transitions` of one transition by its id, each side's own (v2Req.targets): a change to WARNING,
//     which the window lists too, and a first status, which it does not.
//
// The alerts rows of two hosts with an alert each are `health.child` `two-hosts`' (alertsV2TwoHosts). Neither endpoint
// runs a data query: asking them does not pause HEALTH (stream-control.c:99-103).
func TestAlertsV2(t *testing.T) {
	runHealthCases(t, map[string]healthCase{
		"alerts": {
			conf:  alertsV2Conf,
			sc:    alertsV2Scenario(),
			play:  alertsV2PlayAlerts,
			after: alertsV2AccessCase("/api/v2/alerts", "/api/v3/alerts"),
		},
		"sets": {
			conf:   alertsV2SetsConf,
			sc:     alertsV2SetsScenario(),
			stream: healthChildSection("health enabled = yes", "postpone alerts on connect = 0"),
			play:   alertsV2PlaySets,
		},
		"off": {
			off:  true,
			sc:   healthValues("hoff.values", "hoff.ctx", []string{"a"}, map[string]int64{"a": 10}),
			play: alertsV2PlayOff,
		},
		"transitions": {
			conf:  healthSigConf,
			grid:  healthSigGrid,
			sc:    healthValues("hsig.values", "hsig.ctx", []string{"a"}, healthSigPhases...),
			play:  alertsV2PlayTransitions,
			after: alertsV2AccessCase("/api/v2/alert_transitions", "/api/v3/alert_transitions"),
		},
	})
}

// alertsV2Module is the module of the `alerts` case's collected chart (plugin.Values.Module). C groups alerts by their
// chart's `_collect_module` label (api_v2_contexts_alerts.c:141-151), which holds the module sanitized as a label
// value (rrdset-index-id.c:23-28; rrdlabels.c:351), where a comma becomes a period (sanitizers-labels.c:69).
const alertsV2Module = "parity,mod"

// alertsV2PlainEmit defines the `alerts` case's second chart, of the same context, without a module (C labels it
// "[none]": sanitizers-labels.c:152) and never collected (healthScenario's lines).
const alertsV2PlainEmit = "CHART hsig.plain '' 'title' 'units' 'family' 'hsig.ctx' line 1000 1 '' '' ''\n" +
	"DIMENSION b '' absolute 1 1\n"

// alertsV2Conf are `health.api` `endpoints`' three alerts (healthAPIConf) and hm_plain on the chart never collected,
// with a class, and a type and a component of two words each, and the recipient `silent` (D234 F2: what stock rules
// say, `type: Web Server`, `to: silent`). The groupings by them print the rule's texts (api_v2_contexts_alerts.c:
// 113-139, :385-392); a summary entry's sets hold the same texts as label names, where a blank is an underscore
// (:266-275; libnetdata/sanitizers/sanitizers-labels.c:142-149); and an alert whose recipient is exactly `silent`
// counts as `running_silent` in each of its five groupings (api_v2_contexts_alerts.c:245-246).
const alertsV2Conf = healthAPIConf + `
 alarm: hm_plain
    on: hsig.plain
  calc: $b
 every: 1s
  warn: $this > 1000
 units: things
 class: Workload
  type: Parity Check
component: Fixture Part
    to: silent
  info: b above 1000
`

// alertsV2Scenario is the `alerts` case's plugin: hsig.values of `endpoints` with alertsV2Module, created after
// hsig.plain (alertsV2PlainEmit) in one write, so both agents create the two charts in one order.
func alertsV2Scenario() *plugin.Scenario {
	sc := healthScenario(alertsV2PlainEmit, "hsig.values", "hsig.ctx", []string{"a"}, map[string]int64{"a": 10},
		map[string]int64{"a": 70})
	healthChart(sc).Module = alertsV2Module
	return sc
}

// alertsV2Route is the alerts route the rows ask unless they say otherwise, and alertsV2Full the options of a row
// that asks for everything, minified.
const (
	alertsV2Route = "/api/v3/alerts"
	alertsV2Full  = "options=summary,instances,values,minify"
)

// alertsV2PlayAlerts plays `alerts`: the endpoint check's state (health.api `endpoints`: a = 70, ha_low WARNING, ha_mid
// and ha_high CLEAR), then the v2 rows: the fixed ones, the ones that name a transition of each side's own alert log,
// and the 404s, compared raw.
func alertsV2PlayAlerts(t *testing.T, h *healthPair) {
	h.create(t)
	h.waitOracle(t, "the chart's alerts", func() (string, error) {
		v := h.get(0, "/api/v1/alarms?all")
		return v, healthAll("CLEAR", "ha_low", "ha_mid", "ha_high")(v)
	})
	h.release(t, "p1", 1, healthCalcHold)
	// the green anchor, as health.api compares it
	h.compareNow(t, "/api/v1/alarms?all", func(i int) string { return h.get(i, "/api/v1/alarms?all") },
		healthWant(map[string]string{"ha_low": "WARNING", "ha_mid": "CLEAR", "ha_high": "CLEAR"}))
	// the ids' aliases: each side's alert log, once it holds ha_low's change (an alert's `tr_i` is its last entry's id)
	h.compareNow(t, "the alert log's transitions", func(i int) string { return h.transitions(i, "") },
		alertsV2LogHolds("ha_low: CLEAR->WARNING 70 things"))
	fam := alertsV2Family(h.n, alertsV2LogReader(h))
	for _, req := range alertsV2AlertRows {
		compareV2(t, h.p, req, fam)
	}
	// ha_low's change to WARNING, the alert's last transition, and its first status, an older one of the same alert
	last := alertsV2TransitionOf(t, h, h.n, "", "ha_low", "CLEAR", "WARNING")
	first := alertsV2TransitionOf(t, h, h.n, "", "ha_low", "UNINITIALIZED", "CLEAR")
	for _, req := range alertsV2TransitionRows(last, first) {
		compareV2(t, h.p, req, fam)
	}
	for _, row := range alertsV2NotFoundRows {
		t.Run(row.name, func(t *testing.T) {
			for _, problem := range alertsV2RawRound(h.p, row) {
				t.Error(problem)
			}
		})
	}
}

// alertsV2NewestChange is the transition id of the newest of entries (an alert log in the order of its ids) that took
// the alert `name` from the status `old` to `status`; empty when none did.
func alertsV2NewestChange(entries []healthEntry, name, old, status string) string {
	id := ""
	for _, e := range entries {
		if e.Name == name && e.OldStatus == old && e.Status == status {
			id = e.Tid
		}
	}
	return id
}

// alertsV2TransitionOf is, per side, the id of the newest entry of the host's alert log (prefix: empty for the
// agent's own, `/host/<name>` for a child's, read with the host's normalizers n) that took the alert `name` from the
// status `old` to `status` (alertsV2NewestChange). The oracle must have one; a candidate without one is reported and
// gets an empty id, with which a row is still asked: it shows what the candidate answers then.
func alertsV2TransitionOf(t *testing.T, h *healthPair, n [2]*healthNorm, prefix, name, old, status string) [2]string {
	t.Helper()
	var ids [2]string
	for i, side := range h.p.Each() {
		entries, err := h.entriesAs(n[i], i, prefix+"/api/v1/alarm_log")
		if err != nil && side.Role == Oracle {
			t.Fatalf("oracle: %v", err)
		}
		if ids[i] = alertsV2NewestChange(entries, name, old, status); ids[i] != "" {
			continue
		}
		if side.Role == Oracle {
			t.Fatalf("oracle: its alert log%s has no change of %s from %s to %s", prefix, name, old, status)
		}
		t.Errorf("candidate: its alert log%s has no change of %s from %s to %s", prefix, name, old, status)
	}
	return ids
}

// alertsV2Nodes is a guard's view of the nodes an answer lists, each with its index (alertsV2Parent, alertsV2Child):
// exactly these, in this order; without an item, none.
func alertsV2Nodes(items ...string) alertsV2Rows {
	if items == nil {
		items = []string{}
	}
	return alertsV2Rows{member: "nodes", keys: []string{"nm", "ni"}, items: items}
}

// alertsV2Parent is the pair's own host as alertsV2Nodes reads it, and alertsV2Child the health checks' child after
// it.
const (
	alertsV2Parent = "parity-parent 0"
	alertsV2Child  = "health-child 1"
)

// The keys by which the guards read a grouping's entries (api_v2_contexts_alerts.c:394-418): an entry by module has
// no `available` (:385-392 counts the rules by type, component, classification and recipient alone), and neither
// has an entry no rule's first name gave (alertsV2Rows reads `-` there).
var (
	alertsV2ModuleKeys    = []string{"name", "cr", "wr", "cl", "running", "running_silent"}
	alertsV2GroupKeys     = []string{"name", "running", "running_silent", "available?"}
	alertsV2RecipientKeys = []string{"name", "cr", "wr", "cl", "running", "running_silent", "available"}
)

// alertsV2AlertRows are the `alerts` case's fixed v2 rows. Their guards read what C answers for alertsV2Conf at
// a = 70:
//   - the summary (`alerts`, api_v2_contexts_alerts.c:627-683) has one item per alert name the filters keep, with its
//     instances by status (:183-208), in the order the alerts were met: hsig.plain's hm_plain first, its chart created
//     first (the context's instances in creation order); hm_plain is UNINITIALIZED (its chart is never collected), in
//     no status column (:197-199); its sets hold its rule's texts as label names (`Fixture_Part`), the other alerts'
//     are empty but for the default recipient;
//   - the groupings count the running alerts the filters keep (:106-151, :228-248): by the rule's type, component,
//     classification and recipient, which also count every rule name (`available`: :385-392, :685, so a grouping's
//     entry is there with `running` 0 when no alert of it is kept, after the entries of the kept alerts), and by the
//     chart's `_collect_module` label, never `available`: hsig.plain's "[none]", hsig.values' alertsV2Module
//     sanitized; hm_plain, whose recipient is `silent`, is `running_silent` in each;
//   - `status=` keeps the alerts of the statuses its words name (:70-99; web/api/maps/contexts_alert_statuses.c:9-38):
//     `raised` and `active` an alert at WARNING or above (ha_low), `clear` the two CLEAR ones, `uninitialized`
//     hm_plain, `critical` none; a word that is no status is no filter (`bogus`: all four), and a second `status=`
//     that has a value replaces the first (web/api/v2/api_v2_contexts.c:16-19, :50). With a status word C first
//     reads the counts of the host's last health pass, and does not walk a host that has no alert of the words'
//     statuses (database/contexts/api_v2_contexts.c:648-661): the host is still listed, for the mode has the nodes
//     (:656-658; `critical`), also under a window, through its own retention (:675-683; `critical-window`: a walk
//     that kept no alert would drop it there). The pass counts hm_plain, of a chart never collected, as
//     uninitialized: `uninitialized` finds it;
//   - `alert=` matches the alerts' names (api_v2_contexts_alerts.c:61-62): ha_mid and ha_high; a blank is no
//     separator of a web pattern (libnetdata/simple_pattern/simple_pattern.h:55-59), so `!ha_low *` is one negative
//     word, which keeps nothing (`negative`), where `!ha_low|*` keeps every other alert (`negative-or`); a lone `*`
//     and a text without a word are no pattern, and keep them all (`alert-star`, `alert-wordless`);
//   - `contexts=` filters no alert (database/contexts/api_v2_contexts.c:270: the walk's mark is not read): with a
//     pattern no context matches, the host and its four alerts (`contexts-nomatch`);
//   - `alert_instances` (api_v2_contexts_alerts.c:321-383) is there only with `instances` or `values` (:693-695); `v`
//     is the last value (:376), `tr_v` the value of the last change (:361);
//   - `options=long-json-keys` prints every key of libnetdata/json/json-keys.c's table by its long name, and
//     `options=debug` the request as C read it, pretty whatever `minify` says (database/contexts/api_v2_contexts.c:
//     1376-1443): `long-debug`;
//   - `options=mcp` (api_v2_contexts_alerts.c:434-619; database/contexts/api_v2_contexts.c:321-334, :1379,
//     :1545-1546) prints no `api` and no `timings`, the nodes
//     in their MCP form, a summary as a header and one row per name (each set as null, its one name, or a list),
//     the instances as a header and rows with the host's name, and after a list that `cardinality` cut, what was
//     cut; `options=rfc3339` prints an alert's two seconds as dates, a 0 as null (libnetdata/buffer/buffer.h:
//     1086-1096, :1119-1128).
var alertsV2AlertRows = func() []v2Req {
	module, group, recipient := alertsV2ModuleKeys, alertsV2GroupKeys, alertsV2RecipientKeys
	summary := []string{"nm", "cr", "wr", "cl", "er", "in", "nd", "cfg"}
	names := func(items ...string) alertsV2Rows {
		if items == nil {
			items = []string{}
		}
		return alertsV2Rows{member: "alerts", keys: []string{"nm"}, items: items}
	}
	instances := func(items ...string) alertsV2Rows {
		if items == nil {
			items = []string{}
		}
		return alertsV2Rows{member: "alert_instances", keys: []string{"nm", "st"}, items: items}
	}
	// the groupings by type, component and classification: hm_plain's rule alone has them
	groupings := func(running string) []alertsV2Rows {
		return []alertsV2Rows{
			{member: "alerts_by_type", keys: group, items: []string{"Parity Check " + running + " " + running + " 1"}},
			{member: "alerts_by_component", keys: group, items: []string{"Fixture Part " + running + " " + running + " 1"}},
			{member: "alerts_by_classification", keys: group, items: []string{"Workload " + running + " " + running + " 1"}},
		}
	}
	recipients := func(items ...string) alertsV2Rows {
		return alertsV2Rows{member: "alerts_by_recipient", keys: recipient, items: items}
	}
	modules := func(items ...string) alertsV2Rows {
		if items == nil {
			items = []string{}
		}
		return alertsV2Rows{member: "alerts_by_module", keys: module, items: items}
	}
	const (
		plain, low = "hm_plain UNINITIALIZED", "ha_low WARNING"
		mid, high  = "ha_mid CLEAR", "ha_high CLEAR"
		// a rule's recipient when it names none
		nobody = "root 0 0 0 0 0 3"
		silent = "silent 0 0 0 0 0 1"
		noneM  = "[none] 0 0 0 1 1"
		// the MCP form's node and its two headers
		mcpNode = `{"machine_guid":"5a1e0000-0000-4000-8000-0000000000aa","hostname":"parity-parent",` +
			`"relationship":"localhost","connected":true}`
		mcpSummary = `["Alert Name","Alert Summary","Metrics Contexts","Alert Classifications","Alert Components",` +
			`"Alert Types","Notification Recipients","# of Critical Instances","# of Warning Instances",` +
			`"# of Clear Instances","# of Error Instances","# of Instances Watched","# of Nodes Watched",` +
			`"# of Alert Configurations"]`
		mcpInstances = `["Alert Name","Hostname","Context","Instance Name","Status","Family","Info","Summary","Units",` +
			`"Last Transition ID","Last Transition Value","Last Transition Timestamp","Configuration Hash","Source",` +
			`"Recipients","Type","Component","Classification","Last Updated Value","Last Updated Timestamp"]`
		// the request as `options=debug` echoes it, from its scope on, with no selector and no window
		echo = `"scope":{"scope_nodes":null,"scope_contexts":null},"selectors":{"nodes":null,"contexts":null,` +
			`"alerts":{"status":[%s],"alert":null,"transition":null}},"filters":{"after":0,"before":0}}`
	)
	mcpRow := []string{"[0]", "[1]", "[2]{}", "[3]{}", "[4]{}", "[5]{}", "[6]{}", "[7]", "[8]", "[9]", "[10]", "[11]", "[12]", "[13]"}
	all := []alertsV2Rows{names("hm_plain", "ha_low", "ha_mid", "ha_high"), instances(plain, low, mid, high),
		recipients("silent 0 0 0 1 1 1", "root 0 1 2 3 0 3"), modules(noneM, "parity.mod 0 1 2 3 0")}
	lowOnly := []alertsV2Rows{names("ha_low"), instances(low), recipients("root 0 1 0 1 0 3", silent),
		modules("parity.mod 0 1 0 1 0")}
	// the summary of every alert, as a request without `instances` gets it
	every := []alertsV2Rows{alertsV2Nodes(alertsV2Parent), names("hm_plain", "ha_low", "ha_mid", "ha_high"),
		{member: "alert_instances"}, recipients("silent 0 0 0 1 1 1", "root 0 1 2 3 0 3"), modules(noneM, "parity.mod 0 1 2 3 0")}
	nothing := []alertsV2Rows{alertsV2Nodes(alertsV2Parent), names(), recipients(nobody, silent), modules()}
	at := func(query string) string { return alertsV2Route + "?" + query }
	return []v2Req{
		{name: "raised", target: at("options=summary,values,instances,minify&status=raised"), status: "200",
			guard: alertsV2Guard(append([]alertsV2Rows{
				{member: "alerts", keys: summary, items: []string{"ha_low 0 1 0 0 1 1 1"}},
				{member: "alert_instances", keys: []string{"nm", "st", "v", "tr_v"}, items: []string{"ha_low WARNING 70 70"}},
				recipients("root 0 1 0 1 0 3", silent), modules("parity.mod 0 1 0 1 0"),
			}, groupings("0")...)...)},
		{name: "configs", target: at("options=minify,summary"), status: "200",
			guard: alertsV2Guard(append([]alertsV2Rows{
				{member: "alerts", keys: []string{"nm", "cr", "wr", "cl", "ctx{}", "cls{}", "cp{}", "ty{}", "to{}"},
					items: []string{"hm_plain 0 0 0 {hsig.ctx} {Workload} {Fixture_Part} {Parity_Check} {silent}",
						"ha_low 0 1 0 {hsig.ctx} {} {} {} {root}", "ha_mid 0 0 1 {hsig.ctx} {} {} {} {root}",
						"ha_high 0 0 1 {hsig.ctx} {} {} {} {root}"}},
				{member: "alert_instances"},
				recipients("silent 0 0 0 1 1 1", "root 0 1 2 3 0 3"), modules(noneM, "parity.mod 0 1 2 3 0"),
			}, groupings("1")...)...)},
		{name: "by-name", target: at("options=summary,values,instances,minify&alert=ha_mid%7Cha_high"), status: "200",
			guard: alertsV2Guard(
				alertsV2Rows{member: "alerts", keys: []string{"nm", "wr", "cl"}, items: []string{"ha_mid 0 1", "ha_high 0 1"}},
				alertsV2Rows{member: "alert_instances", keys: []string{"nm", "st", "v", "tr_v"}, items: []string{"ha_mid CLEAR 70 10", "ha_high CLEAR 70 10"}},
				modules("parity.mod 0 0 2 2 0"))},
		// pretty, with the walk's `"api":2` (database/contexts/api_v2_contexts.c:1379-1380; the layout is compared too)
		{name: "v2", target: "/api/v2/alerts?options=summary,instances,values", status: "200",
			guard: dashGuard([]dashFact{dashIs("2", "api"), alertsV2Guard(
				alertsV2Rows{member: "alerts", keys: []string{"nm", "wr", "cl"},
					items: []string{"hm_plain 0 0", "ha_low 1 0", "ha_mid 0 1", "ha_high 0 1"}},
				alertsV2Rows{member: "alert_instances", keys: []string{"nm", "st", "v", "to", "tp", "cm", "cl"},
					items: []string{"hm_plain UNINITIALIZED null silent Parity Check Fixture Part Workload",
						"ha_low WARNING 70 root   ", "ha_mid CLEAR 70 root   ", "ha_high CLEAR 70 root   "}},
				modules(noneM, "parity.mod 0 1 2 3 0"))})},

		// the status words
		{name: "clear", target: at("options=summary,instances,minify&status=clear"), status: "200",
			guard: alertsV2Guard(append([]alertsV2Rows{names("ha_mid", "ha_high"), instances(mid, high),
				recipients("root 0 0 2 2 0 3", silent), modules("parity.mod 0 0 2 2 0")}, groupings("0")...)...)},
		{name: "critical", target: at("options=summary,instances,minify&status=critical"), status: "200",
			guard: alertsV2Guard(append(append(slices.Clone(nothing), instances()), groupings("0")...)...)},
		{name: "bogus", target: at("options=summary,instances,minify&status=bogus"), status: "200",
			guard: alertsV2Guard(append(slices.Clone(all), groupings("1")...)...)},
		{name: "active", target: at("options=summary,instances,minify&status=active"), status: "200",
			guard: alertsV2Guard(lowOnly...)},
		{name: "replaced", target: at("options=summary,instances,minify&status=clear&status=critical"), status: "200",
			guard: alertsV2Guard(append(slices.Clone(nothing), instances())...)},
		{name: "uninitialized", target: at("options=summary,instances,minify&status=uninitialized"), status: "200",
			guard: alertsV2Guard(append([]alertsV2Rows{alertsV2Nodes(alertsV2Parent), names("hm_plain"), instances(plain),
				recipients("silent 0 0 0 1 1 1", nobody), modules(noneM)}, groupings("1")...)...)},
		// the host prefilter under a window: the host it skips is listed
		{name: "critical-window", target: at("options=summary,minify&status=critical&after=-600"), status: "200",
			guard: alertsV2Guard(nothing...)},

		// the selectors
		{name: "contexts-nomatch", target: at("options=summary,instances,minify&contexts=nomatch*"), status: "200",
			guard: alertsV2Guard(append([]alertsV2Rows{alertsV2Nodes(alertsV2Parent)}, all...)...)},
		{name: "negative", target: at("options=summary,minify&alert=!ha_low%20*"), status: "200",
			guard: alertsV2Guard(nothing...)},
		{name: "negative-or", target: at("options=summary,minify&alert=!ha_low%7C*"), status: "200",
			guard: alertsV2Guard(alertsV2Nodes(alertsV2Parent), names("hm_plain", "ha_mid", "ha_high"),
				recipients("silent 0 0 0 1 1 1", "root 0 0 2 2 0 3"), modules(noneM, "parity.mod 0 0 2 2 0"))},
		{name: "alert-star", target: at("options=summary,minify&alert=*"), status: "200", guard: alertsV2Guard(every...)},
		{name: "alert-wordless", target: at("options=summary,minify&alert=,"), status: "200", guard: alertsV2Guard(every...)},

		// the long keys and the request's echo, pretty
		{name: "long-debug", target: at("options=summary,instances,values,long-json-keys,debug&status=warning,critical"),
			status: "200",
			guard: dashGuard([]dashFact{
				dashIs(`{"mode":["nodes","alerts"],"options":["debug","instances","values","summary","long-json-keys"],`+
					fmt.Sprintf(echo, `"warning","critical"`), "request"),
				dashKeys("machine_guid hostname nodes_array_index", "nodes", "[0]"),
				dashKeys("alerts_array_index_id nodes_array_index alert summary critical warning clear error "+
					"instances_count nodes_count configurations_count contexts classifications components types recipients",
					"alerts", "[0]"),
				dashKeys("name critical warning clear error running running_silent available", "alerts_by_recipient", "[0]"),
				dashKeys("name critical warning clear error running running_silent", "alerts_by_module", "[0]"),
				dashKeys("alerts_array_index_id nodes_array_index global_id alert context instance_id instance status "+
					"family info summary units last_transition_id last_transition_value last_transition_timestamp "+
					"config_hash_id source recipients type component classification last_updated_value "+
					"last_updated_timestamp", "alert_instances", "[0]"),
				alertsV2Guard(
					alertsV2Rows{member: "alerts", keys: []string{"alert", "warning", "instances_count", "contexts{}", "recipients{}"},
						items: []string{"ha_low 1 1 {hsig.ctx} {root}"}},
					alertsV2Rows{member: "alert_instances", keys: []string{"alert", "instance_id", "status", "global_id",
						"last_transition_value", "last_transition_timestamp", "last_updated_value", "last_updated_timestamp"},
						items: []string{"ha_low hsig.values WARNING G 70 WHEN 70 T"}}),
			})},

		// the request's echo with every selector and a window: the texts as they came, `minify` named though the
		// answer is pretty, and no `configurations` (C drops it from an alerts request first: database/contexts/
		// api_v2_contexts.c:1301-1303)
		{name: "debug-selectors", target: at("options=summary,instances,values,configurations,debug,minify&alert=ha_low" +
			"&scope_nodes=*&nodes=*&scope_contexts=hsig*&contexts=*sig*&after=-600&before=0&cardinality=3"), status: "200",
			guard: dashGuard([]dashFact{
				dashIs(`{"mode":["nodes","alerts"],"options":["minify","debug","instances","values","summary"],`+
					`"scope":{"scope_nodes":"*","scope_contexts":"hsig*"},"selectors":{"nodes":"*","contexts":"*sig*",`+
					`"alerts":{"status":[],"alert":"ha_low","transition":null}},"filters":{"after":-600,"before":0}}`, "request"),
				alertsV2Guard(lowOnly...),
			})},

		// the MCP form
		{name: "mcp-summary", target: at("options=mcp,summary&cardinality=1"), status: "200",
			guard: dashGuard([]dashFact{
				dashKeys("nodes all_alerts_header all_alerts __all_alerts_info__"),
				dashIs("["+mcpNode+"]", "nodes"), dashIs(mcpSummary, "all_alerts_header"),
				dashIs(`{"status":"truncated","total_alerts":4,"shown_alerts":1,"cardinality_limit":1}`, "__all_alerts_info__"),
				alertsV2Guard(alertsV2Rows{member: "all_alerts", keys: mcpRow,
					items: []string{"hm_plain  hsig.ctx Workload Fixture_Part Parity_Check silent 0 0 0 0 1 1 1"}}),
			})},
		{name: "mcp-instances", target: at("options=mcp,instances,values"), status: "200",
			guard: dashGuard([]dashFact{
				dashKeys("nodes alert_instances_header alert_instances"),
				dashIs("["+mcpNode+"]", "nodes"), dashIs(mcpInstances, "alert_instances_header"),
				alertsV2Guard(alertsV2Rows{member: "alert_instances",
					keys: []string{"[0]", "[1]", "[2]", "[3]", "[4]", "[10]", "[11]", "[14]", "[15]", "[18]", "[19]"},
					items: []string{"hm_plain parity-parent hsig.ctx hsig.plain UNINITIALIZED null WHEN silent Parity Check null 0",
						"ha_low parity-parent hsig.ctx hsig.values WARNING 70 WHEN root  70 T",
						"ha_mid parity-parent hsig.ctx hsig.values CLEAR 10 WHEN root  70 T",
						"ha_high parity-parent hsig.ctx hsig.values CLEAR 10 WHEN root  70 T"}}),
			})},
		// both lists cut, and the seconds as dates: hm_plain was never evaluated
		{name: "mcp-rfc3339", target: at("options=mcp,summary,instances,values,rfc3339&cardinality=2"), status: "200",
			guard: dashGuard([]dashFact{
				dashKeys("nodes all_alerts_header all_alerts __all_alerts_info__ alert_instances_header alert_instances " +
					"__alert_instances_info__"),
				dashIs(`{"status":"truncated","total_alerts":4,"shown_alerts":2,"cardinality_limit":2}`, "__all_alerts_info__"),
				dashIs(`{"status":"truncated","total_instances":4,"shown_instances":2,"cardinality_limit":2}`,
					"__alert_instances_info__"),
				alertsV2Guard(
					alertsV2Rows{member: "all_alerts", keys: []string{"[0]", "[8]"}, items: []string{"hm_plain 0", "ha_low 1"}},
					alertsV2Rows{member: "alert_instances", keys: []string{"[0]", "[4]", "[11]", "[19]"},
						items: []string{"hm_plain UNINITIALIZED WHEN" + alertsV2Dated + " null",
							"ha_low WARNING WHEN" + alertsV2Dated + " T" + alertsV2Dated}}),
			})},
		// a limit that is the count cuts nothing and says nothing
		{name: "mcp-limit", target: at("options=mcp,summary,instances,values&cardinality_limit=4"), status: "200",
			guard: dashGuard([]dashFact{
				dashKeys("nodes all_alerts_header all_alerts alert_instances_header alert_instances"),
				alertsV2Guard(
					alertsV2Rows{member: "all_alerts", keys: []string{"[0]"}, items: []string{"hm_plain", "ha_low", "ha_mid", "ha_high"}},
					alertsV2Rows{member: "alert_instances", keys: []string{"[0]", "[4]"}, items: []string{plain, low, mid, high}}),
			})},
		{name: "mcp-debug", target: at("options=mcp,debug,summary"), status: "200",
			guard: dashGuard([]dashFact{
				dashKeys("request nodes all_alerts_header all_alerts"),
				dashIs(`{"mode":["nodes","alerts"],"options":["debug","summary","mcp"],`+fmt.Sprintf(echo, ""), "request"),
				alertsV2Guard(alertsV2Rows{member: "all_alerts", keys: []string{"[0]", "[6]{}", "[9]"},
					items: []string{"hm_plain silent 0", "ha_low root 0", "ha_mid root 1", "ha_high root 1"}}),
			})},
		{name: "mcp-alone", target: at("options=mcp"), status: "200",
			guard: dashGuard([]dashFact{dashKeys("nodes"), dashIs("["+mcpNode+"]", "nodes")})},
		// the plain form's dates
		{name: "rfc3339", target: at("options=instances,values,minify,rfc3339"), status: "200",
			guard: alertsV2Guard(alertsV2Rows{member: "alert_instances", keys: []string{"nm", "tr_t", "t"},
				items: []string{"hm_plain WHEN" + alertsV2Dated + " null", "ha_low WHEN" + alertsV2Dated + " T" + alertsV2Dated,
					"ha_mid WHEN" + alertsV2Dated + " T" + alertsV2Dated, "ha_high WHEN" + alertsV2Dated + " T" + alertsV2Dated}})},
	}
}()

// alertsV2TransitionRows are the `alerts` case's rows that name a transition, each side its own (v2Req.targets): `last`
// ha_low's change to WARNING, its last transition, and `first` its first status. C looks the id up in the alert log
// (database/sqlite/sqlite_health.c:1415-1456): its entry's host replaces the request's node scope and selector, its
// context the request's two context patterns, and its alarm is the one alert the walk keeps
// (api_v2_contexts_alerts.c:733-768). So:
//   - the alert is found by any of its transitions, the last (`transition`) or an older one (`transition-first`), and
//     printed as it is now;
//   - the id is read as a UUID is anywhere: without its dashes, in upper case, and with anything after its 32 digits
//     (libnetdata/uuid/uuid.c:69-124; `-nodash`, `-upper`, `-appended`);
//   - selectors of the request's own that match nothing do not hide it (`-selectors`);
//   - the request's options still decide what is printed: without any, the nodes alone (`-bare`, pretty), with the
//     MCP form's, its one summary row (`-mcp`);
//   - a status word the alert does not have leaves no alert and, without a window, no host: the transition's context
//     is a context pattern, under which a host is listed only for an alert the walk kept (`-critical`: the prefilter
//     does not walk it, database/contexts/api_v2_contexts.c:648-661).
func alertsV2TransitionRows(last, first [2]string) []v2Req {
	found := alertsV2Guard(alertsV2Nodes(alertsV2Parent),
		alertsV2Rows{member: "alerts", keys: []string{"nm", "wr", "in"}, items: []string{"ha_low 1 1"}},
		alertsV2Rows{member: "alert_instances", keys: []string{"nm", "st", "v", "tr_v"}, items: []string{"ha_low WARNING 70 70"}},
		alertsV2Rows{member: "alerts_by_recipient", keys: alertsV2RecipientKeys, items: []string{"root 0 1 0 1 0 3", "silent 0 0 0 0 0 1"}},
		alertsV2Rows{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: []string{"parity.mod 0 1 0 1 0"}})
	row := func(name string, ids [2]string, id func(string) string, query string, guard func(Value) error) v2Req {
		return alertsV2TransitionRow(name, [2]string{id(ids[0]), id(ids[1])}, query, guard)
	}
	asIs := func(id string) string { return id }
	full := "&" + alertsV2Full
	return []v2Req{
		row("transition", last, asIs, full, found),
		row("transition-first", first, asIs, full, found),
		row("transition-nodash", last, func(id string) string { return strings.ReplaceAll(id, "-", "") }, full, found),
		row("transition-upper", last, strings.ToUpper, full, found),
		row("transition-appended", last, func(id string) string { return id + "zz" }, full, found),
		row("transition-selectors", last, asIs,
			"&scope_nodes=nothing*&nodes=nothing*&scope_contexts=nothing*&contexts=nothing*"+full, found),
		row("transition-bare", last, asIs, "", dashGuard([]dashFact{dashKeys("api nodes timings"),
			alertsV2Guard(alertsV2Nodes(alertsV2Parent))})),
		row("transition-mcp", last, asIs, "&options=mcp,summary", dashGuard([]dashFact{
			dashKeys("nodes all_alerts_header all_alerts"),
			alertsV2Guard(alertsV2Rows{member: "all_alerts", keys: []string{"[0]", "[6]{}", "[8]", "[11]"}, items: []string{"ha_low root 1 1"}})})),
		row("transition-critical", last, asIs, full+"&status=critical", alertsV2Guard(alertsV2Nodes(),
			alertsV2Rows{member: "alerts", keys: []string{"nm"}, items: []string{}},
			alertsV2Rows{member: "alert_instances", keys: []string{"nm"}, items: []string{}})),
	}
}

// alertsV2Raw is a request whose answers are compared byte for byte after maskAnswer, sent as written (request: the
// bytes), with what the oracle's answer must be first: its start, the parts it must hold and the parts it must not.
type alertsV2Raw struct {
	name         string
	request      []byte
	start        string
	holds, lacks []string
}

// alertsV2RawJudge holds when a masked answer is what the row wants: it starts with `start`, holds each of holds and
// none of lacks, and ends with its head: no body, and no chunk. The judge is its own, and
// not exactJudge: it is asked without a test (the pins ask it), and a row says what an answer must not hold.
func alertsV2RawJudge(row alertsV2Raw, answer []byte) error {
	a := string(answer)
	if !strings.HasPrefix(a, row.start) {
		return fmt.Errorf("want %q at its start", row.start)
	}
	if head := strings.Index(a, "\r\n\r\n"); head < 0 || head != len(a)-4 {
		return fmt.Errorf("it does not end with its head: a body or a chunk follows, or the head has no end")
	}
	for _, part := range row.holds {
		if !strings.Contains(a, part) {
			return fmt.Errorf("it does not hold %q", part)
		}
	}
	for _, part := range row.lacks {
		if strings.Contains(a, part) {
			return fmt.Errorf("it holds %q", part)
		}
	}
	return nil
}

// alertsV2RawRound sends the row's bytes to both agents, each read until it closes the connection (or stays silent
// for five seconds: `<timeout>`), and lists what is wrong: an agent that cannot be asked, an answer of the oracle's
// that is not what the row wants (alertsV2RawJudge; it starts "oracle:"), or two answers that differ once each
// side's clock and expiry are read against its own flight (maskAnswer).
func alertsV2RawRound(p *Pair, row alertsV2Raw) []string {
	var got [2][]byte
	for i, side := range p.Each() {
		from := time.Now().Unix()
		b, err := rawExchangeFrom("", side.Daemon.Addr, row.request, 5*time.Second)
		if err != nil {
			return []string{fmt.Sprintf("%s: %v", side.Role, err)}
		}
		got[i] = maskAnswer(b, [2]int64{from, time.Now().Unix()})
	}
	if err := alertsV2RawJudge(row, got[0]); err != nil {
		return []string{fmt.Sprintf("oracle: answered %q: %v", truncateBytes(got[0]), err)}
	}
	if !bytes.Equal(got[0], got[1]) {
		return []string{fmt.Sprintf("answers differ\n%s\noracle:    %q\ncandidate: %q", firstDifference(got[0], got[1]),
			truncateBytes(got[0]), truncateBytes(got[1]))}
	}
	return nil
}

// alertsV2NotFoundRows are the `alerts` case's requests for a transition no entry of the alert log has: an id that
// is none's, and a text that is no UUID. C leaves before it starts the answer's JSON (database/contexts/
// api_v2_contexts.c:1361-1366), so the 404 has an empty body of the buffer's first type, text/plain, without a
// length, which turns keep-alive off (web/server/web_client.c:1052-1060), and nothing is sent after its head
// (:1682-1686); to a client that takes gzip C announces a chunked gzip body (:1047-1051) and sends no chunk. Either
// way the client then waits neither to send nor to receive, and is closed (web/server/static/static-threaded.c:
// 86-93, :300; libnetdata/socket/poll-events.c:228-229), whatever the request asked (`-gzip`, on a
// connection the request asked to keep: an agent that kept it open would have its answer end `<timeout>`, not with
// the head's last line).
var alertsV2NotFoundRows = func() []alertsV2Raw {
	const unknown = alertsV2Route + "?transition=11111111-2222-4333-8444-555555555555"
	const start = "HTTP/1.1 404 Not Found\r\n"
	plain := []string{"\r\nContent-Type: text/plain; charset=utf-8\r\n", "\r\nCache-Control: no-cache, no-store, must-revalidate\r\n",
		"\r\nExpires: Date+0\r\n"}
	closed := []string{"Host: localhost", "Connection: close"}
	return []alertsV2Raw{
		{name: "transition-unknown", request: rawRequest("GET", unknown, closed, nil), start: start,
			holds: append([]string{"\r\nConnection: close\r\n"}, plain...),
			lacks: []string{"Content-Length", "Content-Encoding", "Transfer-Encoding"}},
		{name: "transition-text", request: rawRequest("GET", alertsV2Route+"?transition=x", closed, nil), start: start,
			holds: append([]string{"\r\nConnection: close\r\n"}, plain...),
			lacks: []string{"Content-Length", "Content-Encoding", "Transfer-Encoding"}},
		{name: "transition-gzip", request: rawRequest("GET", unknown, []string{"Host: localhost", "Connection: keep-alive",
			"Accept-Encoding: gzip"}, nil), start: start,
			holds: append([]string{"\r\nConnection: keep-alive\r\n", "\r\nContent-Encoding: gzip\r\n",
				"\r\nTransfer-Encoding: chunked\r\n"}, plain...),
			lacks: []string{"Content-Length"}},
	}
}()

// alertsV2SetsConf are the `sets` case's rules, on three charts of two contexts (alertsV2SetsScenario):
//   - hs_tpl, a template on the context of the two charts never collected, with a summary and an info that name
//     the chart's family: one rule, two alerts;
//   - hs_two, twice: an alarm on a chart of each context, each with its own class, type, component and recipient
//     (one `silent`, one of two words), the second with a summary: one name, two rules, two contexts;
//   - hs_sum on the collected chart, its summary and info naming the family and a chart label.
const alertsV2SetsConf = `template: hs_tpl
      on: hset.ctx.a
    calc: $b
   every: 1s
    warn: $this > 1000
   units: things
    info: the template of ${family}
 summary: tpl ${family}

   alarm: hs_two
      on: hset.one
    calc: $b
   every: 1s
    warn: $this > 1000
   units: things
   class: Errors
    type: Type One
component: Part One
      to: silent
    info: two on one

   alarm: hs_two
      on: hset.values
    calc: $a
   every: 1s
    warn: $this > 50
   units: things
   class: Latency
    type: Type Two
component: Part Two
      to: sysadmin webmaster
    info: two on values
 summary: two ${family}

   alarm: hs_sum
      on: hset.values
    calc: $a
   every: 1s
    warn: $this > 500
   units: things
    info: info of ${family} at ${label:kind}
 summary: sum of ${family} at ${label:kind}
`

// alertsV2LongModule is the module of the `sets` case's chart hset.long: 126 bytes, then a character of two bytes.
// C reads a chart's module for the grouping into 128 bytes (api_v2_contexts_alerts.c:141-142): 127 of them, the
// last the character's first byte alone.
var alertsV2LongModule = strings.Repeat("m", 126) + "\u00e9zz"

// alertsV2SetsScenario is the `sets` case's plugin: hset.one and hset.long of the context hset.ctx.a, never collected
// (the first without a module, the second with alertsV2LongModule), then the collected hset.values of hset.ctx.b,
// with a module, a family and a label, which the plugin commits (plugin.Values.Labels). C gives every chart the
// label `_collect_module` (database/rrdset-index-id.c:23-28: "[none]" for a chart without a module) and keeps it
// over a commit that does not name it (database/rrdlabels.h:16, :21; database/rrdlabels.c:600-601), so the
// grouping's "[unset]", C's name for a chart without the label or with an empty one (api_v2_contexts_alerts.c:
// 143-144), is not something a live chart shows: no row has it.
func alertsV2SetsScenario() *plugin.Scenario {
	emit := "CHART hset.one '' 'title' 'units' 'fam one' 'hset.ctx.a' line 1000 1 '' '' ''\n" +
		"DIMENSION b '' absolute 1 1\n" +
		"CHART hset.long '' 'title' 'units' 'fam long' 'hset.ctx.a' line 1000 1 '' '' '" + alertsV2LongModule + "'\n" +
		"DIMENSION b '' absolute 1 1\n"
	sc := healthScenario(emit, "hset.values", "hset.ctx.b", []string{"a"}, map[string]int64{"a": 10}, map[string]int64{"a": 70})
	c := healthChart(sc)
	c.Module, c.Family, c.Labels = "sets,mod", "fam b", []string{"kind x y"}
	return sc
}

// alertsV2PlaySets plays `sets`: hset.values at 70 (its hs_two WARNING, hs_sum CLEAR; the alerts of the two other
// charts stay UNINITIALIZED), the rows of that state, then a child (the health checks' own, its health on by the
// case's stream.conf section) with one chart of the template's context, and the rows of two hosts. The guards read
// what C answers:
//   - a summary entry is its name's alerts on every chart and host (api_v2_contexts_alerts.c:255-299): `in` counts
//     them, `nd` their hosts, `cfg` their rules (hs_tpl: two alerts of one rule; hs_two: two alerts of two rules),
//     and its five sets hold each alert's context and its rule's class, component, type and recipient as label
//     names, each once, in an order that is not the answer's (alertsV2Sets);
//   - its `sum` is the summary of the rule of the first alert met, as written (:260): hs_two's first rule has none.
//     An instance's `sum` is its own alert's, its variables replaced by its chart's family and label
//     (health/rrdcalc.c:183-262), and its `info` its rule's as written (api_v2_contexts_alerts.c:714);
//   - a rule name is `available` once, by its first rule (health/health_prototypes.c:247-259, :743-749): hs_two's
//     second rule's type, component, class and recipient have a running alert and no `available`;
//   - the grouping by module prints the 127 bytes C read of a module (alertsV2LongModule; the render writes out the
//     byte that is half a character, alertsV2Cut), and hset.values' module after its label commit;
//   - `scope_contexts=` as a context's exact id walks that context alone (database/contexts/query_scope.c:96-110),
//     also beside a `transition=` whose alert is on another context: the lookup replaces the scope's pattern, not
//     the request's text, so the host has no alert there and is not listed (`scope-other`); a text that is no
//     context's id is a pattern, which the transition's context replaces (`scope-pattern`);
//   - with the child, hs_tpl is on two hosts: `nd` 2, `ni` both hosts' indexes in the order they were walked, and
//     the child's instance carries the index its host gets (`two-nodes`); a transition of the child's alert narrows
//     the answer to the child (`child-transition`);
//   - once the child left, its alert is gone and its host stays, with the counts of its last health pass (one CLEAR
//     alert): C publishes a host's counts as a pass ends and never takes them back (health/health_event_loop.c:
//     93-115, :877-880), so a status word still decides whether the host is walked (database/contexts/
//     api_v2_contexts.c:648-661). Under a window, a word its last pass had no alert of leaves it listed through its
//     retention (`gone-critical-window`; `gone-uninitialized-window`), and a word it had one of has it walked, found
//     without an alert, and not listed (`gone-clear-window`). That last row stands on the counts of a pass that saw
//     the alert. By reading, C has one interleaving in which the child's last pass counts nothing: HEALTH passes its
//     test of the host (health/health_event_loop.c:389), the receiver then clears the host's online flag and
//     deletes its alerts (streaming/stream-receiver.c:1467, :1483; health/rrdcalc.c:764-770), and the pass finds no
//     alert and publishes zeros (health/health_event_loop.c:877-880). The agent it happens to then lists
//     health-child in `gone-clear-window`, for the rest of the run: a failure of that row at the oracle's guard
//     with the child listed, or as a difference at `nodes` with the child on one side, may be this race (not seen
//     in any run here) and is then no verdict on the candidate: run the case again. The window is also each
//     context's: the context of the two charts never collected has no data in it, so its UNINITIALIZED alerts are
//     not met, and localhost, which the prefilter let through for them, is not listed
//     (`gone-uninitialized-window`); without a status word only the collected chart's alerts are (`gone-window`:
//     database/contexts/api_v2_contexts.c:275-276).
func alertsV2PlaySets(t *testing.T, h *healthPair) {
	h.create(t)
	h.waitOracle(t, "the chart's alerts", func() (string, error) {
		v := h.get(0, "/api/v1/alarms?all")
		return v, healthAll("CLEAR", "hs_two", "hs_sum")(v)
	})
	h.release(t, "p1", 1, healthCalcHold)
	alarms := func(i int) string { return h.get(i, "/api/v1/alarms?all") }
	h.compareNow(t, "/api/v1/alarms?all", alarms, healthWant(map[string]string{"hs_two": "WARNING", "hs_sum": "CLEAR"}))
	h.compareNow(t, "the alert log's transitions", func(i int) string { return h.transitions(i, "") },
		alertsV2LogHolds("hs_two: CLEAR->WARNING 70 things"))
	fam := alertsV2Family(h.n, alertsV2LogReader(h))
	for _, req := range alertsV2SetsRows() {
		compareV2(t, h.p, req, fam)
	}
	// hs_sum's first status, on the collected chart's context
	sum := alertsV2TransitionOf(t, h, h.n, "", "hs_sum", "UNINITIALIZED", "CLEAR")
	for _, req := range alertsV2SetsScopeRows(sum) {
		compareV2(t, h.p, req, fam)
	}

	// The child: its connection makes localhost a parent, which links localhost's alerts again (healthParentRelink),
	// so it connects healthUnlinkHold after their last status, and the rows wait for hs_two's status after the link.
	time.Sleep(healthUnlinkHold)
	v := h.childViews(t)
	child := startFanoutChild(t, h.p, healthChild)
	time.Sleep(2 * time.Second)
	v.settle(child.define("hsetc.values", "hset.ctx.a", []string{"b"}, map[string]int64{"b": 10}))
	h.compareNow(t, "the child's /api/v1/alarms?all", v.all, healthAll("CLEAR", "hs_tpl"))
	h.compareNow(t, "/api/v1/alarms?all with the child", alarms, healthWant(map[string]string{"hs_two": "WARNING", "hs_sum": "CLEAR"}))
	h.compareNow(t, "the alert log's transitions with the child", func(i int) string { return h.transitions(i, "") },
		healthLastChange("hs_two", "UNINITIALIZED->WARNING"))
	both := alertsV2Family(h.n, alertsV2LogReader(h),
		alertsV2Host{name: healthChild.Hostname, n: v.cn, log: alertsV2LogReaderOf(h, healthChildBase)})
	// the child's alert's first status
	theirs := alertsV2TransitionOf(t, h, v.cn, healthChildBase, "hs_tpl", "UNINITIALIZED", "CLEAR")
	for _, req := range alertsV2SetsChildRows(theirs) {
		compareV2(t, h.p, req, both)
	}
	// the child leaves healthUnlinkHold after its alert's status, as the health checks' children do; the rows after it
	// wait for its alert's last entry
	time.Sleep(healthUnlinkHold)
	child.disconnect(t)
	h.compareNow(t, "the child's alert log after it left", v.changes, healthLastChange("hs_tpl", "CLEAR->REMOVED"))
	for _, req := range alertsV2SetsGoneRows() {
		compareV2(t, h.p, req, both)
	}
}

// alertsV2Cut is alertsV2LongModule as the grouping by module prints it and the render writes it (alertsV2Bytes): its
// first 127 bytes, the last one, half a character, written out.
var alertsV2Cut = strings.Repeat("m", 126) + string(alertsV2Marker) + "c3"

// alertsV2SetKeys are a summary entry's five sets as a guard reads them (alertsV2Rows: each a set).
var alertsV2SetKeys = []string{"ctx{}", "cls{}", "cp{}", "ty{}", "to{}"}

// alertsV2TransitionRow is a row that names a transition, each side its own (ids), in the query after it.
func alertsV2TransitionRow(name string, ids [2]string, query string, guard func(Value) error) v2Req {
	target := func(id string) string { return alertsV2Route + "?transition=" + id + query }
	return v2Req{name: name, target: target("<the side's own id>"), targets: [2]string{target(ids[0]), target(ids[1])},
		status: "200", guard: guard}
}

// alertsV2SetsRows are the `sets` case's rows before its child connects (alertsV2PlaySets has what C answers).
func alertsV2SetsRows() []v2Req {
	at := func(query string) string { return alertsV2Route + "?" + query }
	const (
		two    = "{hset.ctx.a,hset.ctx.b} {Errors,Latency} {Part_One,Part_Two} {Type_One,Type_Two} {silent,sysadmin_webmaster}"
		tplSum = "tpl ${family}"
		sumSum = "sum of ${family} at ${label:kind}"
	)
	groupings := []alertsV2Rows{
		{member: "alerts_by_type", keys: alertsV2GroupKeys, items: []string{"Type One 1 1 1", "Type Two 1 0 -"}},
		{member: "alerts_by_component", keys: alertsV2GroupKeys, items: []string{"Part One 1 1 1", "Part Two 1 0 -"}},
		{member: "alerts_by_classification", keys: alertsV2GroupKeys, items: []string{"Errors 1 1 1", "Latency 1 0 -"}},
		{member: "alerts_by_recipient", keys: alertsV2GroupKeys, items: []string{"root 3 0 2", "silent 1 1 1",
			"sysadmin webmaster 1 0 -"}},
		{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: []string{"[none] 0 0 0 2 1", alertsV2Cut + " 0 0 0 1 0",
			"sets.mod 0 1 1 2 0"}},
	}
	return []v2Req{
		{name: "summary", target: at("options=summary,minify"), status: "200",
			guard: alertsV2Guard(append([]alertsV2Rows{alertsV2Nodes(alertsV2Parent),
				{member: "alerts", keys: append([]string{"nm", "sum", "in", "nd", "cfg"}, alertsV2SetKeys...),
					items: []string{"hs_tpl " + tplSum + " 2 1 1 {hset.ctx.a} {} {} {} {root}", "hs_two  2 1 2 " + two,
						"hs_sum " + sumSum + " 1 1 1 {hset.ctx.b} {} {} {} {root}"}}}, groupings...)...)},
		{name: "full", target: at(alertsV2Full), status: "200",
			guard: alertsV2Guard(alertsV2Rows{member: "alert_instances", keys: []string{"ati", "ni", "nm", "ch", "st", "sum", "info"},
				items: []string{"0 0 hs_tpl hset.one UNINITIALIZED tpl fam one the template of ${family}",
					"1 0 hs_two hset.one UNINITIALIZED  two on one",
					"0 0 hs_tpl hset.long UNINITIALIZED tpl fam long the template of ${family}",
					"1 0 hs_two hset.values WARNING two fam b two on values",
					"2 0 hs_sum hset.values CLEAR sum of fam b at x y info of ${family} at ${label:kind}"}})},
		{name: "mcp-summary", target: at("options=mcp,summary,minify"), status: "200",
			guard: alertsV2Guard(alertsV2Rows{member: "all_alerts",
				keys: []string{"[0]", "[1]", "[2]{}", "[3]{}", "[4]{}", "[5]{}", "[6]{}", "[11]", "[12]", "[13]"},
				items: []string{"hs_tpl " + tplSum + " hset.ctx.a null null null root 2 1 1", "hs_two  " + two + " 2 1 2",
					"hs_sum " + sumSum + " hset.ctx.b null null null root 1 1 1"}})},
		// the context of the two charts never collected, by its id
		{name: "scope", target: at("options=summary,instances,minify&scope_contexts=hset.ctx.a"), status: "200",
			guard: alertsV2Guard(alertsV2Nodes(alertsV2Parent),
				alertsV2Rows{member: "alerts", keys: append([]string{"nm", "in", "nd", "cfg"}, alertsV2SetKeys...),
					items: []string{"hs_tpl 2 1 1 {hset.ctx.a} {} {} {} {root}",
						"hs_two 1 1 1 {hset.ctx.a} {Errors} {Part_One} {Type_One} {silent}"}},
				alertsV2Rows{member: "alerts_by_module", keys: alertsV2ModuleKeys,
					items: []string{"[none] 0 0 0 2 1", alertsV2Cut + " 0 0 0 1 0"}})},
	}
}

// alertsV2SetsScopeRows are the `sets` case's rows of a transition of hs_sum (ids: each side's) beside a
// `scope_contexts` of the request's own.
func alertsV2SetsScopeRows(ids [2]string) []v2Req {
	none := []string{}
	return []v2Req{
		alertsV2TransitionRow("scope-other", ids, "&scope_contexts=hset.ctx.a&options=summary,instances,minify",
			alertsV2Guard(alertsV2Nodes(),
				alertsV2Rows{member: "alerts", keys: []string{"nm"}, items: none},
				alertsV2Rows{member: "alert_instances", keys: []string{"nm"}, items: none})),
		alertsV2TransitionRow("scope-pattern", ids, "&scope_contexts=hset.ctx.*&options=summary,instances,minify",
			alertsV2Guard(alertsV2Nodes(alertsV2Parent),
				alertsV2Rows{member: "alerts", keys: []string{"nm", "in"}, items: []string{"hs_sum 1"}},
				alertsV2Rows{member: "alert_instances", keys: []string{"nm", "ctx", "st"}, items: []string{"hs_sum hset.ctx.b CLEAR"}})),
	}
}

// alertsV2SetsChildRows are the `sets` case's rows of two hosts; ids are each side's id of the first status of the
// child's hs_tpl.
func alertsV2SetsChildRows(ids [2]string) []v2Req {
	return []v2Req{
		{name: "two-nodes", target: alertsV2Route + "?" + alertsV2Full + "&alert=hs_tpl", status: "200",
			guard: alertsV2Guard(alertsV2Nodes(alertsV2Parent, alertsV2Child),
				alertsV2Rows{member: "alerts", keys: []string{"nm", "ni", "cl", "in", "nd", "cfg"}, items: []string{"hs_tpl [0,1] 1 3 2 1"}},
				alertsV2Rows{member: "alert_instances", keys: []string{"ni", "nm", "ch", "st", "sum"},
					items: []string{"0 hs_tpl hset.one UNINITIALIZED tpl fam one", "0 hs_tpl hset.long UNINITIALIZED tpl fam long",
						"1 hs_tpl hsetc.values CLEAR tpl family"}},
				alertsV2Rows{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: []string{"[none] 0 0 0 1 0",
					alertsV2Cut + " 0 0 0 1 0", "fanout 0 0 1 1 0"}})},
		alertsV2TransitionRow("child-transition", ids, "&options=summary,instances,minify", alertsV2Guard(
			alertsV2Rows{member: "nodes", keys: []string{"nm", "ni"}, items: []string{"health-child 0"}},
			alertsV2Rows{member: "alerts", keys: []string{"nm", "ni", "in", "nd"}, items: []string{"hs_tpl [0] 1 1"}},
			alertsV2Rows{member: "alert_instances", keys: []string{"ni", "nm", "ch", "st"}, items: []string{"0 hs_tpl hsetc.values CLEAR"}})),
	}
}

// alertsV2SetsGoneRows are the `sets` case's rows after its child left, each under a window of the last ten minutes.
func alertsV2SetsGoneRows() []v2Req {
	at := func(query string) string { return alertsV2Route + "?" + query }
	none := []string{}
	return []v2Req{
		{name: "gone-critical-window", target: at("options=summary,minify&status=critical&after=-600"), status: "200",
			guard: alertsV2Guard(alertsV2Nodes(alertsV2Parent, alertsV2Child),
				alertsV2Rows{member: "alerts", keys: []string{"nm"}, items: none},
				alertsV2Rows{member: "alerts_by_recipient", keys: alertsV2GroupKeys, items: []string{"root 0 0 2", "silent 0 0 1"}},
				alertsV2Rows{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: none})},
		{name: "gone-clear-window", target: at("options=summary,instances,minify&status=clear&after=-600"), status: "200",
			guard: alertsV2Guard(alertsV2Nodes(alertsV2Parent),
				alertsV2Rows{member: "alerts", keys: []string{"nm", "ni", "in", "nd"}, items: []string{"hs_sum [0] 1 1"}},
				alertsV2Rows{member: "alert_instances", keys: []string{"ni", "nm", "ctx", "st"}, items: []string{"0 hs_sum hset.ctx.b CLEAR"}},
				alertsV2Rows{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: []string{"sets.mod 0 0 1 1 0"}})},
		{name: "gone-uninitialized-window", target: at("options=summary,instances,minify&status=uninitialized&after=-600"),
			status: "200",
			guard: alertsV2Guard(
				alertsV2Rows{member: "nodes", keys: []string{"nm", "ni"}, items: []string{"health-child 0"}},
				alertsV2Rows{member: "alerts", keys: []string{"nm"}, items: none},
				alertsV2Rows{member: "alert_instances", keys: []string{"nm"}, items: none})},
		{name: "gone-window", target: at("options=summary,minify&after=-600"), status: "200",
			guard: alertsV2Guard(alertsV2Nodes(alertsV2Parent),
				alertsV2Rows{member: "alerts", keys: []string{"nm", "wr", "cl", "in", "cfg", "ctx{}"},
					items: []string{"hs_two 1 0 1 1 {hset.ctx.b}", "hs_sum 0 1 1 1 {hset.ctx.b}"}},
				alertsV2Rows{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: []string{"sets.mod 0 1 1 2 0"}})},
	}
}

// alertsV2PlayOff plays `off`: health off, and one chart collected each second. C keeps no count of a host whose
// health never ran, so a status word skips no host (database/contexts/api_v2_contexts.c:578-597, :648-654): every
// host is walked, and one without a kept alert is listed only while the request has no context pattern and no window
// (:656-658, :672-673). The guards read what C answers:
//   - with a window the host is not listed, with a status word (`raised-window`: an agent that read "no count yet"
//     as "no alert of the status" would take the prefilter's path and list it through its retention, :675-683) and
//     without one (`window`);
//   - without a window it is (`raised`).
//
// The fixture is judged first, on both sides: the nodes answer of the same window lists the host, so its retention
// meets the window (C drops a host outside the window before anything else, :636). The family is the zero pair's: a
// host without health has no alert, and no alert log to read.
func alertsV2PlayOff(t *testing.T, h *healthPair) {
	h.createChart(t)
	// the nodes answer of the same window lists the host: on the oracle, or the rows judge nothing; on the candidate,
	// or the rows cannot tell an agent that lists it by its retention from one that has none yet
	listed := func(d *daemon.Daemon) error {
		r := healthGet(d, "/api/v3/nodes?after=-60&options=minify")
		if r.Status != http.StatusOK || !bytes.Contains(r.Body, []byte(`"nm":"`+parentIdentity.Hostname+`"`)) {
			return fmt.Errorf("/api/v3/nodes?after=-60 answered %d without the host: %s", r.Status, truncateBytes(r.Body))
		}
		return nil
	}
	h.waitOracle(t, "the host's retention meets the last minute", func() (string, error) { return "", listed(h.p.Oracle) })
	var late error
	for end := time.Now().Add(healthCandidateWait); ; time.Sleep(250 * time.Millisecond) {
		if late = listed(h.p.Candidate); late == nil || time.Now().After(end) {
			break
		}
	}
	if late != nil {
		t.Errorf("candidate: the host's retention does not meet the last minute, so the rows judge no prefilter: %v", late)
	}
	fam := alertsV2Family([2]*healthNorm{}, nil)
	for _, req := range alertsV2OffRows() {
		compareV2(t, h.p, req, fam)
	}
}

// alertsV2OffRows are the `off` case's rows (alertsV2PlayOff has what C answers): nothing but, without a window, the
// host.
func alertsV2OffRows() []v2Req {
	empty := func(nodes alertsV2Rows) func(Value) error {
		rows := []alertsV2Rows{nodes}
		for _, member := range []string{"alerts", "alerts_by_type", "alerts_by_component", "alerts_by_classification",
			"alerts_by_recipient", "alerts_by_module"} {
			rows = append(rows, alertsV2Rows{member: member, keys: []string{"name"}, items: []string{}})
		}
		return alertsV2Guard(rows...)
	}
	return []v2Req{
		{name: "raised-window", target: alertsV2Route + "?options=summary,minify&status=raised&after=-60", status: "200",
			guard: empty(alertsV2Nodes())},
		{name: "window", target: alertsV2Route + "?options=summary,minify&after=-60", status: "200", guard: empty(alertsV2Nodes())},
		{name: "raised", target: alertsV2Route + "?options=summary,minify&status=raised", status: "200",
			guard: empty(alertsV2Nodes(alertsV2Parent))},
	}
}

// alertsV2TwoHosts are `health.child` `two-hosts`' alerts rows (D234 F3), asked while each host's alert is WARNING:
// localhost's alarm hloc_calc and the child's template hch_calc (cn: each side's normalizer of the child's ids). C
// lists a host after it walked its contexts (database/contexts/api_v2_contexts.c:702-709), so an instance carries
// the index its host is about to get (api_v2_contexts_alerts.c:718) and a summary entry the indexes of its hosts
// (:637-644): localhost 0, the child 1. The child's chart has its plugin's module, localhost's none. With
// `options=mcp` a row names its host (:563) and the nodes say which is the child (api_v2_contexts.c:321-334).
func alertsV2TwoHosts(t *testing.T, h *healthPair, cn [2]*healthNorm) {
	t.Helper()
	fam := alertsV2Family(h.n, alertsV2LogReader(h),
		alertsV2Host{name: healthChild.Hostname, n: cn, log: alertsV2LogReaderOf(h, healthChildBase)})
	for _, req := range alertsV2TwoHostsRows() {
		compareV2(t, h.p, req, fam)
	}
}

// alertsV2TwoHostsRows are alertsV2TwoHosts' rows.
func alertsV2TwoHostsRows() []v2Req {
	return []v2Req{
		{name: "alerts", target: alertsV2Route + "?" + alertsV2Full, status: "200",
			guard: alertsV2Guard(alertsV2Nodes(alertsV2Parent, alertsV2Child),
				alertsV2Rows{member: "alerts", keys: []string{"ati", "ni", "nm", "wr", "in", "nd", "cfg", "ctx{}"},
					items: []string{"0 [0] hloc_calc 1 1 1 1 {hloc.ctx}", "1 [1] hch_calc 1 1 1 1 {hchild.ctx}"}},
				alertsV2Rows{member: "alert_instances", keys: []string{"ati", "ni", "nm", "ch", "st", "v"},
					items: []string{"0 0 hloc_calc hloc.values WARNING 70", "1 1 hch_calc hchild.values WARNING 70"}},
				alertsV2Rows{member: "alerts_by_recipient", keys: alertsV2RecipientKeys, items: []string{"root 0 2 0 2 0 2"}},
				alertsV2Rows{member: "alerts_by_module", keys: alertsV2ModuleKeys, items: []string{"[none] 0 1 0 1 0", "fanout 0 1 0 1 0"}})},
		{name: "alerts-mcp", target: alertsV2Route + "?options=mcp,instances,values", status: "200",
			guard: alertsV2Guard(
				alertsV2Rows{member: "nodes", keys: []string{"hostname", "relationship", "connected"},
					items: []string{"parity-parent localhost true", "health-child child true"}},
				alertsV2Rows{member: "alert_instances", keys: []string{"[0]", "[1]", "[3]", "[4]", "[11]", "[19]"},
					items: []string{"hloc_calc parity-parent hloc.values WARNING WHEN T", "hch_calc health-child hchild.values WARNING WHEN T"}})},
	}
}

// alertsV2OneClear is the `one-clear` row's guard: the one transition is hs_calc's first status, from UNINITIALIZED
// to CLEAR at the first value, its notification never run (`when` 0), and it is the one row evaluated, matched and
// returned.
var alertsV2OneClear = alertsV2Guard(
	alertsV2Rows{member: "transitions", keys: append(slices.Clone(alertsV2TransitionKeys), "notification.when"),
		items: []string{"hs_calc UNINITIALIZED CLEAR 10 0"}},
	alertsV2Rows{member: "items", keys: []string{"evaluated", "matched", "returned", "max_to_return", "before", "after"},
		items: []string{"1 1 1 1 0 0"}})

// alertsV2LogHolds is a guard on a transitions view (healthPair.transitions): it holds each of the lines.
func alertsV2LogHolds(lines ...string) func(string) error {
	return func(view string) error {
		got := strings.Split(view, "\n")
		for _, l := range lines {
			if !slices.Contains(got, l) {
				return fmt.Errorf("no %q", l)
			}
		}
		return nil
	}
}

// the transitions health.transitions' alerts make with a status above CLEAR on a side (sqlite_health.c:1474), as
// alertsV2Guard reads them: hs_calc and hs_max step with the value, hs_avg with its aligned window's average (85 is
// WARNING, 44 CLEAR: healthGridSecond)
var (
	alertsV2TransitionKeys = []string{"alert", "old.status", "new.status", "new.value"}
	alertsV2Newest         = []string{"hs_avg CRITICAL CLEAR 44", "hs_avg WARNING CRITICAL 95", "hs_calc CRITICAL CLEAR 10",
		"hs_max CRITICAL CLEAR 10"}
	alertsV2Transitions = append([]string{"hs_avg CLEAR WARNING 70", "hs_calc CLEAR WARNING 70", "hs_calc WARNING CRITICAL 95",
		"hs_max CLEAR WARNING 70", "hs_max WARNING CRITICAL 95"}, alertsV2Newest...)
)

// alertsV2PlayTransitions plays `transitions`: health.transitions' phases (each switched at the third second of
// hs_avg's window, and compared through /api/v1/alarms?all), the alert log's transitions (the green anchor), then the
// v2 rows. Their guards read what C answers:
//   - the window's rows are the transitions with a status above CLEAR on a side (sqlite_health.c:1474: three per
//     alert, the links' entries left out), newest first (:1558; api_v2_contexts_alert_transitions.c:187-255 keeps
//     that order);
//   - `items` counts them (api_v2_contexts_alert_transitions.c:495-515): `evaluated` the rows of the window, `matched`
//     those the facets keep, `before` those at or before `anchor_gi` (:190-194), `after` those past `last` (:215-219,
//     :241-254), `returned` the rest;
//   - with `transition=` the one row of that id, whatever its status (sqlite_health.c:1476-1479, :1502-1512: the
//     direct statement has no status clause, so hs_calc's first status, UNINITIALIZED to CLEAR, answers too, where the
//     window leaves it out), and `last` 1 when the request has none (web/api/v2/api_v2_contexts.c:70-71).
//
// The phases are health.transitions' (healthPlayPhases), played here to keep the second of the second switch.
func alertsV2PlayTransitions(t *testing.T, h *healthPair) {
	names := []string{"hs_avg", "hs_calc", "hs_max"}
	var switched int64
	h.create(t)
	for k := range healthSigPhases {
		if k > 0 {
			if second := h.release(t, fmt.Sprintf("p%d", k), k, healthSigHold); k == 2 {
				switched = second
			}
		}
		h.compareNow(t, fmt.Sprintf("phase %d: /api/v1/alarms?all", k), func(i int) string { return h.get(i, "/api/v1/alarms?all") },
			healthAll(healthSigStatus[k], names...))
	}
	// the green anchor, as health.transitions compares it; it also names each side's transition ids
	h.compareNow(t, "the alert log's transitions", func(i int) string { return h.transitions(i, "") },
		alertsV2LogHolds("hs_calc: CLEAR->WARNING 70 things", "hs_avg: CRITICAL->CLEAR 44 things"))
	// one transition by its id, each side's own: hs_calc's change to WARNING and its first status
	var raised, first [2]string
	var newest int64
	for i, side := range h.p.Each() {
		entries, err := h.entriesAs(h.n[i], i, "/api/v1/alarm_log")
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		for _, e := range entries {
			if e.Name == "hs_calc" && e.OldStatus == "CLEAR" && e.Status == "WARNING" {
				raised[i] = e.Tid
			}
			if e.Name == "hs_calc" && e.OldStatus == "UNINITIALIZED" && e.Status == "CLEAR" {
				first[i] = e.Tid
			}
			if i == 0 {
				newest = max(newest, e.When)
			}
		}
		for what, id := range map[string]string{"from CLEAR to WARNING": raised[i], "from UNINITIALIZED to CLEAR": first[i]} {
			if id != "" {
				continue
			}
			if side.Role == Oracle {
				t.Fatalf("oracle: its alert log has no change of hs_calc %s", what)
			}
			// the row is still asked, without an id: it shows what the candidate answers then
			t.Errorf("candidate: its alert log has no change of hs_calc %s", what)
		}
	}
	// a relative window ends a second before the request's (libnetdata/libnetdata.c:550-551), at that second's first
	// microsecond (sqlite_health.c:1474, :1566-1567): the oracle's newest change (made in its second `when`, its global
	// id later in that second) is in the window from `when`+2 on; one more second for a side a second behind (seen C
	// against C, 2026-10-06 20:23Z: asked at `when`+1, both sides left the two newest out)
	time.Sleep(time.Until(time.Unix(newest+3, 0)))
	items := []string{"evaluated", "matched", "returned", "max_to_return", "before", "after"}
	one := "/api/v3/alert_transitions?options=minify&transition="
	fam := alertsV2Family(h.n, alertsV2LogReader(h))
	for _, req := range []v2Req{
		{name: "window", target: "/api/v2/alert_transitions?after=-600&last=200&options=minify", status: "200",
			guard: alertsV2Guard(
				alertsV2Rows{member: "transitions", keys: alertsV2TransitionKeys, items: alertsV2Transitions, sorted: true},
				alertsV2Rows{member: "items", keys: items, items: []string{"9 9 9 200 0 0"}})},
		// the newest four after the second switch: phase 3's three changes and hs_avg's change to CRITICAL, which its
		// window shows after phase 2 began (S2 = the switch's second, one value for both sides)
		{name: "anchor", target: fmt.Sprintf("/api/v2/alert_transitions?after=-600&last=4&anchor_gi=%d&options=minify", switched*1_000_000),
			status: "200",
			guard: alertsV2Guard(
				alertsV2Rows{member: "transitions", keys: alertsV2TransitionKeys, items: alertsV2Newest, sorted: true},
				alertsV2Rows{member: "items", keys: items, items: []string{"9 9 4 4 3 2"}})},
		{name: "one", target: one + "<hs_calc's change to WARNING>", targets: [2]string{one + raised[0], one + raised[1]},
			status: "200",
			guard: alertsV2Guard(
				alertsV2Rows{member: "transitions", keys: alertsV2TransitionKeys, items: []string{"hs_calc CLEAR WARNING 70"}},
				alertsV2Rows{member: "items", keys: items, items: []string{"1 1 1 1 0 0"}})},
		// a status the window leaves out (neither side of it is above CLEAR): asked by its id it answers
		{name: "one-clear", target: one + "<hs_calc's first status>", targets: [2]string{one + first[0], one + first[1]},
			status: "200", guard: alertsV2OneClear},
	} {
		compareV2(t, h.p, req, fam)
	}
}
