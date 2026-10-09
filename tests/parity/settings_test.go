// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"crypto/md5"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// settingsRow is one request of the settings check and what the oracle must make of it: its answer (want), the state
// of the settings file the row is about, and the settings handler's records in its daemon.log.
type settingsRow struct {
	exactReq
	// file is the settings file the row is about (empty: none, and the settings directory is not looked at); state
	// is what the oracle's directory must hold for it after the row.
	file  string
	state settingsState
	// lay, when set, changes each side's settings directory (its path is the argument) before the request is sent:
	// a stored file the agent did not write, a directory or a link where a file belongs.
	lay func(dir string) error
	// records are the records the row must add to the oracle's daemon.log, in order (settingsRecordsOf: the settings
	// handler's four, and any other record that carries a settings request): the parts each must hold
	// (healthRecordsAre's form); none means the row must add none.
	records []string
	// recorded, when set, makes this a row of a recorded difference (D234 F10 as D240 corrects it: "the reader may
	// refuse a text json-c reads, but never reads a text differently, nor reads one json-c refuses"). It is the
	// reference row, asked first on the same file and judged as any row: what its request makes of a text no reader
	// takes. The candidate may then answer the row itself as the ORACLE answered the reference, with its file left
	// as it was (settingsRecordedJudge).
	recorded *settingsRow
}

// settingsState is what a run directory's `lib/settings` holds for a settings file: the modes of the directory, of
// the file and of `<file>.new` as `ls -l` prints them (empty: there is none), and the file's bytes (settingsData;
// empty too when it is nothing that can be read: a directory, a link to nothing, a file of mode 0).
type settingsState struct{ dir, mode, tmp, data string }

func (s settingsState) String() string {
	show := func(mode string) string {
		if mode == "" {
			return "none"
		}
		return mode
	}
	return fmt.Sprintf("directory %s, file %s %q, .new %s", show(s.dir), show(s.mode), s.data, show(s.tmp))
}

// settingsLong is the most bytes of a file, or of an answer's body, the check compares as they are.
const settingsLong = 4096

// settingsData is how a file's bytes and an answer's body are compared: as they are, or by their length and MD5 when
// they are more than settingsLong (the stored file of 20 MiB).
func settingsData(b []byte) string {
	if len(b) <= settingsLong {
		return string(b)
	}
	return fmt.Sprintf("<%d bytes, md5 %x>", len(b), md5.Sum(b))
}

// settingsAnswer is a masked answer as the check compares it: its head as it is, its body through settingsData.
func settingsAnswer(masked []byte) []byte {
	head, body, whole := bytes.Cut(masked, []byte("\r\n\r\n"))
	if !whole || len(body) <= settingsLong {
		return masked
	}
	return slices.Concat(head, []byte("\r\n\r\n"), []byte(settingsData(body)))
}

// settingsStateOf reads what runDir's settings directory holds for file. A link is not followed for its mode
// (`Lrwxrwxrwx`) and is followed for its bytes, as the agents read it (inlined.h:610-613).
func settingsStateOf(runDir, file string) settingsState {
	dir := filepath.Join(runDir, "lib", "settings")
	mode := func(path string) string {
		fi, err := os.Lstat(path)
		if err != nil {
			return ""
		}
		return fi.Mode().String()
	}
	s := settingsState{dir: mode(dir), mode: mode(filepath.Join(dir, file)), tmp: mode(filepath.Join(dir, file+".new"))}
	if b, err := os.ReadFile(filepath.Join(dir, file)); err == nil {
		s.data = settingsData(b)
	}
	return s
}

// settingsRecordRe finds the settings handler's four records in a daemon.log (api_v3_settings.c:99, :247, :258,
// :275), by their texts.
var settingsRecordRe = regexp.MustCompile(`msg="(file '[^']*' cannot be parsed to extract version"|` +
	`cannot open/create settings file '|cannot save settings to file '|cannot rename file ')`)

// settingsRequestRe finds a record that carries a settings request, whatever its text: what a web thread writes
// while it serves a request carries the request's fields. C writes no other record than the handler's four on the
// way of these rows' requests (web/api/web_api.c:48-107 and the handler log nothing else), so a record of another
// text is a candidate's own, and a row that expects no record fails on it. The field is the target as it came, in
// quotes only when its text needs them (an `=`, a blank, a quote, a backslash or a byte that is not printable
// ASCII: libnetdata/log/nd_log-format-logfmt.c:5-48): a target without a parameter is written bare.
var settingsRequestRe = regexp.MustCompile(` request="?[^" ]*/api/v[0-9]+/settings(["/? ]|$)`)

// settingsRecordsOf renders the settings records among a daemon.log's lines, in file order: the handler's four by
// their texts (settingsRecordRe) and every record that carries a settings request (settingsRequestRe). The run
// directory is written `<RUN>`, and the clock, the thread's id and number, the client's port and the transaction
// are masked (logMasks), as is the web server's connection count (healthConnRe: `conn=` counts the web-client
// cache's allocations). Everything else is compared: the level, the request's fields (a web thread's record carries
// its request and the client's account, role and permissions), the text with its paths, and `errno`: C's records of
// a failed open and of a failed rename carry the failing call's (EINVAL for a `.new` that is no regular file,
// api_v3_settings.c:121-124; nd_log.c:429, :341-342), its record of a stored file without a version carries none,
// or the one json-c's reading of the file left (setRangeRecord).
func settingsRecordsOf(lines []string, runDir string) []string {
	var out []string
	for _, l := range lines {
		if !settingsRecordRe.MatchString(l) && !settingsRequestRe.MatchString(l) {
			continue
		}
		l = strings.ReplaceAll(l, runDir, "<RUN>")
		for _, m := range logMasks {
			l = m.re.ReplaceAllString(l, m.with)
		}
		out = append(out, healthConnRe.ReplaceAllString(l, " conn=N"))
	}
	return out
}

// settingsLogMark is the size of runDir's daemon.log now: what is written after it is the next request's.
func settingsLogMark(runDir string) int64 {
	fi, err := os.Stat(filepath.Join(runDir, "log", "daemon.log"))
	if err != nil {
		return 0
	}
	return fi.Size()
}

// settingsLogSince are the lines runDir's daemon.log gained after mark. Both agents write a record in the handler,
// before the answer leaves: once the answer is read, the request's records are in the file.
func settingsLogSince(runDir string, mark int64) ([]string, error) {
	f, err := os.Open(filepath.Join(runDir, "log", "daemon.log"))
	if errors.Is(err, fs.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	defer f.Close()
	if _, err := f.Seek(mark, io.SeekStart); err != nil {
		return nil, err
	}
	b, err := io.ReadAll(f)
	if err != nil {
		return nil, err
	}
	return strings.FieldsFunc(string(b), func(r rune) bool { return r == '\n' }), nil
}

// settingsOutcome is what one side made of a row: its answer (after maskAnswer and settingsAnswer), the state of the
// row's file after it, and the settings handler's records the row added to its daemon.log (settingsRecordsOf).
type settingsOutcome struct {
	answer  []byte
	state   settingsState
	records []string
}

// settingsGuard is what is wrong with the oracle's outcome of r ("" when nothing): its answer must start and end as
// r wants, r's file be in r's state, and its records be r's, as many and each holding its parts.
func settingsGuard(r settingsRow, o settingsOutcome) string {
	if a := string(o.answer); !strings.HasPrefix(a, r.want[0]) || !strings.HasSuffix(a, r.want[1]) {
		return fmt.Sprintf("answered %q, want %q at its start and %q at its end", truncateBytes(o.answer), r.want[0],
			truncateBytes([]byte(r.want[1])))
	}
	if r.file != "" && o.state != r.state {
		return fmt.Sprintf("settings file %s: %s, want %s", r.file, o.state, r.state)
	}
	if err := healthRecordsAre(r.records...)(o.records); err != nil {
		return fmt.Sprintf("the settings records: %v:\n%s", err, strings.Join(o.records, "\n"))
	}
	return ""
}

// settingsDiffs are the differences between the oracle's outcome of r and the candidate's: the answers byte for
// byte, the file's state when r names a file, and the records.
func settingsDiffs(r settingsRow, o, c settingsOutcome) []string {
	var diffs []string
	if !bytes.Equal(o.answer, c.answer) {
		diffs = append(diffs, fmt.Sprintf("answers differ\n%s\noracle:    %q\ncandidate: %q",
			firstDifference(o.answer, c.answer), truncateBytes(o.answer), truncateBytes(c.answer)))
	}
	if r.file != "" && o.state != c.state {
		diffs = append(diffs, fmt.Sprintf("settings file %s differs:\noracle:    %s\ncandidate: %s", r.file, o.state,
			c.state))
	}
	if !slices.Equal(o.records, c.records) {
		diffs = append(diffs, fmt.Sprintf("the settings records differ:\noracle:\n%s\ncandidate:\n%s",
			strings.Join(o.records, "\n"), strings.Join(c.records, "\n")))
	}
	return diffs
}

// settingsJudge judges the two outcomes of r (0 the oracle's): what is wrong with the oracle's (settingsGuard), and,
// when nothing is, where the candidate's differs from it (settingsDiffs).
func settingsJudge(r settingsRow, got [2]settingsOutcome) (oracle string, diffs []string) {
	if oracle = settingsGuard(r, got[0]); oracle != "" {
		return oracle, nil
	}
	return "", settingsDiffs(r, got[0], got[1])
}

// What the candidate made of a row of a recorded difference (settingsRecordedJudge).
const (
	settingsAsOracle = "answered and stored as the oracle"
	settingsRefused  = "answered as the reference request, its file left as it was"
)

// settingsRecordedJudge judges the two outcomes of r, a row of a recorded difference (settingsRow.recorded), given
// each side's state of r's file before the request and the oracle's outcome of the reference row. The oracle is
// held to r as in any row, and must not have answered as it answered the reference (the row would then record no
// difference). The candidate must have done one of two things, which `taken` names: what the oracle did (the same
// answer, state and records), or what the oracle did with the reference (that answer and those records, byte for
// byte) with its file left as it was before the request. Anything else is a difference: other bytes stored, another
// status, a refusal that changed the file.
func settingsRecordedJudge(r settingsRow, got [2]settingsOutcome, before [2]settingsState,
	ref settingsOutcome) (oracle, taken string, diffs []string) {
	if before[0] != before[1] {
		return fmt.Sprintf("harness: before the row the two sides hold settings file %s differently:\noracle:    %s\n"+
			"candidate: %s", r.file, before[0], before[1]), "", nil
	}
	if oracle = settingsGuard(r, got[0]); oracle != "" {
		return oracle, "", nil
	}
	if bytes.Equal(got[0].answer, ref.answer) {
		return fmt.Sprintf("answered as it answered the reference request, %q: the row records no difference",
			truncateBytes(ref.answer)), "", nil
	}
	c := got[1]
	asOracle := settingsDiffs(r, got[0], c)
	if len(asOracle) == 0 {
		return "", settingsAsOracle, nil
	}
	refused := settingsDiffs(r, settingsOutcome{answer: ref.answer, state: before[1], records: ref.records}, c)
	if len(refused) == 0 {
		return "", settingsRefused, nil
	}
	return "", "", []string{fmt.Sprintf("the candidate neither did as the oracle nor refused as the recorded "+
		"difference allows (the oracle's answer to the reference request, its own file left as it was).\n"+
		"Against the oracle: %s\nAgainst the recorded refusal (\"oracle\" below is the refusal): %s",
		strings.Join(asOracle, "\n"), strings.Join(refused, "\n"))}
}

// settingsAsk lays r's files out on both sides, asks r of both (exactAnswers) and hands back each side's outcome
// and, from before the request, each side's state of r's file.
func settingsAsk(t *testing.T, p *Pair, r settingsRow) (before [2]settingsState, got [2]settingsOutcome) {
	t.Helper()
	var marks [2]int64
	for i, side := range p.Each() {
		run := side.Daemon.Opts.RunDir
		if r.lay != nil {
			if err := r.lay(filepath.Join(run, "lib", "settings")); err != nil {
				t.Fatalf("harness: %s: the %s's settings directory: %v", r.name, side.Role, err)
			}
		}
		if r.file != "" {
			before[i] = settingsStateOf(run, r.file)
		}
		marks[i] = settingsLogMark(run)
	}
	answers, err := exactAnswers(p, r.exactReq)
	if err != nil {
		t.Fatal(err)
	}
	for i, side := range p.Each() {
		run := side.Daemon.Opts.RunDir
		lines, err := settingsLogSince(run, marks[i])
		if err != nil {
			t.Fatalf("harness: %s: the %s's daemon.log: %v", r.name, side.Role, err)
		}
		got[i] = settingsOutcome{answer: settingsAnswer(answers[i]), records: settingsRecordsOf(lines, run)}
		if r.file != "" {
			got[i].state = settingsStateOf(run, r.file)
		}
	}
	return before, got
}

// run asks r of the pair and judges it (settingsJudge); a row of a recorded difference after its reference row,
// through settingsRecordedJudge.
func (r settingsRow) run(t *testing.T, p *Pair) {
	t.Helper()
	report := func(what, oracle string, diffs []string) {
		t.Helper()
		if oracle != "" {
			t.Fatalf("%soracle: %s", what, oracle)
		}
		for _, d := range diffs {
			t.Errorf("%s%s", what, d)
		}
	}
	if r.recorded == nil {
		_, got := settingsAsk(t, p, r)
		oracle, diffs := settingsJudge(r, got)
		report("", oracle, diffs)
		return
	}
	_, ref := settingsAsk(t, p, *r.recorded)
	oracle, diffs := settingsJudge(*r.recorded, ref)
	report("the reference request: ", oracle, diffs)
	before, got := settingsAsk(t, p, r)
	oracle, taken, diffs := settingsRecordedJudge(r, got, before, ref[0])
	report("", oracle, diffs)
	if taken != "" {
		t.Logf("recorded difference: the candidate %s", taken)
	}
}

// settingsRowsProblem is what is wrong with a pair's rows as a table ("" when nothing): a row without a name of its
// own, with a `/` in it (Go would split the subtest's name) or without a guard on the oracle's answer; a state or
// laid files without a file to judge; a row of a recorded difference whose name does not end in `-recorded`, that
// names no file, whose reference is itself recorded, lacks a guard or is about another file, or whose file another
// row uses (the two sides may hold it differently after the row: nothing may depend on it); a row whose name says
// `recorded` and that is none.
func settingsRowsProblem(rows []settingsRow) string {
	names, files, recorded := map[string]bool{}, map[string]int{}, map[string]string{}
	guarded := func(r settingsRow) bool { return r.want[0] != "" && strings.HasPrefix(r.want[1], "\r\n\r\n") }
	for _, r := range rows {
		switch {
		case r.name == "" || strings.Contains(r.name, "/") || names[r.name]:
			return fmt.Sprintf("row %q: every row needs a name of its own, without a slash", r.name)
		case !guarded(r):
			return fmt.Sprintf("row %s has no guard on the oracle's answer", r.name)
		case r.file == "" && (r.state != settingsState{} || r.lay != nil):
			return fmt.Sprintf("row %s has a state or lays files out and names no file to judge", r.name)
		case r.recorded == nil && strings.Contains(r.name, "recorded"):
			return fmt.Sprintf("row %s says it is recorded and has no reference request", r.name)
		}
		names[r.name] = true
		files[r.file]++
		if ref := r.recorded; ref != nil {
			switch {
			case !strings.HasSuffix(r.name, "-recorded"):
				return fmt.Sprintf("row %s compares a recorded difference: its name must end in -recorded", r.name)
			case r.file == "":
				return fmt.Sprintf("row %s compares a recorded difference and names no file", r.name)
			case ref.recorded != nil || !guarded(*ref) || ref.file != r.file:
				return fmt.Sprintf("row %s: its reference must be a guarded row on %s that is not itself recorded",
					r.name, r.file)
			}
			recorded[r.file] = r.name
		}
	}
	for file, name := range recorded {
		if files[file] != 1 {
			return fmt.Sprintf("row %s compares a recorded difference on %s, which %d other rows use: the two sides "+
				"may hold it differently after it", name, file, files[file]-1)
		}
	}
	return ""
}

// settingsPair is dashPair's options with fnWriteTokens' two bearer tokens laid out before each side starts (the
// admin's and the member's rows).
func settingsPair(t *testing.T) *Pair {
	t.Helper()
	return startPairWith(t, dashPairOptions(daemon.Options{}), parentIdentity, binaries(t), [2]string{},
		[2]Role{Oracle, Candidate}, fnWriteTokens)
}

// What C answers `/api/v3/settings` with (api_v3_settings.c:288-364): an nRPC body for every PUT and every error
// (json-c-parser-inline.c:43-53), the stored file as it is for a GET, `{"version":1}` when there is none (:75-80).
// A PUT stores its payload with the version raised by one, as json-c prints it (:233-238).
const (
	setPath    = "/api/v3/settings"
	setOK      = `{"status":200,"errorMessage":"OK"}`
	setFresh   = `{"version":1}`
	setInvalid = `{"status":400,"errorMessage":"Invalid settings file given."}`
	setMode    = `{"status":400,"errorMessage":"Invalid HTTP mode. HTTP modes GET and PUT are supported."}`
	// the handler's other refusals, in its order (:327-337, :345-349, :193-231, :244-280)
	setAgentOnly = `{"status":400,"errorMessage":"Settings API is only allowed for the agent node."}`
	setAnonymous = `{"status":400,"errorMessage":"Only the 'default' settings file is allowed for anonymous users"}`
	setNoPayload = `{"status":400,"errorMessage":"Settings API PUT action requires a payload."}`
	setNoPath    = `{"status":400,"errorMessage":"Settings path cannot be created or accessed."}`
	setUnparsed  = `{"status":400,"errorMessage":"Payload cannot be parsed as a JSON object"}`
	setNoVersion = `{"status":400,"errorMessage":"Field version is not found in payload"}`
	setStale     = `{"status":409,"errorMessage":"Payload version does not match the version of the stored object"}`
	setNoCreate  = `{"status":500,"errorMessage":"Cannot create payload file"}`
	setNoMove    = `{"status":500,"errorMessage":"Failed to move the payload file to its final location"}`
	// the router's, before the handler (web_api.c:72-77: a command without subpaths; :103-106: no such command in
	// versions 1 and 2)
	setSubpath     = `API command 'settings' does not support subpaths.`
	setUnsupported = `Unsupported API command: settings`

	setStatusOK       = "HTTP/1.1 200 OK\r\n"
	setStatusBad      = "HTTP/1.1 400 Bad Request\r\n"
	setStatusMissing  = "HTTP/1.1 404 Not Found\r\n"
	setStatusConflict = "HTTP/1.1 409 Conflict\r\n"
	setStatusError    = "HTTP/1.1 500 Internal Server Error\r\n"

	// the modes C gives the settings directory (one mkdir of 0750, :193, paths.c:182-193) and a file it stores (a
	// `.new` created 0666 under the daemon's umask 0007, :136, daemon.c:482, then renamed, :268)
	setDirMode  = "drwxr-x---"
	setFileMode = "-rw-rw----"
)

// The dashboard's PUT of its preferred nodes, the fixture child (installed v3 app), on the fresh file and on its second
// version (setPutV1, setPutV2), and the file C stores for each, the version raised by one, as json-c prints it (setV2,
// setV3).
var (
	setPutV1 = []byte(`{"version":1,"value":{"preferred_node_ids":["` + childHost.MachineGUID + `"]}}`)
	setPutV2 = []byte(`{"version":2,"value":{"preferred_node_ids":["` + childHost.MachineGUID + `"]}}`)
	setV2    = `{ "version": 2, "value": { "preferred_node_ids": [ "` + childHost.MachineGUID + `" ] } }`
	setV3    = `{ "version": 3, "value": { "preferred_node_ids": [ "` + childHost.MachineGUID + `" ] } }`
)

// setOther is what the committed `admin-put` row stores in the admin's file `other`: json-c's escapes, numbers and
// empty containers (api_v3_settings.c:238).
const setOther = `{ "version": 2, "value": { "url": "http:\/\/x\/y", "n": 1.50, "on": true, "off": null, ` +
	`"list": [ ] } }`

// setRow is a settings row whose answer the oracle must give with the status line and the body exactly.
func setRow(name, method, target string, body []byte, status, answer string, headers ...string) exactReq {
	return exactReq{name: name, method: method, target: target, body: body, headers: headers,
		want: [2]string{status, "\r\n\r\n" + answer}}
}

// setFile is the target of the settings file `name`.
func setFile(name string) string { return setPath + "?file=" + name }

// setGet is a GET row of target: the status line and the body the oracle must answer.
func setGet(name, target, status, answer string, headers ...string) settingsRow {
	return settingsRow{exactReq: setRow(name, "", target, nil, status, answer, headers...)}
}

// setPut is a PUT row of body to target (an empty body is sent with `Content-Length: 0`).
func setPut(name, target, body, status, answer string, headers ...string) settingsRow {
	return settingsRow{exactReq: setRow(name, "PUT", target, []byte(body), status, answer, headers...)}
}

// on names the row's file and the state the oracle's directory must hold for it after the row.
func (r settingsRow) on(file string, state settingsState) settingsRow {
	r.file, r.state = file, state
	return r
}

// laid gives the row what it lays out in each side's settings directory before its request.
func (r settingsRow) laid(lay func(dir string) error) settingsRow {
	r.lay = lay
	return r
}

// logs gives the row the settings records the oracle must write for it.
func (r settingsRow) logs(records ...string) settingsRow {
	r.records = records
	return r
}

// setHeld is the state of a file with this mode and these bytes, in a settings directory as C makes it.
func setHeld(mode, data string) settingsState {
	return settingsState{dir: setDirMode, mode: mode, data: data}
}

// setStored is the state of a file C stored (or the harness laid out with C's mode).
func setStored(data string) settingsState { return setHeld(setFileMode, data) }

// setNoFile is the state of a file that is not there, in a settings directory that is.
var setNoFile = settingsState{dir: setDirMode}

// setVersionRecord are the parts of C's record of a stored file that gives no version (api_v3_settings.c:96-101;
// a PUT writes it too, reading the stored version, :201): an error of the daemon's web thread without an errno,
// carrying the request.
func setVersionRecord(method, file string) string {
	return `source=daemon level=error tid=N thread=WEB[n] … !errno= … req_method=` + method + ` … request="` +
		setFile(file) + `" msg="file '<RUN>/lib/settings/` + file + `' cannot be parsed to extract version"`
}

// setRangeRecord is setVersionRecord for a stored file whose last number is beyond the doubles (`1e999`): reading it,
// the system's json-c leaves ERANGE in errno, and C's record carries whatever errno its thread has (nd_log.c:429).
func setRangeRecord(method, file string) string {
	return `source=daemon level=error errno="34, Numerical result out of range" tid=N thread=WEB[n] … req_method=` +
		method + ` … request="` + setFile(file) + `" msg="file '<RUN>/lib/settings/` + file +
		`' cannot be parsed to extract version"`
}

// setFailRecord are the parts of C's record of a failed open or rename of `<file>.new` (api_v3_settings.c:247,
// :275): the errno, and the text that follows `cannot `.
func setFailRecord(errno, file, text string) string {
	return `source=daemon level=error errno="` + errno + `" tid=N thread=WEB[n] … req_method=PUT … request="` +
		setFile(file) + `" msg="cannot ` + text + `"`
}

// setLayFile lays a file with these bytes and exactly this mode into the settings directory, which is made as C
// makes it when it is not there.
func setLayFile(name, data string, mode fs.FileMode) func(dir string) error {
	return func(dir string) error {
		if err := os.Mkdir(dir, 0o750); err == nil {
			if err := os.Chmod(dir, 0o750); err != nil {
				return err
			}
		} else if !errors.Is(err, fs.ErrExist) {
			return err
		}
		path := filepath.Join(dir, name)
		if err := os.WriteFile(path, []byte(data), 0o600); err != nil {
			return err
		}
		return os.Chmod(path, mode)
	}
}

// setLayDirs makes directories (mode 0750) in the settings directory, each under its parent.
func setLayDirs(names ...string) func(dir string) error {
	return func(dir string) error {
		for _, name := range names {
			if err := os.Mkdir(filepath.Join(dir, name), 0o750); err != nil {
				return err
			}
			if err := os.Chmod(filepath.Join(dir, name), 0o750); err != nil {
				return err
			}
		}
		return nil
	}
}

// setLayLink makes `name` a link to target in the settings directory.
func setLayLink(name, target string) func(dir string) error {
	return func(dir string) error { return os.Symlink(target, filepath.Join(dir, name)) }
}

// setLayRemove removes entries of the settings directory, which must be there.
func setLayRemove(names ...string) func(dir string) error {
	return func(dir string) error {
		for _, name := range names {
			if err := os.Remove(filepath.Join(dir, name)); err != nil {
				return err
			}
		}
		return nil
	}
}

// setLayAll lays out one thing after another.
func setLayAll(lays ...func(dir string) error) func(dir string) error {
	return func(dir string) error {
		for _, lay := range lays {
			if err := lay(dir); err != nil {
				return err
			}
		}
		return nil
	}
}

// setStores are an admin's two rows on a file of its own: a PUT of payload (to a file that is not there: version
// 1), which C must store as `stored`, then a GET, which must return those bytes: each side reads back what it
// stored (the stored version is read from the file, api_v3_settings.c:96-98).
func setStores(file, payload, stored string) []settingsRow {
	return []settingsRow{
		setPut(file, setFile(file), payload, setStatusOK, setOK, dcAdmin).on(file, setStored(stored)),
		setGet(file+"-get", setFile(file), setStatusOK, stored, dcAdmin).on(file, setStored(stored)),
	}
}

// setRefuses is an admin's PUT row on a file of its own that C refuses, storing nothing.
func setRefuses(file, payload, status, answer string) settingsRow {
	return setPut(file, setFile(file), payload, status, answer, dcAdmin).on(file, setNoFile)
}

// settingsCommitted are the check's first twenty rows, asked in order (the file is state): GETs and PUTs of the
// default file through its versions (409 for a stale version), the payload and file-name errors (400), the other
// methods, a routed host, and an admin's other file.
func settingsCommitted() []settingsRow {
	def := setFile("default")
	put := func(name string, body []byte, status, answer, content string) settingsRow {
		return settingsRow{exactReq: setRow(name, "PUT", def, body, status, answer)}.on("default", setStored(content))
	}
	return []settingsRow{
		// a GET makes neither the directory nor the file (api_v3_settings.c:82-109)
		setGet("get-fresh", def, setStatusOK, setFresh).on("default", settingsState{}),
		put("put-v1", setPutV1, setStatusOK, setOK, setV2),
		setGet("get-v2", def, setStatusOK, setV2),
		// the stored version is 2 now (C bumps it on every PUT, :233-235)
		put("put-stale", setPutV1, setStatusConflict, setStale, setV2),
		put("put-v2", setPutV2, setStatusOK, setOK, setV3),
		setGet("get-v3", def, setStatusOK, setV3),
		put("put-not-json", []byte("not json"), setStatusBad, setUnparsed, setV3),
		put("put-no-version", []byte(`{"value":1}`), setStatusBad, setNoVersion, setV3),
		put("put-array", []byte(`[1]`), setStatusBad, setNoVersion, setV3),
		put("put-empty", []byte{}, setStatusBad, setNoPayload, setV3),
		setGet("get-other", setFile("other"), setStatusBad, setAnonymous),
		setGet("get-dot", setFile("a.b"), setStatusBad, setInvalid),
		setGet("get-dotdot", setFile("../x"), setStatusBad, setInvalid),
		setGet("get-none", setPath, setStatusBad, setInvalid),
		// a POST would store a version 4 if it were taken for a PUT
		settingsRow{exactReq: setRow("post", "POST", def, []byte(`{"version":3}`), setStatusBad, setMode)}.
			on("default", setStored(setV3)),
		settingsRow{exactReq: setRow("delete", "DELETE", def, nil, setStatusBad, setMode)}.
			on("default", setStored(setV3)),
		setGet("host", "/host/"+childHost.Hostname+def, setStatusBad, setAgentOnly),
		setGet("admin-get", setFile("other"), setStatusOK, setFresh, dcAdmin),
		// the stored bytes are json-c's (:238): its escapes, numbers and empty containers as GET returns them
		setPut("admin-put", setFile("other"),
			`{"version":1,"value":{"url":"http://x/y","n":1.50,"on":true,"off":null,"list":[]}}`, setStatusOK, setOK,
			dcAdmin).on("other", setStored(setOther)),
		setGet("admin-get2", setFile("other"), setStatusOK, setOther, dcAdmin),
	}
}

// settingsPrintRows: what json-c reads of JSON and how it prints it back, each on a file of the admin's own (D234
// F13). The stored bytes are what C stored (C against C, 2026-10-08); the comments say what each row holds a reader
// or a printer to.
func settingsPrintRows() []settingsRow {
	deep := func(n int) string {
		return `{"version":1,"d":` + strings.Repeat("[", n) + "1" + strings.Repeat("]", n) + "}"
	}
	return slices.Concat(
		// a string's bytes: a raw `é`, a `\u` escape and a surrogate pair decoded to UTF-8, `\u0001` kept as an
		// escape, a raw tab printed `\t`, `\"` and `\\` kept, 0x7f raw, `/` printed `\/`
		setStores("esc", "{\"version\":1,\"s\":\"\u00e9\\u00e9\\ud83d\\ude00\\u0001\t\\\"\\\\\x7f/\"}",
			"{ \"version\": 2, \"s\": \"\u00e9\u00e9\U0001F600\\u0001\\t\\\"\\\\\x7f\\/\" }"),
		// a key is printed as a string is
		setStores("esc-key", "{\"version\":1,\"k\u00e9\\u00e9\t\\n\x7f/\\\"\":1}",
			"{ \"version\": 2, \"k\u00e9\u00e9\\t\\n\x7f\\/\\\"\": 1 }"),
		// the five short escapes are printed back; of the `\u` escapes only those below 0x20 stay escapes
		// (U+007F, U+0080, U+00FF, U+2028 and U+FFFF are stored as UTF-8)
		setStores("esc-short", `{"version":1,"s":"\b\f\n\r\t\/\u001f\u007f\u0080\u00ff\u2028\uffff"}`,
			"{ \"version\": 2, \"s\": \"\\b\\f\\n\\r\\t\\/\\u001f\x7f\u0080\u00ff\u2028\uffff\" }"),
		// a NUL an escape brought into a value stays, printed as the escape
		setStores("nul-value", `{"version":1,"s":"a\u0000b"}`, `{ "version": 2, "s": "a\u0000b" }`),
		// bytes that are no UTF-8 are stored as they came
		setStores("bad-utf8", "{\"version\":1,\"s\":\"a\xffb\xc3\"}", "{ \"version\": 2, \"s\": \"a\xffb\xc3\" }"),
		// numbers: an integer is printed from its value (`-0` is 0; beyond 64 bits it is the nearest limit, and
		// one above the signed range is kept unsigned), a number with a fraction or an exponent keeps its text
		// (`1e400` and `-0.0` too)
		setStores("num", `{"version":1,"n":[-0,1e5,1E+5,18446744073709551616,-9223372036854775809,1e400,0.0,-0.0,`+
			`1.0e-2,9223372036854775807,9223372036854775808,18446744073709551615,-9223372036854775808,0,-1]}`,
			`{ "version": 2, "n": [ 0, 1e5, 1E+5, 18446744073709551615, -9223372036854775808, 1e400, 0.0, -0.0, `+
				`1.0e-2, 9223372036854775807, 9223372036854775808, 18446744073709551615, -9223372036854775808, 0, `+
				`-1 ] }`),
		// a repeated key keeps its first place and takes its last value; `version` is raised where it stands
		setStores("order", `{"z":1,"version":1,"a":{"b":1,"b":2},"z":3}`, `{ "z": 3, "version": 2, "a": { "b": 2 } }`),
		// the empty object and array, a two-item array, containers in containers
		setStores("containers", `{"version":1,"o":{},"a":[1,2],"e":[],"n":{"x":{}},"m":[[],{}]}`,
			`{ "version": 2, "o": { }, "a": [ 1, 2 ], "e": [ ], "n": { "x": { } }, "m": [ [ ], { } ] }`),
		// the payload's own blanks (space, tab, the two line ends) are not kept
		setStores("blanks", "  {  \"version\" : 1 ,\n \"a\" :\t[ 1 , 2 ]\r\n}  ", `{ "version": 2, "a": [ 1, 2 ] }`),
		// text after the root value is ignored
		setStores("tail", `{"version":1,"a":1} trailing text`, `{ "version": 2, "a": 1 }`),
		// a value inside 31 containers is read; inside 32 the payload is refused (json-c's depth)
		setStores("depth31", deep(30), `{ "version": 2, "d": `+strings.Repeat("[ ", 30)+"1"+strings.Repeat(" ]", 30)+
			" }"),
		[]settingsRow{setRefuses("depth32", deep(31), setStatusBad, setUnparsed)},
		// a surrogate escape without its pair is U+FFFD (D240 point 2; the review R105's run-only request 3), also
		// in the middle of a string, where what follows is read as it stands
		setStores("lone", `{"version":1,"value":"\ud83d"}`, "{ \"version\": 2, \"value\": \"\ufffd\" }"),
		setStores("lone-mid", `{"version":1,"value":"a\ud83db\ude00c\ud83d\ud83d\ude00"}`,
			"{ \"version\": 2, \"value\": \"a\ufffdb\ufffdc\ufffd\U0001F600\" }"),
		// a key ends at a NUL an escape brought (D240 point 2): `a\u0000b` is the key `a`, which the later `a`
		// then repeats; `version\u0000x` is the member `version`
		setStores("nul-key", `{"version":1,"a\u0000b":1,"a":2}`, `{ "version": 2, "a": 2 }`),
		setStores("nul-version", `{"version\u0000x":1}`, `{ "version": 2 }`),
		// the payload is a C string for json-c: it ends at its first NUL byte (R105's run-only request 7). After
		// the root that is text ignored; inside a string, or first, it leaves nothing that parses
		setStores("nul-junk", "{\"version\":1}\x00junk", `{ "version": 2 }`),
		[]settingsRow{
			setRefuses("nul-mid", "{\"version\":1,\"a\":\"x\x00y\"}", setStatusBad, setUnparsed),
			setRefuses("nul-first", "\x00{\"version\":1}", setStatusBad, setUnparsed),
			// a `/` after the root that opens no comment fails the whole text in json-c (D240 point 1; R105's
			// run-only request 2): 400, the directory there, no file
			setRefuses("slash", `{"version":1}/`, setStatusBad, setUnparsed),
			setRefuses("slash-blank", `{"version":1} /x`, setStatusBad, setUnparsed),
		},
	)
}

// settingsVersionRows: how the payload's `version` is read (json_object_get_int, api_v3_settings.c:223): each form
// on a file of the admin's own that is not there (the stored version is 1). The accepted ones are stored with the
// version 2 where the member stood; the others answer 409 and store nothing.
func settingsVersionRows() []settingsRow {
	payload := func(version string) string { return `{"a":0,"version":` + version + `,"z":9}` }
	var rows []settingsRow
	for _, v := range []struct{ file, version string }{
		{"v-str", `"1"`},     // a string by its digits (a strict reader refuses it)
		{"v-dbl", "1.9"},     // a double is truncated (rounded, it would be 2)
		{"v-true", "true"},   // a boolean is 0 or 1
		{"v-e0", "1e0"},      // a number with an exponent is a double
		{"v-stre3", `"1e3"`}, // a string is read as an integer: 1, not 1000
		{"v-strx", `" 1x"`},  // blanks before a string's digits and text after them are passed over
	} {
		rows = append(rows, setStores(v.file, payload(v.version), `{ "a": 0, "version": 2, "z": 9 }`)...)
	}
	for _, v := range []struct{ file, version string }{
		{"v-null", "null"}, {"v-obj", "{}"}, {"v-arr", "[1]"}, {"v-false", "false"}, {"v-strempty", `""`}, // 0
		{"v-zero", "0"},
		// beyond 32 bits a version is the nearest limit, not its low 32 bits (which are 1 in each of these)
		{"v-big", "4294967297"}, {"v-neg", "-4294967295"}, {"v-bigdbl", "4294967297.0"},
		{"v-e10", "1e10"}, // the plan's question 15
	} {
		rows = append(rows, setRefuses(v.file, payload(v.version), setStatusConflict, setStale))
	}
	return rows
}

// settingsStoredRows: stored files the agent did not write, the same bytes with the same mode laid into both sides'
// directory (each a file of the admin's own): a file that gives no version reads as `{"version":1}` with C's record,
// on a GET and on a PUT (which reads the stored version first), and a PUT of version 1 replaces it; a file that
// gives one is served byte for byte; and the version at the limits of C's int.
func settingsStoredRows() []settingsRow {
	var rows []settingsRow
	lay := func(file, text string) func(dir string) error { return setLayFile(file, text, 0o660) }
	get := func(name, file, answer, stored string, records ...string) settingsRow {
		return setGet(name, setFile(file), setStatusOK, answer, dcAdmin).on(file, setStored(stored)).logs(records...)
	}
	put := func(name, file, payload, status, answer, stored string, records ...string) settingsRow {
		return setPut(name, setFile(file), payload, status, answer, dcAdmin).on(file, setStored(stored)).
			logs(records...)
	}
	// no version: text that is no JSON, a version 0, an empty file, a root `null` (json-c's NULL), an array, an
	// object without the member, and `{"version":2}/` (json-c fails the text at the `/`: R105's run-only request 2)
	for _, c := range []struct{ file, text string }{
		{"st-notjson", "not json"}, {"st-v0", `{"version":0}`}, {"st-empty", ""}, {"st-null", "null"},
		{"st-arr", `[{"version":5}]`}, {"st-nomember", `{"value":1}`}, {"st-slash", `{"version":2}/`},
	} {
		getRecord, putRecord := setVersionRecord("GET", c.file), setVersionRecord("PUT", c.file)
		rows = append(rows,
			get(c.file+"-get", c.file, setFresh, c.text, getRecord).laid(lay(c.file, c.text)),
			// the stored version reads as 1: a PUT of the version the file may seem to hold is stale
			put(c.file+"-stale", c.file, `{"version":2}`, setStatusConflict, setStale, c.text, putRecord),
			put(c.file+"-put", c.file, `{"version":1}`, setStatusOK, setOK, `{ "version": 2 }`, putRecord),
			get(c.file+"-get2", c.file, `{ "version": 2 }`, `{ "version": 2 }`),
		)
	}
	// a version: the file is served as it is, a trailing newline, C's own layout or none
	pretty := "{\n  \"x\" : [1,2],\n  \"version\":5\n}\n"
	rows = append(rows,
		get("st-nl-get", "st-nl", "{\"version\":2}\n", "{\"version\":2}\n").laid(lay("st-nl", "{\"version\":2}\n")),
		put("st-nl-stale", "st-nl", `{"version":1}`, setStatusConflict, setStale, "{\"version\":2}\n"),
		put("st-nl-put", "st-nl", `{"version":2,"a":1}`, setStatusOK, setOK, `{ "version": 3, "a": 1 }`),
		get("st-pretty-get", "st-pretty", pretty, pretty).laid(lay("st-pretty", pretty)),
		put("st-pretty-put", "st-pretty", `{"version":5}`, setStatusOK, setOK, `{ "version": 6 }`),
		// a stored version that is a string is read by its digits
		get("st-strver-get", "st-strver", `{"version":"7"}`, `{"version":"7"}`).
			laid(lay("st-strver", `{"version":"7"}`)),
		put("st-strver-put", "st-strver", `{"version":7}`, setStatusOK, setOK, `{ "version": 8 }`),
		// the stored text is a C string for json-c and the answer is the file: bytes after a NUL are served
		get("st-nul-get", "st-nul", "{\"version\":2}\x00junk", "{\"version\":2}\x00junk").
			laid(lay("st-nul", "{\"version\":2}\x00junk")),
		put("st-nul-put", "st-nul", `{"version":2}`, setStatusOK, setOK, `{ "version": 3 }`),
	)
	// the version at the limits of C's int (:223, :233-235): the next of the highest is the lowest; a payload's
	// version above 32 bits reads as the highest, which the lowest is not; the next of the lowest
	rows = append(rows,
		get("st-max-get", "st-max", `{"version":2147483647}`, `{"version":2147483647}`).
			laid(lay("st-max", `{"version":2147483647}`)),
		put("st-max-put", "st-max", `{"version":2147483647,"a":1}`, setStatusOK, setOK,
			`{ "version": -2147483648, "a": 1 }`),
		get("st-max-get2", "st-max", `{ "version": -2147483648, "a": 1 }`, `{ "version": -2147483648, "a": 1 }`),
		put("st-max-above", "st-max", `{"version":2147483648}`, setStatusConflict, setStale,
			`{ "version": -2147483648, "a": 1 }`),
		put("st-max-lowest", "st-max", `{"version":-2147483648}`, setStatusOK, setOK, `{ "version": -2147483647 }`),
		// a stored version above 32 bits reads as the highest too: version 1 is stale, the highest is taken
		get("st-bigver-get", "st-bigver", `{"version":4294967297}`, `{"version":4294967297}`).
			laid(lay("st-bigver", `{"version":4294967297}`)),
		put("st-bigver-stale", "st-bigver", `{"version":1}`, setStatusConflict, setStale, `{"version":4294967297}`),
		put("st-bigver-put", "st-bigver", `{"version":2147483647}`, setStatusOK, setOK, `{ "version": -2147483648 }`),
		// the next of -1 is 0, which then reads as a file without a version
		get("st-neg-get", "st-neg", `{"version":-1}`, `{"version":-1}`).laid(lay("st-neg", `{"version":-1}`)),
		put("st-neg-put", "st-neg", `{"version":-1}`, setStatusOK, setOK, `{ "version": 0 }`),
		get("st-neg-get2", "st-neg", setFresh, `{ "version": 0 }`, setVersionRecord("GET", "st-neg")),
		put("st-neg-stale", "st-neg", `{"version":0}`, setStatusConflict, setStale, `{ "version": 0 }`,
			setVersionRecord("PUT", "st-neg")),
		put("st-neg-put2", "st-neg", `{"version":1}`, setStatusOK, setOK, `{ "version": 2 }`,
			setVersionRecord("PUT", "st-neg")),
	)
	// a file without a version whose last number is beyond the doubles: C's record carries the errno json-c's
	// reading left (ERANGE), on the GET and on the PUT that replaces the file
	ranged := `{"version":0,"a":1e999}`
	rows = append(rows,
		get("st-range-get", "st-range", setFresh, ranged, setRangeRecord("GET", "st-range")).laid(lay("st-range", ranged)),
		put("st-range-put", "st-range", `{"version":1}`, setStatusOK, setOK, `{ "version": 2 }`,
			setRangeRecord("PUT", "st-range")),
	)
	return rows
}

// settingsRecordedPut is a row of the recorded difference on a PUT (D234 F10, D240): a text json-c reads and the
// Rust reader may refuse, put to a file of the admin's own. C must answer 200 and store `stored`; the candidate may
// instead answer as C answers a payload that is no JSON at all (the reference: 400 `Payload cannot be parsed as a
// JSON object`), storing nothing.
func settingsRecordedPut(file, payload, stored string) settingsRow {
	ref := setRefuses(file, "not json", setStatusBad, setUnparsed)
	ref.name = file + "-reference"
	r := setPut(file+"-recorded", setFile(file), payload, setStatusOK, setOK, dcAdmin).on(file, setStored(stored))
	r.recorded = &ref
	return r
}

// settingsRecordedStored is a row of the recorded difference on a stored file (D234 F10: "a stored file that holds
// it reads as version 1"): a text json-c reads, laid into both sides' directory as a file of the admin's own. C must
// serve it as it is; the candidate may instead answer as C answers a stored file that is no JSON at all (the
// reference: `{"version":1}` and the record), the file left as it was. A GET serves the file's bytes whatever version
// was read of it, as long as it is not 0: these rows judge whether a reader takes the text, not what it reads of it
// (no row may follow on the file: settingsRowsProblem).
func settingsRecordedStored(file, text string) settingsRow {
	ref := setGet(file+"-reference", setFile(file), setStatusOK, setFresh, dcAdmin).on(file, setStored("not json")).
		laid(setLayFile(file, "not json", 0o660)).logs(setVersionRecord("GET", file))
	r := setGet(file+"-recorded", setFile(file), setStatusOK, text, dcAdmin).on(file, setStored(text)).
		laid(setLayFile(file, text, 0o660))
	r.recorded = &ref
	return r
}

// settingsRecordedRows: D234 F10's boundary as D240 corrects it, each form on a file of its own: what json-c reads
// beyond strict JSON, as a PUT's payload and as a stored file. The stored bytes are C's (C against C, 2026-10-08).
func settingsRecordedRows() []settingsRow {
	return []settingsRow{
		settingsRecordedPut("f10-comment", `{"version":1,/* c */"a":1}`, `{ "version": 2, "a": 1 }`),
		settingsRecordedPut("f10-line-comment", "{\"version\":1,// c\n\"a\":1}", `{ "version": 2, "a": 1 }`),
		// a comment after the root, which json-c reads and the Rust reader refuses with the `/` (D240 point 1)
		settingsRecordedPut("f10-tail-comment", `{"version":1}/* c */`, `{ "version": 2 }`),
		settingsRecordedPut("f10-quote", `{'version':1,'a':'b'}`, `{ "version": 2, "a": "b" }`),
		settingsRecordedPut("f10-comma", `{"version":1,"a":[1,2,],}`, `{ "version": 2, "a": [ 1, 2 ] }`),
		settingsRecordedPut("f10-nan", `{"version":1,"a":NaN,"b":Infinity,"c":-Infinity}`,
			`{ "version": 2, "a": NaN, "b": Infinity, "c": -Infinity }`),
		// leading zeros: json-c reads `01` as 1 and `007` as 7
		settingsRecordedPut("f10-zero", `{"version":01,"a":007}`, `{ "version": 2, "a": 7 }`),
		settingsRecordedPut("f10-case", `{"version":1,"a":TRUE,"b":Null,"c":False}`,
			`{ "version": 2, "a": true, "b": null, "c": false }`),
		// `1.` and `-.5` keep their text
		settingsRecordedPut("f10-dot", `{"version":1,"a":1.,"b":-.5}`, `{ "version": 2, "a": 1., "b": -.5 }`),
		// an exponent without digits: json-c drops the marker. Neither D234 F10's list nor D240 names this form; the
		// reader's vectors do (text/tests/vectors/jsonc_doc.tsv: `{"version":1e}`, which the reader may refuse)
		settingsRecordedPut("f10-exp", `{"version":1,"a":1e}`, `{ "version": 2, "a": 1 }`),
		settingsRecordedStored("f10-st-quote", `{'version':2}`),
		settingsRecordedStored("f10-st-comment", `/* c */{"version":2}`),
		settingsRecordedStored("f10-st-comma", `{"version":2,}`),
		settingsRecordedStored("f10-st-nan", `{"version":2,"a":NaN}`),
		settingsRecordedStored("f10-st-zero", `{"version":02}`),
		settingsRecordedStored("f10-st-case", `{"version":2,"a":TRUE}`),
		settingsRecordedStored("f10-st-dot", `{"version":2,"a":1.}`),
	}
}

// settingsFileRows: what is in the way of a file or of its `.new` (api_v3_settings.c:118-170, :240-280;
// inlined.h:610-657), each on a file of the admin's own, laid into both sides' directory; with the failure's record,
// which carries the errno (R105's run-only request 4), and the plan's question 17.
func settingsFileRows() []settingsRow {
	get := func(name, file, answer string, state settingsState) settingsRow {
		return setGet(name, setFile(file), setStatusOK, answer, dcAdmin).on(file, state)
	}
	put := func(name, file, payload, status, answer string, state settingsState) settingsRow {
		return setPut(name, setFile(file), payload, status, answer, dcAdmin).on(file, state)
	}
	tmpDir := setStored(`{ "version": 2 }`)
	tmpDir.tmp = setDirMode
	tmpLink := settingsState{dir: setDirMode, tmp: "Lrwxrwxrwx"}
	isDir := setHeld(setDirMode, "")
	// a document of n bytes, and how the check compares it
	sized := func(n int) (text, data string) {
		text = `{"version":5,"p":"` + strings.Repeat("a", n-len(`{"version":5,"p":""}`)) + `"}`
		return text, settingsData([]byte(text))
	}
	limit, limitData := sized(20 * 1024 * 1024)
	over, overData := sized(20*1024*1024 + 1)
	name251 := strings.Repeat("n", 251)
	return []settingsRow{
		// `<file>.new` is a directory: 500 and the record with EINVAL; nothing is removed, the file stays
		put("new-dir-put", "new-dir", `{"version":1}`, setStatusOK, setOK, setStored(`{ "version": 2 }`)),
		put("new-dir-refused", "new-dir", `{"version":2}`, setStatusError, setNoCreate, tmpDir).
			laid(setLayDirs("new-dir.new")).
			logs(setFailRecord("22, Invalid argument", "new-dir",
				"open/create settings file '<RUN>/lib/settings/new-dir.new'")),
		get("new-dir-get", "new-dir", `{ "version": 2 }`, tmpDir),
		// once the directory is gone the same PUT is stored
		put("new-dir-again", "new-dir", `{"version":2}`, setStatusOK, setOK, setStored(`{ "version": 3 }`)).
			laid(setLayRemove("new-dir.new")),
		// `<file>.new` is a link, to nothing: it is not followed (no file appears at its target) and it stays
		put("new-link-refused", "new-link", `{"version":1}`, setStatusError, setNoCreate, tmpLink).
			laid(setLayLink("new-link.new", "new-link-target")).
			logs(setFailRecord("22, Invalid argument", "new-link",
				"open/create settings file '<RUN>/lib/settings/new-link.new'")),
		get("new-link-target", "new-link-target", setFresh, setNoFile),
		// `<file>.new` is a regular file that cannot be written (mode 0): the open fails and the record carries
		// its errno, EACCES
		put("new-mode0-refused", "new-mode0", `{"version":1}`, setStatusError, setNoCreate,
			settingsState{dir: setDirMode, tmp: "----------"}).
			laid(setLayFile("new-mode0.new", "left behind", 0)).
			logs(setFailRecord("13, Permission denied", "new-mode0",
				"open/create settings file '<RUN>/lib/settings/new-mode0.new'")),
		// a name of 251 bytes is stored (its `.new` is a directory entry of 255 bytes, the most the run's
		// filesystem takes); one byte more and the `.new` cannot be looked at: the record carries ENAMETOOLONG
		put("name-251-put", name251, `{"version":1}`, setStatusOK, setOK, setStored(`{ "version": 2 }`)),
		put("name-252-refused", name251+"n", `{"version":1}`, setStatusError, setNoCreate, setNoFile).
			logs(setFailRecord("36, File name too long", name251+"n",
				"open/create settings file '<RUN>/lib/settings/"+name251+"n.new'")),
		// the final name is a directory that is not empty: a GET reads no file; a PUT writes `.new`, fails to
		// rename it (500, the record with EISDIR) and removes it
		get("name-dir-get", "name-dir", setFresh, isDir).laid(setLayDirs("name-dir", "name-dir/inside")),
		put("name-dir-refused", "name-dir", `{"version":1}`, setStatusError, setNoMove, isDir).
			logs(setFailRecord("21, Is a directory", "name-dir",
				"rename file '<RUN>/lib/settings/name-dir.new' to '<RUN>/lib/settings/name-dir'")),
		// a stale regular `<file>.new` of mode 0600 is reused, emptied, and gives the file its mode (the plan's
		// question 17); the next PUT makes a `.new` of its own
		put("stale-put", "stale", `{"version":1}`, setStatusOK, setOK, setHeld("-rw-------", `{ "version": 2 }`)).
			laid(setLayFile("stale.new", "left behind, and longer than the document that follows", 0o600)),
		get("stale-get", "stale", `{ "version": 2 }`, setHeld("-rw-------", `{ "version": 2 }`)),
		put("stale-again", "stale", `{"version":2}`, setStatusOK, setOK, setStored(`{ "version": 3 }`)),
		// the file is a link to a file: it is followed for reading; a PUT takes the link's place and leaves the
		// file it led to
		get("link-get", "link", `{"version":7,"t":1}`, setHeld("Lrwxrwxrwx", `{"version":7,"t":1}`)).
			laid(setLayAll(setLayFile("link-target", `{"version":7,"t":1}`, 0o660), setLayLink("link", "link-target"))),
		put("link-put", "link", `{"version":7}`, setStatusOK, setOK, setStored(`{ "version": 8 }`)),
		get("link-target", "link-target", `{"version":7,"t":1}`, setStored(`{"version":7,"t":1}`)),
		// a link to nothing reads as no file, without a record
		get("link-nothing-get", "link-nothing", setFresh, setHeld("Lrwxrwxrwx", "")).
			laid(setLayLink("link-nothing", "link-nowhere")),
		put("link-nothing-put", "link-nothing", `{"version":1}`, setStatusOK, setOK, setStored(`{ "version": 2 }`)),
		// a file of mode 0 (the agents do not run as root: were they, C would serve it and the guard would say so)
		// reads as no file, without a record: its version is stale, and a PUT of version 1 takes its place with
		// C's mode
		get("mode0-get", "mode0", setFresh, setHeld("----------", "")).laid(setLayFile("mode0", `{"version":5}`, 0)),
		put("mode0-stale", "mode0", `{"version":5}`, setStatusConflict, setStale, setHeld("----------", "")),
		put("mode0-put", "mode0", `{"version":1}`, setStatusOK, setOK, setStored(`{ "version": 2 }`)),
		// a stored file of 20 MiB is served (compared by length and MD5); one byte more reads as no file, without
		// a record (MAX_SETTINGS_SIZE_BYTES, :36, :91), its version stale
		get("limit-get", "limit", limitData, setStored(limitData)).laid(setLayFile("limit", limit, 0o660)),
		get("over-get", "over", setFresh, setStored(overData)).laid(setLayFile("over", over, 0o660)),
		put("over-stale", "over", `{"version":5}`, setStatusConflict, setStale, setStored(overData)),
		put("over-put", "over", `{"version":1}`, setStatusOK, setOK, setStored(`{ "version": 2 }`)),
	}
}

// settingsMemberRows: C asks only for a bearer token for another file than `default` (api_v3_settings.c:333): the
// member's token reads and writes one, in either header; a token nobody has is an anonymous client.
func settingsMemberRows() []settingsRow {
	stored := `{ "version": 2, "who": "member" }`
	unknown := "Authorization: Bearer b6b6b6b6-6666-4666-8666-0000000000ff"
	return []settingsRow{
		setGet("member-get", setFile("member"), setStatusOK, setFresh, dcMember).on("member", setNoFile),
		setPut("member-put", setFile("member"), `{"version":1,"who":"member"}`, setStatusOK, setOK, dcMember).
			on("member", setStored(stored)),
		setGet("member-get2", setFile("member"), setStatusOK, stored, dcMember),
		setGet("member-x-auth", setFile("member"), setStatusOK, stored, "X-Netdata-Auth: Bearer "+fnMemberToken.token),
		setGet("member-admin", setFile("member"), setStatusOK, stored, dcAdmin),
		setGet("unknown-token-get", setFile("member"), setStatusBad, setAnonymous, unknown),
		setPut("unknown-token-put", setFile("member"), `{"version":2}`, setStatusBad, setAnonymous, unknown).
			on("member", setStored(stored)),
	}
}

// settingsOrderRows: which of the handler's checks answers when two would (the file's name, the routed host, the
// anonymous client, the method, the payload: api_v3_settings.c:321-362), and how the query is read: the web server
// decodes it whole before the handler splits it (web_client.c:2119-2123), and the last `file` with a value wins
// (api_v3_settings.c:306-319). The default file is at its third version and the admin's `other` at its second
// (settingsCommitted).
func settingsOrderRows() []settingsRow {
	def, child := setFile("default"), "/host/"+childHost.Hostname
	req := func(name, method, target, body, answer string, headers ...string) settingsRow {
		return settingsRow{exactReq: setRow(name, method, target, []byte(body), setStatusBad, answer, headers...)}
	}
	is := func(name, target string) settingsRow { return setGet(name, target, setStatusOK, setV3) }
	not := func(name, target, answer string) settingsRow { return setGet(name, target, setStatusBad, answer) }
	return []settingsRow{
		// the name before the method, the host and the payload
		req("order-post-dot", "POST", setFile("a.b"), `{"version":3}`, setInvalid),
		not("order-child-dot", child+setFile("a.b"), setInvalid),
		setPut("order-empty-dot", setFile("a.b"), "", setStatusBad, setInvalid),
		// the host before the anonymous client, whatever the token and the method
		not("order-child-other", child+setFile("other"), setAgentOnly),
		setGet("order-child-admin", child+setFile("other"), setStatusBad, setAgentOnly, dcAdmin),
		setPut("order-child-put", child+def, `{"version":3}`, setStatusBad, setAgentOnly).on("default", setStored(setV3)),
		// the anonymous client before the method and the payload
		req("order-post-other", "POST", setFile("other"), `{"version":3}`, setAnonymous),
		setPut("order-empty-other", setFile("other"), "", setStatusBad, setAnonymous),
		// the method last: a POST of the stored version by the admin stores nothing
		req("order-post-admin", "POST", setFile("other"), `{"version":2}`, setMode, dcAdmin).
			on("other", setStored(setOther)),
		// the last `file` with a value wins
		not("query-last-other", def+"&file=other", setAnonymous),
		is("query-last-empty", def+"&file="),
		is("query-last-default", setFile("other")+"&file=default"),
		not("query-last-invalid", def+"&file=a.b", setInvalid),
		// an item is cut at its first `=`, an empty name before it passed over
		is("query-eq-first", setPath+"?=file=default"),
		not("query-eq-twice", setPath+"?file==default", setInvalid),
		not("query-eq-inside", setPath+"?file=a=b", setInvalid),
		// the query is decoded before it is split: an escaped byte of the name or of the value, an escaped `&`
		// (which then separates) and an escaped `=`
		is("query-percent-value", setPath+"?file=%64efault"),
		is("query-percent-name", setPath+"?%66ile=default"),
		is("query-percent-amp", setPath+"?file=other%26file=default"),
		is("query-percent-eq", setPath+"?file%3Ddefault"),
		// a `%00` ends the decoded query (R105's run-only request 7); a `%zz` is no name of an anonymous client's
		is("query-nul", def+"%00x"),
		not("query-bad-escape", setFile("%zz"), setAnonymous),
		// (C's decoder makes the one byte `3` of it, url.c:10-13, :64-67: with a token, that is the file)
		setPut("query-bad-escape-put", setFile("%zz"), `{"version":1}`, setStatusOK, setOK, dcAdmin).
			on("3", setStored(`{ "version": 2 }`)),
		// the parameter's name and the anonymous client's `default` are compared as they are written
		not("query-upper-name", setPath+"?FILE=default", setInvalid),
		not("query-upper-value", setFile("Default"), setAnonymous),
		// other parameters and empty items are passed over; no parameter at all
		is("query-others", setPath+"?x=1&file=default&y"),
		is("query-amps", setPath+"?&&file=default&&"),
		not("query-empty", setPath+"?", setInvalid),
		// a blank after the name, a byte above ASCII
		not("query-blank", def+"%20", setInvalid),
		not("query-utf8", setFile("d\u00e9fault"), setInvalid),
		// a name of letters, digits, `-` and `_`, and one of dashes alone
		setGet("name-dash-get", setFile("a-b_C9"), setStatusOK, setFresh, dcAdmin).on("a-b_C9", setNoFile),
		setPut("name-dash-put", setFile("a-b_C9"), `{"version":1}`, setStatusOK, setOK, dcAdmin).
			on("a-b_C9", setStored(`{ "version": 2 }`)),
		setGet("name-dash-get2", setFile("a-b_C9"), setStatusOK, `{ "version": 2 }`, dcAdmin),
		setGet("name-dashes", setFile("--"), setStatusOK, setFresh, dcAdmin).on("--", setNoFile),
		// localhost by its own name and by its node id is the agent node
		is("host-self", "/host/"+parentIdentity.Hostname+def),
		is("node-self", "/node/"+parentIdentity.MachineGUID+def),
		// the router's answers before the handler: no subpath; no such command in the other versions
		not("path-sub", setPath+"/x?file=default", setSubpath),
		not("path-slash", setPath+"/?file=default", setSubpath),
		setGet("path-v1", "/api/v1/settings?file=default", setStatusMissing, setUnsupported),
		setGet("path-v2", "/api/v2/settings?file=default", setStatusMissing, setUnsupported),
		setPut("path-v2-put", "/api/v2/settings?file=default", `{"version":3}`, setStatusMissing, setUnsupported).
			on("default", setStored(setV3)),
	}
}

// settingsFirstRows are the rows of the pair of its own for the first PUT: nothing before the handler's own PUT
// makes the settings directory; a regular file in its place is the handler's 400, before the payload is looked at;
// a first PUT that fails inside the handler (a payload that does not parse) has made the directory, mode 0750, and
// no file. After it: the default file's record for an anonymous client, and a link at the settings path.
func settingsFirstRows() []settingsRow {
	def := setFile("default")
	nothing, asFile := settingsState{}, settingsState{dir: "-rw-r-----"}
	// more parts of a record: the client of a request without a token
	const anonymous = " … role=none permissions=0x8 … !account= … !user="
	layPathFile := func(dir string) error {
		if err := os.WriteFile(dir, []byte("no directory"), 0o600); err != nil {
			return err
		}
		return os.Chmod(dir, 0o640)
	}
	return []settingsRow{
		setGet("get-fresh", def, setStatusOK, setFresh).on("default", nothing),
		// the payload's absence and the method are refused before the directory is made
		setPut("put-empty", def, "", setStatusBad, setNoPayload).on("default", nothing),
		settingsRow{exactReq: setRow("post", "POST", def, []byte(`{"version":1}`), setStatusBad, setMode)}.
			on("default", nothing),
		// the settings path is a regular file: the 400, also for a payload that would not parse; a GET reads no file
		setPut("path-file-put", def, `{"version":1}`, setStatusBad, setNoPath).on("default", asFile).laid(layPathFile),
		setPut("path-file-unparsed", def, "not json", setStatusBad, setNoPath).on("default", asFile),
		setGet("path-file-get", def, setStatusOK, setFresh).on("default", asFile),
		// the first PUT that reaches the handler's own work makes the directory, though its payload does not parse
		setPut("put-unparsed", def, "not json", setStatusBad, setUnparsed).on("default", setNoFile).
			laid(func(dir string) error { return os.Remove(dir) }),
		setGet("get-after", def, setStatusOK, setFresh).on("default", setNoFile),
		setPut("put-v1", def, `{"version":1}`, setStatusOK, setOK).on("default", setStored(`{ "version": 2 }`)),
		// the record of a stored file without a version carries the anonymous client too (no account, no user)
		setGet("anonymous-get", def, setStatusOK, setFresh).on("default", setStored("not json")).
			laid(setLayFile("default", "not json", 0o660)).logs(setVersionRecord("GET", "default") + anonymous),
		setPut("anonymous-put", def, `{"version":1}`, setStatusOK, setOK).on("default", setStored(`{ "version": 2 }`)).
			logs(setVersionRecord("PUT", "default") + anonymous),
		// the settings path is a link to a directory: it is one (paths.c:186 follows the link)
		setPut("path-link-put", def, `{"version":2}`, setStatusOK, setOK).
			on("default", settingsState{dir: "Lrwxrwxrwx", mode: setFileMode, data: `{ "version": 3 }`}).
			laid(func(dir string) error {
				if err := os.Rename(dir, dir+"-real"); err != nil {
					return err
				}
				return os.Symlink("settings-real", dir)
			}),
		// a link to nothing is no directory, and none can be made in its place: the 400; a GET reads no file
		setPut("path-dangling-put", def, `{"version":1}`, setStatusBad, setNoPath).
			on("default", settingsState{dir: "Lrwxrwxrwx"}).
			laid(func(dir string) error {
				if err := os.Remove(dir); err != nil {
					return err
				}
				return os.Symlink("settings-nowhere", dir)
			}),
		setGet("path-dangling-get", def, setStatusOK, setFresh).on("default", settingsState{dir: "Lrwxrwxrwx"}),
	}
}

// settingsPairs are the check's pairs of agents, each with rows of its own (settingsRowsOf).
var settingsPairs = []string{"rows", "first"}

// settingsRowsOf are the rows of one of the check's pairs: `rows` (the committed twenty, then the rows on other
// files, which leave the committed rows' state alone) and `first`.
func settingsRowsOf(pair string) []settingsRow {
	if pair == "first" {
		return settingsFirstRows()
	}
	return slices.Concat(settingsCommitted(), settingsPrintRows(), settingsVersionRows(), settingsStoredRows(),
		settingsRecordedRows(), settingsFileRows(), settingsMemberRows(), settingsOrderRows())
}

// TestSettingsAPI (check `api.settings`, M10 commit 7, D224, D234 F13): `/api/v3/settings`, the dashboard's
// per-agent settings file, asked in order on one pair (the file is state): the committed twenty rows (GETs and PUTs
// of the default file through its versions, the 400s, the other methods, a routed host, an admin's other file),
// then rows on files of their own: what json-c reads and prints, the forms of `version`, stored files the agent did
// not write, D234 F10's recorded difference, what is in the way of a file or of its `.new`,
// the member's token, the checks' order and the query's forms. A second pair (`first`) holds the first PUT. Every
// row is judged by settingsJudge: the oracle's answer, its file's state (bytes, modes, `<file>.new`) and its
// settings records in daemon.log, then the candidate's against the oracle's; a row of a recorded difference by
// settingsRecordedJudge. Answers are compared raw after maskAnswer (each side's clock and expiry read against its
// own flight: an nRPC answer to a PUT expires a second after its Date, json-c-parser-inline.c:49-50). Health is off.
func TestSettingsAPI(t *testing.T) {
	for _, pair := range settingsPairs {
		t.Run(pair, func(t *testing.T) {
			rows := settingsRowsOf(pair)
			if problem := settingsRowsProblem(rows); problem != "" {
				t.Fatalf("harness: %s", problem)
			}
			p := settingsPair(t)
			if pair == "rows" {
				dashChild(t, p, dashBase())
			}
			for _, r := range rows {
				t.Run(r.name, func(t *testing.T) { r.run(t, p) })
			}
		})
	}
	t.Run("access", func(t *testing.T) { accessRows(t, []accessConf{accessACL, accessBearer}, setAccessRows) })
}

// setAccessRows: the route's ACL is the dashboard's (web_api_v3.c:188-197), so the `acl` client is refused before
// the handler (web_api.c:82-84); its access is ANONYMOUS_DATA, which an anonymous client lacks under bearer protection
// (web_api.c:21-28, :86-89). Both come before the handler, whatever the method: the GET is accessRoutes' shape, the
// PUT is written here.
var setAccessRows = func() []accessRow {
	put := rawRequest("PUT", setPath+"?file=default", []string{"Host: localhost"}, []byte(setFresh))
	return append(accessRoutes(setPath+"?file=default"),
		accessRow{conf: accessACL.name, name: "v3-settings-put", request: put, want: accessDenied},
		accessRow{conf: accessBearer.name, name: "v3-settings-put", request: put, want: accessAnonymous})
}()
