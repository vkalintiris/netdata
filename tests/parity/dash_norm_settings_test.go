// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"crypto/md5"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"
)

// dashNormSettingsRecord is one row of TestSettingsAPI as both C agents answered it in the recorded run
// (dash_norm_settings_data_test.go, generated): the row (its pair, its name, what it asked: the MD5 of the request's
// bytes; the file it is about), the answer after maskAnswer and settingsAnswer (its status line, the index of its
// head, its Content-Length and its body), the state of the row's file after it, the settings records it added and,
// for a row of a recorded difference, the file's state before the request. `kind` is `row`, `reference` (the
// reference row of a recorded difference) or `recorded`.
type dashNormSettingsRecord struct {
	pair, kind, name, method, target, file, request string
	status                                          string
	head, length                                    int
	body                                            string
	state, before                                   settingsState
	records                                         []string
}

// outcome is the recorded outcome as the check's judges take one.
func (d dashNormSettingsRecord) outcome() settingsOutcome {
	head := strings.Replace(dashNormSettingsHeads[d.head], "LENGTH", strconv.Itoa(d.length), 1)
	return settingsOutcome{answer: []byte(d.status + "\r\n" + head + "\r\n\r\n" + d.body), state: d.state,
		records: slices.Clone(d.records)}
}

// dashNormSettingsPlant is a wrong outcome made of a recorded one: what it is, and whether the oracle's guard sees it
// (it reads the answer's start and end, the state and the records; the rest of the head is the comparison's).
type dashNormSettingsPlant struct {
	what    string
	guarded bool
	outcome settingsOutcome
}

// dashNormSettingsPlants are the wrong outcomes a row must refuse, made of its recorded one: the answer with one more
// byte before its body, with another status, with its expiry at another distance from its date; the file with other
// bytes, another mode (or there when it should not be), a `.new` left (or gone), the directory with another mode (or
// made when it should not be); one record more, one record fewer, a record with an errno it should not have or
// without the one it should.
func dashNormSettingsPlants(r settingsRow, o settingsOutcome) []dashNormSettingsPlant {
	var plants []dashNormSettingsPlant
	answer := func(what string, guarded bool, old, new string) {
		if !bytes.Contains(o.answer, []byte(old)) {
			return
		}
		c := o
		c.answer = bytes.Replace(o.answer, []byte(old), []byte(new), 1)
		plants = append(plants, dashNormSettingsPlant{what, guarded, c})
	}
	answer("a byte before the body", true, "\r\n\r\n", "\r\n\r\n ")
	answer("another status", true, "HTTP/1.1 ", "HTTP/1.1 299 Other\r\nX-Was: ")
	answer("an expiry a second after the date", false, "\r\nExpires: Date+0\r\n", "\r\nExpires: Date+1\r\n")
	answer("an expiry at the date", false, "\r\nExpires: Date+1\r\n", "\r\nExpires: Date+0\r\n")
	if r.file != "" {
		state := func(what string, change func(s *settingsState)) {
			c := o
			change(&c.state)
			plants = append(plants, dashNormSettingsPlant{what, true, c})
		}
		state("other bytes in the file", func(s *settingsState) { s.data += "x" })
		if o.state.mode == "" {
			state("a file where none belongs", func(s *settingsState) { s.mode = setFileMode })
		} else {
			state("another mode of the file", func(s *settingsState) { s.mode = "-rw-r--r--" })
			state("no file", func(s *settingsState) { s.mode, s.data = "", "" })
		}
		if o.state.tmp == "" {
			state("a .new left", func(s *settingsState) { s.tmp = setFileMode })
		} else {
			state("the .new gone", func(s *settingsState) { s.tmp = "" })
		}
		if o.state.dir == "" {
			state("a directory made", func(s *settingsState) { s.dir = setDirMode })
		} else {
			state("another mode of the directory", func(s *settingsState) { s.dir = "drwxr-xr-x" })
			state("no directory", func(s *settingsState) { s.dir = "" })
		}
	}
	records := func(what string, records []string) {
		c := o
		c.records = records
		plants = append(plants, dashNormSettingsPlant{what, true, c})
	}
	records("one record more", append(slices.Clone(o.records), "time=T comm=netdata source=daemon level=error "+
		"tid=N thread=WEB[n] msg=\"file '<RUN>/lib/settings/x' cannot be parsed to extract version\""))
	for k, l := range o.records {
		records("a record fewer", slices.Delete(slices.Clone(o.records), k, k+1))
		changed := slices.Clone(o.records)
		if strings.Contains(l, " errno=") {
			changed[k] = errnoRe.ReplaceAllString(l, "")
			records("a record without its errno", changed)
		} else {
			changed[k] = strings.Replace(l, " level=error ", ` level=error errno="2, No such file or directory" `, 1)
			records("a record with an errno", changed)
		}
	}
	return plants
}

// testDashNormSettings pins the settings check (TestSettingsAPI): its pure judges (settingsJudge,
// settingsRecordedJudge), its state reader, its record render and its table's rails, with no agent started. Every
// row's own facts are held to what both C agents made of the row in one recorded run (dashNormSettingsRun), and every
// row must refuse the wrong outcomes made of its recorded one (dashNormSettingsPlants) and the named ones below.
func testDashNormSettings(t *testing.T) {
	recorded := map[string]dashNormSettingsRecord{}
	for _, d := range dashNormSettingsRun {
		recorded[d.pair+"/"+d.name] = d
	}
	used := map[string]bool{}
	// the recorded outcome of a row, which must still ask what was recorded
	of := func(pair, kind string, r settingsRow) (dashNormSettingsRecord, bool) {
		t.Helper()
		d, ok := recorded[pair+"/"+r.name]
		if !ok {
			t.Errorf("%s/%s: no recorded outcome: record the rows again (a C-against-C run of TestSettingsAPI with a "+
				"dump of each row's outcome, as dash_norm_settings_data_test.go was made)", pair, r.name)
			return d, false
		}
		used[pair+"/"+r.name] = true
		request := v2Request(v2Req{method: r.method, target: r.target, body: r.body, headers: r.headers})
		if sum := fmt.Sprintf("%x", md5.Sum(request)); d.kind != kind || d.method != r.method || d.target != r.target ||
			d.request != sum || d.file != r.file {
			t.Errorf("%s/%s: the row is a %s asking %s %q (request MD5 %s) about file %q; recorded was a %s asking "+
				"%s %q (%s) about file %q", pair, r.name, kind, r.method, r.target, sum, r.file, d.kind, d.method,
				d.target, d.request, d.file)
			return d, false
		}
		return d, true
	}

	rows := map[string]map[string]settingsRow{}
	for _, pair := range settingsPairs {
		table := settingsRowsOf(pair)
		if problem := settingsRowsProblem(table); problem != "" {
			t.Errorf("%s: %s", pair, problem)
		}
		rows[pair] = map[string]settingsRow{}
		for _, r := range table {
			rows[pair][r.name] = r
			// a row and a reference row: the recorded C pair shows nothing; each plant on the candidate's side is one
			// difference; each plant the guard reads, on the oracle's side, fails the row there
			plain := func(kind string, r settingsRow) (settingsOutcome, bool) {
				t.Helper()
				d, ok := of(pair, kind, r)
				if !ok {
					return settingsOutcome{}, false
				}
				o := d.outcome()
				if oracle, diffs := settingsJudge(r, [2]settingsOutcome{o, o}); oracle != "" || len(diffs) > 0 {
					t.Errorf("%s/%s: the recorded C pair: oracle %q, differences %q", pair, r.name, oracle, diffs)
				}
				for _, plant := range dashNormSettingsPlants(r, o) {
					oracle, diffs := settingsJudge(r, [2]settingsOutcome{o, plant.outcome})
					if oracle != "" || len(diffs) != 1 {
						t.Errorf("%s/%s: a candidate with %s: oracle %q, differences %q, want one difference", pair, r.name,
							plant.what, oracle, diffs)
					}
					oracle, diffs = settingsJudge(r, [2]settingsOutcome{plant.outcome, o})
					if (oracle != "") != plant.guarded || (oracle == "" && len(diffs) != 1) {
						t.Errorf("%s/%s: an oracle with %s: oracle %q, differences %q (the guard reads it: %v)", pair,
							r.name, plant.what, oracle, diffs, plant.guarded)
					}
				}
				return o, true
			}
			if r.recorded == nil {
				plain("row", r)
				continue
			}
			ref, ok := plain("reference", *r.recorded)
			d, ok2 := of(pair, "recorded", r)
			if !ok || !ok2 {
				continue
			}
			testDashNormSettingsRecorded(t, pair+"/"+r.name, r, d.outcome(), d.before, ref)
		}
	}
	for key := range recorded {
		if !used[key] {
			t.Errorf("%s was recorded and is no row: record the rows again", key)
		}
	}

	// named wrong candidates, each made of a row's recorded outcome by one replacement in its answer (`answer`), in
	// its file's bytes (`data`) or modes (`mode`, `tmp`, `dir`), or in its records (`records`: joined by newlines)
	for _, c := range []struct{ pair, row, what, field, old, new string }{
		{"rows", "esc", "a decoded escape printed back as one", "data", "\u00e9\u00e9", "\u00e9\\u00e9"},
		{"rows", "esc", "a slash not escaped", "data", `\/`, `/`},
		{"rows", "esc", "0x7f escaped", "data", "\x7f", `\u007f`},
		{"rows", "esc", "a raw tab kept raw", "data", `\t`, "\t"},
		{"rows", "esc-short", "U+2028 escaped", "data", "\u2028", `\u2028`},
		{"rows", "nul-value", "a value cut at its NUL", "data", `a\u0000b`, `a`},
		{"rows", "bad-utf8", "a byte that is no UTF-8 replaced", "data", "\xff", "\ufffd"},
		{"rows", "num", "minus zero kept", "data", "[ 0, 1e5", "[ -0, 1e5"},
		{"rows", "num", "a double printed from its value", "data", " 1e5,", " 100000.0,"},
		{"rows", "num", "an integer beyond 64 bits wrapped", "data", "18446744073709551615, -9223372036854775808, 1e400",
			"0, -9223372036854775808, 1e400"},
		{"rows", "num", "1e400 as an infinity", "data", "1e400", "Infinity"},
		{"rows", "order", "the version moved first", "data", `{ "z": 3, "version": 2,`, `{ "version": 2, "z": 3,`},
		{"rows", "order", "a repeated key at its last place", "data", `{ "z": 3, "version": 2, "a": { "b": 2 } }`,
			`{ "version": 2, "a": { "b": 2 }, "z": 3 }`},
		{"rows", "order", "a repeated key with its first value", "data", `"z": 3`, `"z": 1`},
		{"rows", "containers", "an empty object without its blank", "data", `"o": { }`, `"o": {}`},
		{"rows", "containers", "a compact print", "data", `{ "version": 2, "o": { }, "a": [ 1, 2 ]`,
			`{"version":2,"o":{},"a":[1,2]`},
		{"rows", "tail", "the text after the root refused", "answer", "HTTP/1.1 200 OK", "HTTP/1.1 400 Bad Request"},
		{"rows", "lone", "a lone surrogate as CESU-8", "data", "\ufffd", "\xed\xa0\xbd"},
		{"rows", "nul-key", "a key kept past its NUL", "data", `"a": 2`, `"a\u0000b": 1, "a": 2`},
		{"rows", "nul-junk", "the bytes after the NUL stored", "data", `{ "version": 2 }`, "{ \"version\": 2 }\x00junk"},
		{"rows", "depth32", "a value inside 32 containers read", "answer", "HTTP/1.1 400 Bad Request", "HTTP/1.1 200 OK"},
		{"rows", "slash", "a slash after the root ignored", "mode", "", setFileMode},
		{"rows", "v-dbl", "a double rounded: the conflict", "answer", setOK, setStale},
		{"rows", "v-stre3", "the member moved", "data", `{ "a": 0, "version": 2, "z": 9 }`,
			`{ "version": 2, "a": 0, "z": 9 }`},
		{"rows", "v-big", "the low 32 bits taken: stored", "mode", "", setFileMode},
		{"rows", "st-notjson-get", "the file served as it is", "answer", setFresh, "not json"},
		{"rows", "st-notjson-get", "another text in the record", "records", "cannot be parsed to extract version",
			"has no version"},
		{"rows", "st-notjson-put", "the file kept", "data", `{ "version": 2 }`, "not json"},
		{"rows", "st-nl-get", "the stored file printed again", "answer", "{\"version\":2}\n", `{ "version": 2 }`},
		{"rows", "st-nul-get", "the answer cut at the NUL", "answer", "{\"version\":2}\x00junk", `{"version":2}`},
		{"rows", "st-range-get", "the record without the errno json-c's reading left", "records",
			` errno="34, Numerical result out of range"`, ""},
		{"rows", "st-range-get", "the record with another errno", "records", `errno="34, Numerical result out of range"`,
			`errno="22, Invalid argument"`},
		{"rows", "st-max-put", "the next of the highest one more", "data", "-2147483648", "2147483648"},
		{"rows", "st-max-above", "a version above 32 bits wrapped to the lowest: stored", "data",
			`{ "version": -2147483648, "a": 1 }`, `{ "version": -2147483647 }`},
		{"rows", "st-neg-put", "the next of -1 refused", "data", `{ "version": 0 }`, `{"version":-1}`},
		{"rows", "st-neg-get2", "version 0 served", "answer", setFresh, `{ "version": 0 }`},
		{"rows", "new-dir-refused", "the directory at .new removed", "tmp", setDirMode, ""},
		{"rows", "new-dir-refused", "the record's errno the open's", "records", `errno="22, Invalid argument"`,
			`errno="21, Is a directory"`},
		{"rows", "new-link-refused", "the link followed: the file stored", "mode", "", setFileMode},
		{"rows", "name-dir-refused", "the .new left", "tmp", "", setFileMode},
		{"rows", "name-dir-refused", "the record at another level", "records", " level=error ", " level=warning "},
		{"rows", "stale-put", "the file given C's own mode", "mode", "-rw-------", setFileMode},
		{"rows", "stale-put", "the stale bytes kept after the document", "data", `{ "version": 2 }`,
			`{ "version": 2 }and longer than the document that follows`},
		{"rows", "link-put", "the link kept and its file written", "mode", setFileMode, "Lrwxrwxrwx"},
		{"rows", "mode0-get", "a record for a file that cannot be read", "records", "", "time=T x msg=\"file '<RUN>/" +
			"lib/settings/mode0' cannot be parsed to extract version\""},
		{"rows", "limit-get", "a file of 20 MiB not served", "answer", "<20971520 bytes, md5 ", setFresh + "<"},
		{"rows", "over-get", "a file above 20 MiB served", "answer", setFresh, "<20971521 bytes>"},
		{"rows", "member-put", "the member refused", "answer", "HTTP/1.1 200 OK", "HTTP/1.1 400 Bad Request"},
		{"rows", "query-percent-amp", "the query split before it is decoded", "answer", "HTTP/1.1 200 OK",
			"HTTP/1.1 400 Bad Request"},
		{"rows", "path-v1", "the 404 with another text", "answer", setUnsupported, "Unsupported API command: v1"},
		{"rows", "new-mode0-refused", "the record's errno EINVAL", "records", `errno="13, Permission denied"`,
			`errno="22, Invalid argument"`},
		{"rows", "name-252-refused", "the name cut to what a directory takes: stored", "mode", "", setFileMode},
		{"rows", "query-bad-escape-put", "the bad escape kept as text: no file `3`", "mode", setFileMode, ""},
		{"first", "anonymous-get", "the record with an account and a user", "records", " src_transport=http ",
			" src_transport=http account=b6b6b6b666664666866600000000acc1 user=fnhttp-admin "},
		{"first", "anonymous-get", "the record with the admin's role and permissions", "records",
			" role=none permissions=0x8 ", " role=admin permissions=0x7ff "},
		{"first", "path-link-put", "the link replaced by a directory", "dir", "Lrwxrwxrwx", setDirMode},
		{"first", "path-dangling-put", "a directory made through the link", "answer", "HTTP/1.1 400 Bad Request",
			"HTTP/1.1 200 OK"},
		{"first", "put-empty", "the directory made for a PUT without a payload", "dir", "", setDirMode},
		{"first", "path-file-put", "the file at the settings path replaced", "dir", "-rw-r-----", setDirMode},
		{"first", "put-unparsed", "no directory made", "dir", setDirMode, ""},
		{"first", "put-unparsed", "the directory made with another mode", "dir", setDirMode, "drwxr-xr-x"},
	} {
		r, ok := rows[c.pair][c.row]
		d, ok2 := recorded[c.pair+"/"+c.row]
		if !ok || !ok2 || r.recorded != nil {
			t.Errorf("%s/%s (%s): no such plain row, or it was not recorded", c.pair, c.row, c.what)
			continue
		}
		o := d.outcome()
		w := d.outcome()
		field := map[string]*string{"data": &w.state.data, "mode": &w.state.mode, "tmp": &w.state.tmp, "dir": &w.state.dir}
		switch p := field[c.field]; {
		case c.field == "answer" && bytes.Contains(o.answer, []byte(c.old)):
			w.answer = bytes.Replace(o.answer, []byte(c.old), []byte(c.new), 1)
		case c.field == "records" && c.old == "" && len(o.records) == 0:
			w.records = []string{c.new}
		case c.field == "records" && c.old != "" && strings.Contains(strings.Join(o.records, "\n"), c.old):
			w.records = strings.Split(strings.Replace(strings.Join(o.records, "\n"), c.old, c.new, 1), "\n")
		case p != nil && c.old == "" && *p == "":
			*p = c.new
		case p != nil && c.old != "" && strings.Contains(*p, c.old):
			*p = strings.Replace(*p, c.old, c.new, 1)
		default:
			t.Errorf("%s/%s (%s): the recorded %s does not hold %q: the plant plants nothing", c.pair, c.row, c.what,
				c.field, c.old)
			continue
		}
		if oracle, diffs := settingsJudge(r, [2]settingsOutcome{o, w}); oracle != "" || len(diffs) != 1 {
			t.Errorf("%s/%s: a candidate with %s: oracle %q, differences %q, want one difference", c.pair, c.row, c.what,
				oracle, diffs)
		}
		if oracle, _ := settingsJudge(r, [2]settingsOutcome{w, o}); oracle == "" {
			t.Errorf("%s/%s: an oracle with %s passed the row's guard", c.pair, c.row, c.what)
		}
	}

	testDashNormSettingsRails(t)
	testDashNormSettingsHelpers(t)
}

// testDashNormSettingsRecorded pins settingsRecordedJudge on one row of a recorded difference: the row r, C's
// recorded outcome of it, the file's state before it and C's recorded outcome of the reference row.
func testDashNormSettingsRecorded(t *testing.T, name string, r settingsRow, o settingsOutcome, before settingsState,
	ref settingsOutcome) {
	t.Helper()
	judge := func(c settingsOutcome) (string, string, []string) {
		return settingsRecordedJudge(r, [2]settingsOutcome{o, c}, [2]settingsState{before, before}, ref)
	}
	// C's pair, and the recorded refusal: what C answered the reference, the file left as it was
	if oracle, taken, diffs := judge(o); oracle != "" || taken != settingsAsOracle || len(diffs) > 0 {
		t.Errorf("%s: the recorded C pair: oracle %q, taken %q, differences %q", name, oracle, taken, diffs)
	}
	refusal := settingsOutcome{answer: ref.answer, state: before, records: ref.records}
	if oracle, taken, diffs := judge(refusal); oracle != "" || taken != settingsRefused || len(diffs) > 0 {
		t.Errorf("%s: the recorded refusal: oracle %q, taken %q, differences %q", name, oracle, taken, diffs)
	}
	// neither: each is one difference, and names neither way
	changed := o.state
	if changed == before {
		changed.data = setFresh // (a stored file rewritten as the fresh document)
	}
	wrong := map[string]settingsOutcome{
		"that stored other bytes": {answer: o.answer, state: settingsState{dir: o.state.dir, mode: o.state.mode,
			tmp: o.state.tmp, data: o.state.data + "x"}, records: o.records},
		"that answered another status": {answer: bytes.Replace(ref.answer, []byte("HTTP/1.1 "),
			[]byte("HTTP/1.1 500 Internal Server Error\r\nX-Was: "), 1), state: before, records: ref.records},
		"that refused with another text": {answer: append(slices.Clone(ref.answer), '.'), state: before,
			records: ref.records},
		"that refused and changed its file": {answer: ref.answer, state: changed, records: ref.records},
		"that refused and left a .new": {answer: ref.answer, state: settingsState{dir: before.dir, mode: before.mode,
			tmp: setFileMode, data: before.data}, records: ref.records},
		"that refused with one record more": {answer: ref.answer, state: before,
			records: append(slices.Clone(ref.records), "time=T one more")},
		"that answered as the oracle with one record more": {answer: o.answer, state: o.state,
			records: append(slices.Clone(o.records), "time=T one more")},
	}
	if o.state != before {
		wrong["that answered as the oracle and stored nothing"] = settingsOutcome{answer: o.answer, state: before,
			records: o.records}
	}
	if len(ref.records) > 0 {
		wrong["that refused without the record"] = settingsOutcome{answer: ref.answer, state: before}
	}
	for what, c := range wrong {
		if oracle, taken, diffs := judge(c); oracle != "" || taken != "" || len(diffs) != 1 {
			t.Errorf("%s: a candidate %s: oracle %q, taken %q, differences %q, want one difference", name, what, oracle,
				taken, diffs)
		}
	}
	// the oracle is held to the row first: a plant the guard reads fails it, whatever the candidate did
	for _, plant := range dashNormSettingsPlants(r, o) {
		oracle, taken, diffs := settingsRecordedJudge(r, [2]settingsOutcome{plant.outcome, refusal},
			[2]settingsState{before, before}, ref)
		if plant.guarded && (oracle == "" || taken != "" || len(diffs) > 0) {
			t.Errorf("%s: an oracle with %s: oracle %q, taken %q, differences %q", name, plant.what, oracle, taken, diffs)
		}
	}
	// an oracle that answers the row as it answered the reference records no difference: the reference row, taken
	// for the row itself, shows it
	same := *r.recorded
	same.recorded = r.recorded
	if oracle, _, _ := settingsRecordedJudge(same, [2]settingsOutcome{ref, ref}, [2]settingsState{ref.state, ref.state},
		ref); !strings.Contains(oracle, "the row records no difference") {
		t.Errorf("%s: an oracle that answers the row as the reference: %q", name, oracle)
	}
	// the two sides must hold the file alike before the row
	other := before
	other.data += "x"
	if oracle, _, _ := settingsRecordedJudge(r, [2]settingsOutcome{o, o}, [2]settingsState{before, other},
		ref); !strings.HasPrefix(oracle, "harness: before the row") {
		t.Errorf("%s: two sides that differ before the row: %q", name, oracle)
	}
}

// testDashNormSettingsRails pins settingsRowsProblem: each mistake of a table is named.
func testDashNormSettingsRails(t *testing.T) {
	ok := func(name string) settingsRow { return setGet(name, setFile("x"), setStatusOK, setFresh) }
	rec := func(name, file string) settingsRow {
		ref := ok(name+"-reference").on(file, setNoFile)
		r := ok(name).on(file, setNoFile)
		r.recorded = &ref
		return r
	}
	with := func(r settingsRow, change func(r *settingsRow)) settingsRow {
		change(&r)
		return r
	}
	for what, c := range map[string]struct {
		rows []settingsRow
		want string
	}{
		"a good table":         {[]settingsRow{ok("a"), ok("b").on("f", setNoFile), rec("c-recorded", "g")}, ""},
		"two rows of one name": {[]settingsRow{ok("a"), ok("a")}, `row "a": every row needs a name of its own`},
		"a row without a name": {[]settingsRow{ok("")}, `row "": every row needs a name of its own`},
		"a slash in a name":    {[]settingsRow{ok("a/b")}, `row "a/b": every row needs a name of its own`},
		"no status to start with": {[]settingsRow{with(ok("a"), func(r *settingsRow) { r.want[0] = "" })},
			"row a has no guard"},
		"no body to end with": {[]settingsRow{with(ok("a"), func(r *settingsRow) { r.want[1] = "" })},
			"row a has no guard"},
		"an end that is no whole body": {[]settingsRow{with(ok("a"), func(r *settingsRow) { r.want[1] = setFresh })},
			"row a has no guard"},
		"a state without a file": {[]settingsRow{with(ok("a"), func(r *settingsRow) { r.state = setNoFile })},
			"row a has a state or lays files out and names no file"},
		"laid files without a file": {[]settingsRow{ok("a").laid(setLayDirs("d"))},
			"row a has a state or lays files out and names no file"},
		"a name that says recorded": {[]settingsRow{ok("a-recorded")}, "row a-recorded says it is recorded and has no"},
		"a recorded row that does not say so": {[]settingsRow{rec("c", "g")},
			"row c compares a recorded difference: its name must end in -recorded"},
		"a recorded row without a file": {[]settingsRow{with(rec("c-recorded", "g"), func(r *settingsRow) {
			r.file, r.state = "", settingsState{}
		})}, "row c-recorded compares a recorded difference and names no file"},
		"a reference that is recorded": {[]settingsRow{with(rec("c-recorded", "g"), func(r *settingsRow) {
			ref := *r.recorded
			ref.recorded = r.recorded
			r.recorded = &ref
		})}, "row c-recorded: its reference must be a guarded row on g"},
		"a reference without a guard": {[]settingsRow{with(rec("c-recorded", "g"), func(r *settingsRow) {
			ref := *r.recorded
			ref.want[0] = ""
			r.recorded = &ref
		})}, "row c-recorded: its reference must be a guarded row on g"},
		"a reference on another file": {[]settingsRow{with(rec("c-recorded", "g"), func(r *settingsRow) {
			ref := r.recorded.on("h", setNoFile)
			r.recorded = &ref
		})}, "row c-recorded: its reference must be a guarded row on g"},
		"a recorded row's file used again": {[]settingsRow{rec("c-recorded", "g"), ok("d").on("g", setNoFile)},
			"row c-recorded compares a recorded difference on g, which 1 other rows use"},
		"a recorded row's file used before": {[]settingsRow{ok("d").on("g", setNoFile), rec("c-recorded", "g")},
			"row c-recorded compares a recorded difference on g, which 1 other rows use"},
	} {
		got := settingsRowsProblem(c.rows)
		if (c.want == "") != (got == "") || !strings.HasPrefix(got, c.want) {
			t.Errorf("rails: %s: %q, want %q at its start", what, got, c.want)
		}
	}
	// the check's own tables: every recorded difference says so in its row's name, and the committed twenty lead
	names := func(pair string) []string {
		var out []string
		for _, r := range settingsRowsOf(pair) {
			out = append(out, r.name)
		}
		return out
	}
	committed := "get-fresh put-v1 get-v2 put-stale put-v2 get-v3 put-not-json put-no-version put-array put-empty " +
		"get-other get-dot get-dotdot get-none post delete host admin-get admin-put admin-get2"
	if got := strings.Join(names("rows")[:20], " "); got != committed {
		t.Errorf("rails: the committed twenty rows lead the `rows` pair in their order: %s", got)
	}
	if got := strings.Join(names("first"), " "); got != "get-fresh put-empty post path-file-put path-file-unparsed "+
		"path-file-get put-unparsed get-after put-v1 anonymous-get anonymous-put path-link-put path-dangling-put "+
		"path-dangling-get" {
		t.Errorf("rails: the `first` pair's rows: %s", got)
	}
	var recorded []string
	for _, r := range settingsRowsOf("rows") {
		if r.recorded != nil {
			recorded = append(recorded, strings.TrimSuffix(r.name, "-recorded"))
		}
	}
	if got := strings.Join(recorded, " "); got != "f10-comment f10-line-comment f10-tail-comment f10-quote f10-comma "+
		"f10-nan f10-zero f10-case f10-dot f10-exp f10-st-quote f10-st-comment f10-st-comma f10-st-nan f10-st-zero "+
		"f10-st-case f10-st-dot" {
		t.Errorf("rails: the rows of a recorded difference: %s", got)
	}
}

// dashNormSettingsLines are records of the two C agents of one run (H37's probes, C against C, 2026-10-08), as each
// wrote them: the settings handler's for a stored file without a version (a GET), for a `.new` that is a directory
// and for a failed rename, in the order written; between them a record of each side that is none of the handler's:
// on side 0 one that carries a settings request (the WebSocket handshake's refusal of an upgrade request on this
// path, which settingsRecordsOf reads), on side 1 one that carries none (a startup record, which it does not).
var dashNormSettingsLines = [2][]string{{
	`time=2026-10-08T15:44:56.499Z comm=netdata source=daemon level=error tid=354352 thread=WEB[27] src_transport=http ` +
		`account=b6b6b6b666664666866600000000acc1 user=fnhttp-admin role=admin permissions=0x7ff src_ip=localhost ` +
		`src_port=60048 req_method=GET conn=0 transaction=736f4e9d8c6a46a0a93d9b063a18abf1 ` +
		`request="/api/v3/settings?file=st-notjson" msg="file '/ndt/TestSagProberows1700712517/001/oracle/lib/settings/` +
		`st-notjson' cannot be parsed to extract version"`,
	`time=2026-10-08T15:46:58.304Z comm=netdata source=daemon level=error tid=467148 thread=WEB[21] src_transport=http ` +
		`role=none permissions=0x8 src_ip=localhost src_port=47388 req_method=GET conn=0 ` +
		`transaction=06eaac5914904eae9fab9736299d3ec1 request="/api/v3/settings?file=default" msg="WEBSOCKET: No valid ` +
		`protocol selected by either URL or subprotocol"`,
	`time=2026-10-08T15:45:01.151Z comm=netdata source=daemon level=error errno="22, Invalid argument" tid=354353 ` +
		`thread=WEB[28] src_transport=http account=b6b6b6b666664666866600000000acc1 user=fnhttp-admin role=admin ` +
		`permissions=0x7ff src_ip=localhost src_port=32810 req_method=PUT conn=0 ` +
		`transaction=cbb791a8b66c4b27ae817225bd9869b4 request="/api/v3/settings?file=newdir" msg="cannot open/create ` +
		`settings file '/ndt/TestSagProberows1700712517/001/oracle/lib/settings/newdir.new'"`,
	`time=2026-10-08T15:45:01.417Z comm=netdata source=daemon level=error errno="21, Is a directory" tid=354354 ` +
		`thread=WEB[29] src_transport=http account=b6b6b6b666664666866600000000acc1 user=fnhttp-admin role=admin ` +
		`permissions=0x7ff src_ip=localhost src_port=32864 req_method=PUT conn=0 ` +
		`transaction=5dad0558b99047fd871586987ef5f677 request="/api/v3/settings?file=rendir" msg="cannot rename file ` +
		`'/ndt/TestSagProberows1700712517/001/oracle/lib/settings/rendir.new' to ` +
		`'/ndt/TestSagProberows1700712517/001/oracle/lib/settings/rendir'"`,
}, {
	`time=2026-10-08T15:44:56.521Z comm=netdata source=daemon level=error tid=357171 thread=WEB[32] src_transport=http ` +
		`account=b6b6b6b666664666866600000000acc1 user=fnhttp-admin role=admin permissions=0x7ff src_ip=localhost ` +
		`src_port=58508 req_method=GET conn=0 transaction=4951c8548def422d8e2cfba0656b7236 ` +
		`request="/api/v3/settings?file=st-notjson" msg="file '/ndt/TestSagProberows1700712517/002/candidate/lib/` +
		`settings/st-notjson' cannot be parsed to extract version"`,
	`time=2026-10-08T15:46:37.664Z comm=netdata source=daemon level=info tid=448769  msg="NETDATA STARTUP: in      91 ` +
		`ms, anonymous analytics (disabled) - next: mrg cleanup"`,
	`time=2026-10-08T15:45:01.173Z comm=netdata source=daemon level=error errno="22, Invalid argument" tid=357160 ` +
		`thread=WEB[21] src_transport=http account=b6b6b6b666664666866600000000acc1 user=fnhttp-admin role=admin ` +
		`permissions=0x7ff src_ip=localhost src_port=59476 req_method=PUT conn=0 ` +
		`transaction=18386f44fa3242a495bd297fd966850e request="/api/v3/settings?file=newdir" msg="cannot open/create ` +
		`settings file '/ndt/TestSagProberows1700712517/002/candidate/lib/settings/newdir.new'"`,
	`time=2026-10-08T15:45:01.440Z comm=netdata source=daemon level=error errno="21, Is a directory" tid=357158 ` +
		`thread=WEB[19] src_transport=http account=b6b6b6b666664666866600000000acc1 user=fnhttp-admin role=admin ` +
		`permissions=0x7ff src_ip=localhost src_port=59508 req_method=PUT conn=0 ` +
		`transaction=8fc28bbee6ab438ab49c9ef2c9bd77d4 request="/api/v3/settings?file=rendir" msg="cannot rename file ` +
		`'/ndt/TestSagProberows1700712517/002/candidate/lib/settings/rendir.new' to ` +
		`'/ndt/TestSagProberows1700712517/002/candidate/lib/settings/rendir'"`,
}}

// testDashNormSettingsHelpers pins the check's readers and renders: the record render on the two C agents' lines of
// one run, the parts a row's records are held to, the bytes' and the answer's summary, the state reader and the
// layers on a directory of the test's own, and the log's mark.
func testDashNormSettingsHelpers(t *testing.T) {
	// the records: the handler's three records render alike on the two C sides, in file order; side 0's record of
	// another text that carries a settings request is read too, side 1's record without a request is not
	runs := [2]string{"/ndt/TestSagProberows1700712517/001/oracle", "/ndt/TestSagProberows1700712517/002/candidate"}
	want := []string{
		`time=T comm=netdata source=daemon level=error tid=N thread=WEB[n] src_transport=http ` +
			`account=b6b6b6b666664666866600000000acc1 user=fnhttp-admin role=admin permissions=0x7ff src_ip=localhost ` +
			`src_port=P req_method=GET conn=N transaction=X request="/api/v3/settings?file=st-notjson" ` +
			`msg="file '<RUN>/lib/settings/st-notjson' cannot be parsed to extract version"`,
		`time=T comm=netdata source=daemon level=error errno="22, Invalid argument" tid=N thread=WEB[n] ` +
			`src_transport=http account=b6b6b6b666664666866600000000acc1 user=fnhttp-admin role=admin permissions=0x7ff ` +
			`src_ip=localhost src_port=P req_method=PUT conn=N transaction=X request="/api/v3/settings?file=newdir" ` +
			`msg="cannot open/create settings file '<RUN>/lib/settings/newdir.new'"`,
		`time=T comm=netdata source=daemon level=error errno="21, Is a directory" tid=N thread=WEB[n] ` +
			`src_transport=http account=b6b6b6b666664666866600000000acc1 user=fnhttp-admin role=admin permissions=0x7ff ` +
			`src_ip=localhost src_port=P req_method=PUT conn=N transaction=X request="/api/v3/settings?file=rendir" ` +
			`msg="cannot rename file '<RUN>/lib/settings/rendir.new' to '<RUN>/lib/settings/rendir'"`,
	}
	var rendered [2][]string
	for i := range runs {
		rendered[i] = settingsRecordsOf(dashNormSettingsLines[i], runs[i])
	}
	if got := rendered[0]; len(got) != 4 || !strings.HasPrefix(got[1], "time=T comm=netdata source=daemon level=error ") ||
		!strings.HasSuffix(got[1], ` request="/api/v3/settings?file=default" msg="WEBSOCKET: No valid protocol selected `+
			`by either URL or subprotocol"`) {
		t.Errorf("records: side 0's record of another text that carries a settings request: %q", got)
	} else {
		rendered[0] = slices.Delete(slices.Clone(got), 1, 2)
	}
	for i := range runs {
		if !slices.Equal(rendered[i], want) {
			t.Errorf("records: side %d renders\n%s\nwant\n%s", i, strings.Join(rendered[i], "\n"), strings.Join(want, "\n"))
		}
	}
	// what a render must keep: each planted line of side 1 shows against side 0's
	for what, c := range map[string]struct {
		k        int
		old, new string
	}{
		"no errno on the open's record":      {2, ` errno="22, Invalid argument"`, ""},
		"another errno on the rename's":      {3, `errno="21, Is a directory"`, `errno="39, Directory not empty"`},
		"an errno on the version's record":   {0, " level=error ", ` level=error errno="2, No such file or directory" `},
		"another level":                      {0, " level=error ", " level=warning "},
		"another source":                     {0, " source=daemon ", " source=collector "},
		"another file in the text":           {0, "settings/st-notjson' cannot", "settings/st-notjso' cannot"},
		"another directory in the text":      {2, "002/candidate/lib/settings/newdir.new", "002/candidate/lib/newdir.new"},
		"another method":                     {0, " req_method=GET ", " req_method=PUT "},
		"another request":                    {0, `request="/api/v3/settings?file=st-notjson"`, `request="/api/v3/settings"`},
		"another role":                       {0, " role=admin ", " role=member "},
		"another permissions":                {0, " permissions=0x7ff ", " permissions=0x8 "},
		"no account":                         {0, " account=b6b6b6b666664666866600000000acc1", ""},
		"another thread":                     {0, " thread=WEB[32] ", " thread=HEALTH "},
		"the rename's record of another way": {3, `msg="cannot rename file '`, `msg="cannot move file '`},
	} {
		lines := slices.Clone(dashNormSettingsLines[1])
		if !strings.Contains(lines[c.k], c.old) {
			t.Errorf("records: %s: the line does not hold %q", what, c.old)
			continue
		}
		lines[c.k] = strings.Replace(lines[c.k], c.old, c.new, 1)
		if got := settingsRecordsOf(lines, runs[1]); slices.Equal(got, want) {
			t.Errorf("records: %s: a candidate's line renders as C's", what)
		}
	}
	// what a render must hide: each of these on side 1's lines still renders as C's
	for what, c := range map[string]struct {
		k        int
		old, new string
	}{
		"another connection count": {0, " conn=0 ", " conn=7 "},
		"another client port":      {2, " src_port=59476 ", " src_port=4 "},
		"another thread id":        {3, " tid=357158 ", " tid=1 "},
		"another web thread":       {0, " thread=WEB[32] ", " thread=WEB[1] "},
		"another clock":            {2, "time=2026-10-08T15:45:01.173Z ", "time=2027-01-01T00:00:00.000Z "},
		"another transaction":      {3, " transaction=8fc28bbee6ab438ab49c9ef2c9bd77d4 ", " transaction=00ff "},
	} {
		lines := slices.Clone(dashNormSettingsLines[1])
		if !strings.Contains(lines[c.k], c.old) {
			t.Errorf("records: %s: the line does not hold %q", what, c.old)
			continue
		}
		lines[c.k] = strings.Replace(lines[c.k], c.old, c.new, 1)
		if got := settingsRecordsOf(lines, runs[1]); !slices.Equal(got, want) {
			t.Errorf("records: %s shows in the render:\n%s", what, strings.Join(got, "\n"))
		}
	}
	// the fourth record; a text that only resembles one is read for the settings request it carries, and not without
	// one or with another route's; a handler's record is read by its text alone, without the request's fields
	save := strings.Replace(dashNormSettingsLines[0][2], "cannot open/create settings file",
		"cannot save settings to file", 1)
	near := strings.Replace(dashNormSettingsLines[0][0], "cannot be parsed to extract version", "cannot be parsed", 1)
	const request = ` request="/api/v3/settings?file=st-notjson"`
	bare := func(line string) string { return strings.Replace(line, request, "", 1) }
	other := strings.Replace(near, request, ` request="/api/v3/info?file=st-notjson"`, 1)
	if !strings.Contains(near, request) || bare(near) == near || other == near {
		t.Fatalf("records: the line does not hold %q", request)
	}
	if got := settingsRecordsOf([]string{save, near}, runs[0]); len(got) != 2 || !strings.Contains(got[0],
		`msg="cannot save settings to file '<RUN>/lib/settings/newdir.new'"`) || !strings.HasSuffix(got[1],
		request+` msg="file '<RUN>/lib/settings/st-notjson' cannot be parsed"`) {
		t.Errorf("records: the save's record and a record of another text on a settings request: %q", got)
	}
	if got := settingsRecordsOf([]string{bare(near), other}, runs[0]); len(got) != 0 {
		t.Errorf("records: a text that is none of the handler's, without a settings request: %q", got)
	}
	// the request's field as the agents write it: bare when its text needs no quotes (a target without a
	// parameter, or with an escaped `=`), under a routed host, in another version of the API, with a subpath; and
	// what is no settings request
	for form, read := range map[string]bool{
		` request=/api/v3/settings`:                                  true,
		` request=/api/v3/settings?`:                                 true,
		` request=/api/v3/settings?file%3Ddefault`:                   true,
		` request="/host/parity-child/api/v3/settings?file=default"`: true,
		` request="/api/v1/settings?file=default"`:                   true,
		` request="/api/v3/settings/x?file=default"`:                 true,
		` request=/api/v3/info`:                                      false,
		` request="/api/v3/settingsx?file=default"`:                  false,
		` request=/api/v3/settingsx`:                                 false,
	} {
		line := strings.Replace(near, request, form, 1)
		if got := settingsRecordsOf([]string{line}, runs[0]); (len(got) == 1) != read {
			t.Errorf("records: a record of another text with%s: read %v, want %v", form, len(got) == 1, read)
		}
	}
	for text, line := range map[string]string{
		"cannot be parsed to extract version": dashNormSettingsLines[0][0],
		"cannot open/create settings file":    dashNormSettingsLines[0][2],
		"cannot save settings to file":        save,
		"cannot rename file":                  dashNormSettingsLines[0][3],
	} {
		if got := settingsRecordsOf([]string{strings.Replace(line, ` request="`, ` asked="`, 1)}, runs[0]); len(got) != 1 ||
			!strings.Contains(got[0], text) || strings.Contains(got[0], ` request="`) {
			t.Errorf("records: the handler's record %q without the request's field: %q", text, got)
		}
	}
	// the parts a row holds the oracle's records to
	parts := func(records []string, want ...string) string {
		if err := healthRecordsAre(want...)(records); err != nil {
			return err.Error()
		}
		return ""
	}
	version, open := setVersionRecord("GET", "st-notjson"), setFailRecord("22, Invalid argument", "newdir",
		"open/create settings file '<RUN>/lib/settings/newdir.new'")
	rename := setFailRecord("21, Is a directory", "rendir",
		"rename file '<RUN>/lib/settings/rendir.new' to '<RUN>/lib/settings/rendir'")
	if got := parts(rendered[0], version, open, rename); got != "" {
		t.Errorf("records: the rows' parts on C's records: %s", got)
	}
	for what, c := range map[string]struct {
		k        int
		old, new string
		want     string
	}{
		"an errno on the version's record": {0, " src_transport=http ", ` errno="2, x" src_transport=http `,
			`record 1 holds "errno="`},
		"a PUT's version record":          {0, " req_method=GET ", " req_method=PUT ", "record 1 does not hold"},
		"another file's version record":   {0, "file=st-notjson", "file=st-other", "record 1 does not hold"},
		"a warning":                       {0, " level=error ", " level=warning ", "record 1 does not hold"},
		"another thread's record":         {0, " thread=WEB[n] ", " thread=HEALTH ", "record 1 does not hold"},
		"the open's record without errno": {1, ` errno="22, Invalid argument"`, "", "record 2 does not hold"},
		"the open's record with ENOENT":   {1, `errno="22, Invalid argument"`, `errno="2, x"`, "record 2 does not hold"},
		"the open's record of a GET":      {1, " req_method=PUT ", " req_method=GET ", "record 2 does not hold"},
		"the rename's record of one name": {2, " to '<RUN>/lib/settings/rendir'", "", "record 3 does not hold"},
	} {
		records := slices.Clone(rendered[0])
		if !strings.Contains(records[c.k], c.old) {
			t.Errorf("records: %s: the record does not hold %q", what, c.old)
			continue
		}
		records[c.k] = strings.Replace(records[c.k], c.old, c.new, 1)
		if got := parts(records, version, open, rename); !strings.HasPrefix(got, c.want) {
			t.Errorf("records: %s: %q, want %q", what, got, c.want)
		}
	}
	if got := parts(rendered[0][:2], version, open, rename); got != "2 records, want 3" {
		t.Errorf("records: a record fewer: %q", got)
	}
	if got := parts(rendered[0]); got != "3 records, want 0" {
		t.Errorf("records: records where a row wants none: %q", got)
	}

	// the summary of bytes and of an answer's body
	long := strings.Repeat("a", settingsLong+1)
	for _, c := range [][2]string{{"", ""}, {"{ \"version\": 2 }", "{ \"version\": 2 }"},
		{long[:settingsLong], long[:settingsLong]}, {long, "<4097 bytes, md5 8cfc1a0bd8cd76599e76e5e721c6e62e>"},
		{long[:settingsLong] + "b", "<4097 bytes, md5 106836fafd099d4be8ef1e58a2049ea8>"}} {
		if got := settingsData([]byte(c[0])); got != c[1] {
			t.Errorf("settingsData of %d bytes: %q, want %q", len(c[0]), got, c[1])
		}
	}
	head := "HTTP/1.1 200 OK\r\nContent-Length: 4097"
	for _, c := range [][2]string{{head + "\r\n\r\nshort", head + "\r\n\r\nshort"},
		{head + "\r\n\r\n" + long[:settingsLong], head + "\r\n\r\n" + long[:settingsLong]},
		{head + "\r\n\r\n" + long, head + "\r\n\r\n<4097 bytes, md5 8cfc1a0bd8cd76599e76e5e721c6e62e>"},
		{long, long}, {head, head}} {
		if got := string(settingsAnswer([]byte(c[0]))); got != c[1] {
			t.Errorf("settingsAnswer of %d bytes: %q, want %q", len(c[0]), truncateBytes([]byte(got)), c[1])
		}
	}

	// the state reader and the layers, on a directory of the test's own
	run := t.TempDir()
	if err := os.Mkdir(filepath.Join(run, "lib"), 0o755); err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(run, "lib", "settings")
	step := func(what string, lay func(dir string) error, file string, want settingsState) {
		t.Helper()
		if lay != nil {
			if err := lay(dir); err != nil {
				t.Fatalf("state: %s: %v", what, err)
			}
		}
		if got := settingsStateOf(run, file); got != want {
			t.Errorf("state: %s: %s, want %s", what, got, want)
		}
	}
	step("nothing", nil, "f", settingsState{})
	step("a file at the settings path", func(dir string) error {
		if err := os.WriteFile(dir, nil, 0o600); err != nil {
			return err
		}
		return os.Chmod(dir, 0o640)
	}, "f", settingsState{dir: "-rw-r-----"})
	step("a laid file makes the directory as C does", setLayAll(func(dir string) error { return os.Remove(dir) },
		setLayFile("f", `{"version":2}`, 0o660)), "f", setStored(`{"version":2}`))
	step("another file is not this one", nil, "g", setNoFile)
	step("a file's own .new is not the file", nil, "f.ne", setNoFile)
	step("a laid file again, with another mode", setLayFile("f", "x", 0o600), "f", setHeld("-rw-------", "x"))
	if os.Geteuid() != 0 {
		step("a file of mode 0", setLayFile("zero", "x", 0), "zero", setHeld("----------", ""))
	}
	step("a .new beside the file", setLayFile("f.new", "left", 0o600), "f", settingsState{dir: setDirMode,
		mode: "-rw-------", tmp: "-rw-------", data: "x"})
	step("a .new alone", setLayFile("n.new", "left", 0o660), "n", settingsState{dir: setDirMode, tmp: setFileMode})
	step("directories, each under its parent", setLayDirs("d", "d/inside", "e.new"), "d", setHeld(setDirMode, ""))
	step("a directory at .new", nil, "e", settingsState{dir: setDirMode, tmp: setDirMode})
	step("a link to a file", setLayLink("l", "f"), "l", setHeld("Lrwxrwxrwx", "x"))
	step("a link to nothing", setLayLink("m", "nowhere"), "m", setHeld("Lrwxrwxrwx", ""))
	step("a link at .new", setLayLink("o.new", "nowhere"), "o", settingsState{dir: setDirMode, tmp: "Lrwxrwxrwx"})
	step("a long file", setLayFile("long", long, 0o660), "long",
		setStored("<4097 bytes, md5 8cfc1a0bd8cd76599e76e5e721c6e62e>"))
	step("removed entries", setLayRemove("f.new", "m"), "f", setHeld("-rw-------", "x"))
	step("removed entries, the second", nil, "m", setNoFile)
	if err := setLayRemove("no-such-entry")(dir); !errors.Is(err, fs.ErrNotExist) {
		t.Errorf("state: removing what is not there: %v", err)
	}
	if err := setLayDirs("d")(dir); !errors.Is(err, fs.ErrExist) {
		t.Errorf("state: making a directory that is there: %v", err)
	}
	if err := setLayAll(setLayRemove("no-such-entry"), setLayFile("after", "x", 0o660))(dir); err == nil ||
		settingsStateOf(run, "after") != setNoFile {
		t.Errorf("state: a layer after a failed one ran (error %v)", err)
	}
	if err := os.Chmod(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	step("a directory of another mode stays as it is under a laid file", setLayFile("p", "x", 0o660), "p",
		settingsState{dir: "drwx------", mode: setFileMode, data: "x"})

	// the log's mark: what a file gains after it, whole lines, and nothing of a file that is not there
	if mark := settingsLogMark(run); mark != 0 {
		t.Errorf("log: the mark of no file: %d", mark)
	}
	if lines, err := settingsLogSince(run, 0); err != nil || lines != nil {
		t.Errorf("log: the lines of no file: %q, %v", lines, err)
	}
	if err := os.Mkdir(filepath.Join(run, "log"), 0o755); err != nil {
		t.Fatal(err)
	}
	log := filepath.Join(run, "log", "daemon.log")
	if err := os.WriteFile(log, []byte("one\ntwo\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	mark := settingsLogMark(run)
	if mark != 8 {
		t.Errorf("log: the mark after two lines: %d, want 8", mark)
	}
	if err := os.WriteFile(log, []byte("one\ntwo\nthree\n\nfour\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if lines, err := settingsLogSince(run, mark); err != nil || !slices.Equal(lines, []string{"three", "four"}) {
		t.Errorf("log: the lines after the mark: %q, %v", lines, err)
	}
	if lines, err := settingsLogSince(run, 0); err != nil || len(lines) != 4 {
		t.Errorf("log: the lines after the start: %q, %v", lines, err)
	}
}
