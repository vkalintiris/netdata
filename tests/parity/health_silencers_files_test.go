// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"cmp"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"
)

// The two files of the management API as an agent finds them at its start (check `health.silencers`, M9 commit 7,
// D210): the key's file (`key-*`, api_v1_manage.c:7-123) and the silencers' (`file-*`, health_silencers.c:425-459).
// An agent reads each once, so each state is a case of its own: both agents start on the state, with no chart, and
// the case compares what stands at the file's path afterwards, what the route answers, and, both stopped, the records
// of the start.

// a management key as an agent makes one: a UUID in lower case (api_v1_manage.c:48-52)
var healthKeyRe = regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`)

// What a key file holds, as keyState names it.
const (
	healthKeyLaid = "the text laid out before the start"
	healthKeyNew  = "a UUID in lower case that was not laid out"
	// healthKeyMade is the file of a key the agent made and saved (api_v1_manage.c:63-87)
	healthKeyMade = "the key file: a regular file of 36 bytes, mode 0600: " + healthKeyNew
	// healthKeyKept is the file of the fixed key, left as it was laid out
	healthKeyKept = "the key file: a regular file of 36 bytes, mode 0600: " + healthKeyLaid
)

// healthKeyStateOf is what stands at a key file's path: nothing, a directory, a symbolic link with its target, or a
// regular file with its size, its mode and what it holds: the text `laid` out before the start, a key that was not
// (the agent's own: its value is each side's), or another text.
func healthKeyStateOf(file, laid string) string {
	st, err := os.Lstat(file)
	switch {
	case os.IsNotExist(err):
		return "no file"
	case err != nil:
		return "not readable: " + err.Error()
	case st.Mode()&os.ModeSymlink != 0:
		target, _ := os.Readlink(file)
		return "a symbolic link to " + target
	case st.IsDir():
		return "a directory"
	case !st.Mode().IsRegular():
		return "a " + st.Mode().Type().String()
	}
	what := ""
	switch b, err := os.ReadFile(file); {
	case err != nil:
		what = "not readable"
	case string(b) == laid:
		what = healthKeyLaid
	case healthKeyRe.Match(b):
		what = healthKeyNew
	default:
		what = "the text " + strconv.Quote(string(b))
	}
	return fmt.Sprintf("a regular file of %d bytes, mode %04o: %s", st.Size(), st.Mode().Perm(), what)
}

// keyState is side i's key file as the checks compare it (healthKeyStateOf); for a case whose netdata.conf names
// another file, C's default one too.
func (h *healthPair) keyState(i int) []string {
	run := h.p.Each()[i].Daemon.Opts.RunDir
	laid := cmp.Or(h.c.key, healthKey)
	out := []string{"the key file: " + healthKeyStateOf(filepath.Join(run, cmp.Or(h.c.keyFile, healthKeyFile)), laid)}
	if h.c.keyFile != "" {
		out = append(out, "the default key file: "+healthKeyStateOf(filepath.Join(run, healthKeyFile), laid))
	}
	return out
}

// compareKeyFile compares both sides' key files (keyState); the oracle's must be as `want` says.
func (h *healthPair) compareKeyFile(t *testing.T, want ...string) {
	t.Helper()
	h.compareLines(t, "the key file", h.keyState, func(oracle []string) error {
		if !slices.Equal(oracle, want) {
			return fmt.Errorf("want %q", want)
		}
		return nil
	})
}

// sessionKey is the key side i tells its log when it could not save one (api_v1_manage.c:120-121): the only place
// it is. The record is waited for 5 s.
func (h *healthPair) sessionKey(t *testing.T, i int) string {
	t.Helper()
	s := h.p.Each()[i]
	for end := time.Now().Add(5 * time.Second); ; time.Sleep(100 * time.Millisecond) {
		for _, l := range logLines(t, s.Daemon.Opts.RunDir, "daemon.log") {
			if m := healthSessionKeyRe.FindStringSubmatch(l); m != nil {
				return m[2]
			}
		}
		if time.Now().After(end) {
			t.Fatalf("%s: no record of a session key in daemon.log", s.Role)
		}
	}
}

// The records of the key's file, as the guards name them (api_v1_manage.c:28-121, registry_internals.c:83-86).
const (
	healthRecKeyNotRead    = `level=error … !errno= … msg="Failed to read management API key from '<RUN>/lib/netdata.api.key'"`
	healthRecKeyNotRegular = `level=error … !errno= … msg="Management API key file '<RUN>/lib/netdata.api.key' is not a regular file."`
	healthRecKeyNotSaved   = `level=error errno="13, Permission denied" … msg="Cannot create unique management API key file '<RUN>/lib/netdata.api.key'. Please adjust config parameter 'netdata management api key file' to a proper path and file."`
	healthRecKeySession    = `level=info … !errno= … msg="You can still continue to use the alarm management API using the authorization token <KEY> during this Netdata session only."`
	healthRecKeyNoGUID     = `level=info … !errno= … msg="Registry: GUID '' is not a valid GUID."`
	healthRecKeyNotValid   = `level=error … !errno= … msg="Failed to validate management API key '' from '<RUN>/lib/netdata.api.key'."`
)

// the `key-text` case's file: 51 bytes, of which C reads 36, and no UUID
const healthKeyNoUUID = "this text is no key and is longer than a key is ..."

// healthKeyCases are the `key-*` cases: one state of the key file each (healthCase.key), with health on and no
// chart. C reads the file when `lstat` says it is a regular file: its first 36 bytes, which it takes for the key
// unless its validation says no: a UUID in any letter case is kept in lower case, and the file is left as it is. The
// validation refuses an empty text only (`key-nul`); any other text that is no UUID passes it, and leaves the agent
// with a key nobody knows (`key-text`). A file that is not regular, is shorter, or was refused makes a new random
// key, which is saved to the file when that is a regular file or can be created, and else lives for the session only,
// told to the log. Each case compares the file's state after the start, the route's answers to the key the agent
// holds and to the others, the file again, and the records.
func healthKeyCases() map[string]healthCase {
	upper := strings.ToUpper(healthKey)
	type keyCase struct {
		c healthCase
		// state is what stands at the key file's path after the start; key, where the agent's key is: "file" (what
		// the file holds), "session" (the log's), or the key itself
		state []string
		key   string
		// (an agent that could not save its key), "unknown" (a key nobody can know: no request is authorized), or the
		// key itself.
		// deny are tokens the oracle must refuse; own, that each side made a key of its own
		deny []string
		own  bool
		// records are the main thread's records about the key
		records []string
	}
	cases := map[string]keyCase{
		// no file: a key is made and saved
		"key-none": {c: healthCase{key: healthKeyNone}, state: []string{healthKeyMade}, key: "file", deny: []string{healthKey}, own: true},
		// a UUID in upper case: the key is its lower case, and the file is not rewritten
		"key-upper": {c: healthCase{key: upper}, state: []string{healthKeyKept}, key: healthKey, deny: []string{upper}},
		// the key and a newline, as an editor leaves it: the first 36 bytes are the key
		"key-newline": {c: healthCase{key: healthKey + "\n"},
			state: []string{"the key file: a regular file of 37 bytes, mode 0600: " + healthKeyLaid}, key: healthKey},
		// 35 bytes: not read; a new key replaces them
		"key-short": {c: healthCase{key: healthKey[:35]}, state: []string{healthKeyMade}, key: "file", deny: []string{healthKey, healthKey[:35]},
			own: true, records: []string{healthRecKeyNotRead}},
		// 51 bytes that are no UUID. C's flexible parser answers -4 for them, regenerate_guid() takes only -1 for a
		// failure (registry_internals.c:83-86, libnetdata/uuid/uuid.c:69-125: -1 is an empty text's alone), so the text
		// passes for a key: the file stays, nothing is logged, and the key is the print of 16 bytes the parser never
		// wrote. Nobody knows it: every token the case can think of is refused, the file's own text first.
		"key-text": {c: healthCase{key: healthKeyNoUUID}, state: []string{"the key file: a regular file of 51 bytes, mode 0600: " + healthKeyLaid},
			key: "unknown", deny: []string{healthKeyNoUUID[:36], healthKeyNoUUID, healthKey, parentIdentity.MachineGUID}},
		// 36 bytes whose first is a NUL: the one text C's validation refuses (an empty one): both records, then a new
		// key, saved
		"key-nul": {c: healthCase{key: "\x00" + healthKey[1:]}, state: []string{healthKeyMade}, key: "file", deny: []string{healthKey}, own: true,
			records: []string{healthRecKeyNoGUID, healthRecKeyNotValid}},
		// a symbolic link to a file that holds a key: not followed, for reading or for writing
		"key-link": {c: healthCase{key: healthLink + "real.key", files: map[string]string{"lib/real.key": healthKey}},
			state: []string{"the key file: a symbolic link to real.key"}, key: "session", deny: []string{healthKey}, own: true,
			records: []string{healthRecKeyNotRegular, healthRecKeySession}},
		// a directory
		"key-dir": {c: healthCase{key: healthKeyDir}, state: []string{"the key file: a directory"}, key: "session", own: true,
			records: []string{healthRecKeyNotRegular, healthRecKeySession}},
		// a file nobody may write, with a text that is no key: the new key cannot be saved
		"key-readonly": {c: healthCase{key: healthKey[:35], keyMode: 0o444},
			state: []string{"the key file: a regular file of 35 bytes, mode 0444: " + healthKeyLaid}, key: "session", deny: []string{healthKey}, own: true,
			records: []string{healthRecKeyNotRead, healthRecKeyNotSaved, healthRecKeySession}},
		// the file netdata.conf names: read there, and nothing is made at the default path
		"key-path": {c: healthCase{keyFile: "lib/other.key"}, state: []string{healthKeyKept, "the default key file: no file"}, key: healthKey},
	}
	J, S, L, D := healthSilencersJSON, healthSaved, healthListed, healthDenied
	out := map[string]healthCase{}
	for name, k := range cases {
		c := k.c
		c.play = func(t *testing.T, h *healthPair) {
			h.compareKeyFile(t, k.state...)
			for i := range h.keys {
				switch k.key {
				case "file":
					h.keys[i] = h.key(i)
				case "session":
					h.keys[i] = h.sessionKey(t, i)
				default:
					h.keys[i] = k.key
				}
			}
			if k.key != "unknown" {
				h.manageStep(t, L(J(false, "None")))
			}
			h.manageSteps(t, D("cmd=LIST", healthNoToken), D("cmd=LIST", healthBadToken))
			for _, token := range k.deny {
				denied := D("cmd=LIST", token)
				denied.label = fmt.Sprintf("cmd=LIST with the token %q", token)
				h.manageStep(t, denied)
			}
			if k.own {
				h.otherKey(t)
			}
			if k.key != "unknown" {
				h.manageSteps(t, S("cmd=SILENCE ALL", healthMsgSilenceAll, J(true, "SILENCE")), L(J(true, "SILENCE")))
			}
			// no request touches the key's file
			h.compareKeyFile(t, k.state...)
			if target, link := strings.CutPrefix(c.key, healthLink); link {
				h.compareLines(t, "the link's target", func(i int) []string {
					file := filepath.Join(h.p.Each()[i].Daemon.Opts.RunDir, filepath.Dir(healthKeyFile), target)
					return []string{healthKeyStateOf(file, healthKey)}
				}, func(oracle []string) error {
					// its mode is the harness's umask's
					if !strings.HasPrefix(oracle[0], "a regular file of 36 bytes, mode ") || !strings.HasSuffix(oracle[0], ": "+healthKeyLaid) {
						return fmt.Errorf("want a regular file of 36 bytes that holds %s", healthKeyLaid)
					}
					return nil
				})
			}
		}
		c.after = func(t *testing.T, h *healthPair) {
			want := slices.Concat([]string{healthRecNoFile}, k.records)
			if k.key != "unknown" {
				want = append(want, healthRecWritten)
			}
			h.compareRecords(t, want...)
		}
		out[name] = c
	}
	return out
}

// healthFileEdited is the `file-edited` case's silencers file: a text an operator could have written, in the JSON
// both agents read (no comment, no trailing comma), whose members C's walk takes otherwise than their names say
// (health_silencers.c:124-170, libnetdata/json/json.c:195-227, :476-503): a boolean of any name sets `all` and the
// last one stands; every array's objects are selectors, a nested one's too; a null element is skipped; a key in
// upper case is the key and the later of two stands; `template` and unknown names set nothing; an object inside an
// element is walked for `type` and booleans; an object at the root is not looked at. No element is a string, a
// number, a boolean or an array: C dies of those at its start (D210 F4).
const healthFileEdited = `{
  "type": "DISABLE",
  "silencers": [
    { "alarm": "one", "ALARM": "two", "template": "t", "note": "n" },
    null,
    { "chart": "c", "context": "x", "hosts": "h", "more": [ { "alarm": "nested" } ], "deep": { "extra": true, "type": "SILENCE" } }
  ],
  "second": [ { "hosts": "other" } ],
  "all": true,
  "number": 7,
  "object": { "type": "DISABLE", "all": true },
  "last": false
}
`

// healthFileCases are the `file-*` cases: one state of the silencers file each, with health on and no chart: what
// LIST answers after the start, the file's bytes (the start writes nothing), what a request that saves makes of the
// file, and the records.
func healthFileCases() map[string]healthCase {
	J, S, L := healthSilencersJSON, healthSaved, healthListed
	none := J(false, "None")
	kept := J(false, "SILENCE", healthSel{"alarm", "kept"})
	pad := func(n int) string { return kept + strings.Repeat(" ", n-len(kept)) }
	const file = "<RUN>/lib/health.silencers.json"
	outOfRange := func(size int) string {
		return fmt.Sprintf(`level=error … !errno= … msg="Health silencers file %s has the size %d that is out of range[ 1 , 10000 ]. Aborting read."`, file, size)
	}
	added := healthSel{"alarm", "added"}
	type fileCase struct {
		// text is the file before the start (nil: none)
		text *string
		// loaded is LIST's answer after the start; saved, the file after `alarm=added`, a request that saves, with its
		// answer `said`
		loaded, said, saved string
		// records are the main thread's records of the start
		records []string
	}
	text := func(s string) *string { return &s }
	cases := map[string]fileCase{
		// no file: an INFO record, the default state
		"file-missing": {nil, none, healthMsgAdded + healthMsgNoType, J(false, "None", added), []string{healthRecNoFile}},
		// 0 bytes and 10,000 bytes are refused; 9,999 are read
		"file-empty": {text(""), none, healthMsgAdded + healthMsgNoType, J(false, "None", added), []string{outOfRange(0)}},
		"file-big":   {text(pad(10000)), none, healthMsgAdded + healthMsgNoType, J(false, "None", added), []string{outOfRange(10000)}},
		"file-max": {text(pad(9999)), kept, healthMsgAdded, J(false, "SILENCE", added, healthSel{"alarm", "kept"}),
			[]string{healthRecParsed}},
		// a text that is no JSON: an ERR record, then the INFO record that says it was parsed
		"file-invalid": {text(`{"all": true, "type": "DISABLE", "silencers": [`), none, healthMsgAdded + healthMsgNoType, J(false, "None", added),
			[]string{`level=error … !errno= … msg="JSON: Invalid json string."`, healthRecParsed}},
		// a hand-edited text
		"file-edited": {text(healthFileEdited),
			J(false, "SILENCE", healthSel{"hosts", "other"}, healthSel{"alarm", "nested"}, healthSel{"chart", "c", "context", "x", "hosts", "h"},
				healthSel{"alarm", "two"}),
			healthMsgAdded,
			J(false, "SILENCE", added, healthSel{"hosts", "other"}, healthSel{"alarm", "nested"}, healthSel{"chart", "c", "context", "x", "hosts", "h"},
				healthSel{"alarm", "two"}),
			[]string{healthRecParsed}},
	}
	out := map[string]healthCase{}
	for name, f := range cases {
		c := healthCase{}
		if f.text != nil {
			c.files = map[string]string{healthSilencersFile: *f.text}
		}
		c.play = func(t *testing.T, h *healthPair) {
			// the start read the file and wrote nothing
			h.compareLines(t, "the silencers file after the start", func(i int) []string { return []string{h.silencersFile(i)} },
				func(oracle []string) error {
					want := "the silencers file: open {run}/lib/health.silencers.json: no such file or directory"
					if f.text != nil {
						want = "the silencers file: " + strconv.Quote(*f.text)
					}
					if oracle[0] != want {
						return fmt.Errorf("want %s", want)
					}
					return nil
				})
			// a file the agent makes has the mode its umask leaves (C's is 0007: daemon.c:482); one that was there
			// keeps its own (the harness's 0644)
			saved := S("alarm=added", f.said, f.saved)
			if saved.mode = "0644"; f.text == nil {
				saved.mode = "0660"
			}
			h.manageSteps(t, L(f.loaded), saved, L(f.saved))
		}
		c.after = func(t *testing.T, h *healthPair) {
			h.compareRecords(t, slices.Concat(f.records, []string{healthRecWritten})...)
		}
		out[name] = c
	}
	// a file that cannot be written: its directory does not exist. The request is answered as any other, the state
	// changes, and a web thread writes an ERR record with the open's error
	const nowhere = "lib/none/health.silencers.json"
	out["file-unwritable"] = healthCase{
		silencers: nowhere,
		play: func(t *testing.T, h *healthPair) {
			h.manageSteps(t, L(none),
				healthManage{label: "cmd=SILENCE ALL", path: "?cmd=SILENCE%20ALL", want: healthText(200, healthMsgSilenceAll)},
				L(J(true, "SILENCE")),
				healthManage{label: "cmd=RESET", path: "?cmd=RESET", want: healthText(200, healthMsgReset)},
				L(none))
			h.compareLines(t, "the silencers file", func(i int) []string { return []string{h.silencersFile(i)} },
				func(oracle []string) error {
					if want := "the silencers file: open {run}/" + nowhere + ": no such file or directory"; oracle[0] != want {
						return fmt.Errorf("want %s", want)
					}
					return nil
				})
		},
		after: func(t *testing.T, h *healthPair) {
			notWritten := `level=error errno="2, No such file or directory" … thread=WEB[n] … msg="Silencer changes could not be written to <RUN>/` + nowhere + `. Error No such file or directory"`
			h.compareRecords(t, `level=info errno="2, No such file or directory" … msg="Cannot open the file <RUN>/`+nowhere+`, so Netdata will work with the default health configuration."`,
				notWritten, notWritten)
		},
	}
	// a directory where the file would be: C opens it for reading, takes the end of the directory for the file's size
	// and refuses that; a request is answered and the state changes, and the write fails
	out["file-dir"] = healthCase{
		files: map[string]string{healthSilencersFile + "/keep": ""},
		play: func(t *testing.T, h *healthPair) {
			h.manageSteps(t, L(none),
				healthManage{label: "cmd=SILENCE ALL", path: "?cmd=SILENCE%20ALL", want: healthText(200, healthMsgSilenceAll)},
				L(J(true, "SILENCE")))
		},
		after: func(t *testing.T, h *healthPair) {
			// the size is what the file system says a directory's end is (ext4: the largest offset there is)
			h.compareRecords(t, `level=error … !errno= … msg="Health silencers file <RUN>/lib/health.silencers.json has the size `+
				` … that is out of range[ 1 , 10000 ]. Aborting read."`,
				`level=error errno="21, Is a directory" … thread=WEB[n] … msg="Silencer changes could not be written to <RUN>/lib/health.silencers.json. Error Is a directory"`)
		},
	}
	return out
}

// TestHealthSilencersNorm checks what the `health.silencers` cases stand on, without an agent: the JSON text of a
// state against the text C writes, a query's encoding, the script's flags and its count of changes, the golden
// files' reading, a transcript's and a log view's head, the records' rendering and their guard, the flags' guard,
// and the key file's states as healthMakeKey lays them out and healthKeyStateOf names them.
func TestHealthSilencersNorm(t *testing.T) {
	// the state's text: C's own (the unit oracle's rows of health_silencers2json())
	for name, c := range map[string]struct{ got, want string }{
		"empty": {healthSilencersJSON(false, "None"), "{\n\t\"all\": false,\n\t\"type\": \"None\",\n\t\"silencers\": []\n}\n"},
		"two selectors": {healthSilencersJSON(false, "DISABLE", healthSel{"context", "system.cpu"}, healthSel{"chart", "system.load"}),
			"{\n\t\"all\": false,\n\t\"type\": \"DISABLE\",\n\t\"silencers\": [\n\t\t{\n\t\t\t\"context\": \"system.cpu\"\n\t\t},\n\t\t{\n\t\t\t\"chart\": \"system.load\"\n\t\t}\n\t]\n}\n"},
		"two members": {healthSilencersJSON(true, "SILENCE", healthSel{"alarm", "a\"b", "hosts", "h"}),
			"{\n\t\"all\": true,\n\t\"type\": \"SILENCE\",\n\t\"silencers\": [\n\t\t{\n\t\t\t\"alarm\": \"a\"b\",\n\t\t\t\"hosts\": \"h\"\n\t\t}\n\t]\n}\n"},
		"a selector with nothing set": {healthSilencersJSON(false, "SILENCE", healthSel{}),
			"{\n\t\"all\": false,\n\t\"type\": \"SILENCE\",\n\t\"silencers\": [\n\t\t{\n\t\t}\n\t]\n}\n"},
		"as the older helper": {healthJSONAnswer(healthSilencersJSON(false, "DISABLE", healthSel{"context", "hsig.ctx"}, healthSel{"alarm", "hs_calc"})),
			healthSilencersList("false", "DISABLE", `"context": "hsig.ctx"`, `"alarm": "hs_calc"`)},
		"as the older helper, empty": {healthJSONAnswer(healthSilencersJSON(true, "SILENCE")), healthSilencersList("true", "SILENCE")},
		"a query":                    {healthQuery("cmd=SILENCE ALL&alarm=*a b&x=a\"b\\c\td==e&&é!"), "cmd=SILENCE%20ALL&alarm=*a%20b&x=a%22b%5Cc%09d==e&&%C3%A9!"},
		"collapsed":                  {healthCollapse(healthSilencersJSON(false, "SILENCE") + healthMsgNoSelector), "{ \"all\": false, \"type\": \"SILENCE\", \"silencers\": [] } WARNING: SILENCE or DISABLE command is ineffective without defining any alarm selectors.\n"},
		"the script's flags":         {strings.Join(healthScriptFlags("True False False True False False"), "|"), "10min_cpu_usage disabled=true silenced=false|10min_cpu_iowait disabled=false silenced=true|load_trigger disabled=false silenced=false"},
		"a transcript's head":        {healthCallsHead("call 1: argv[0]=x\ncall 1: end exit 0\ncall 2: argv[0]=y\ncall 3: argv[0]=z", 2), "call 1: argv[0]=x\ncall 1: end exit 0\ncall 2: argv[0]=y"},
		"a short transcript's head":  {healthCallsHead("call 1: argv[0]=x", 2), "call 1: argv[0]=x"},
		"a log view's head":          {healthLinesHead("a: x last_repeat=0\na: y last_repeat=T\nb: z last_repeat=0\na: y last_repeat=T\na: y last_repeat=T", " last_repeat=T", 2), "a: x last_repeat=0\na: y last_repeat=T\nb: z last_repeat=0\na: y last_repeat=T"},
		"a short log view's head":    {healthLinesHead("a: x last_repeat=0", " last_repeat=T", 2), "a: x last_repeat=0"},
	} {
		if c.got != c.want {
			t.Errorf("%s:\n got %q\nwant %q", name, c.got, c.want)
		}
	}
	// the collapsed text of a state is C's golden file of it, and the script names 15 files of the 16
	if got, want := healthCollapse(healthSilencersJSON(false, "DISABLE", healthSel{"context", "system.cpu"}, healthSel{"chart", "system.load"})),
		healthGolden(t, "CONTEXT_SYSTEM_CPU"); got != want {
		t.Errorf("the golden file CONTEXT_SYSTEM_CPU:\n got %q\nwant %q", got, want)
	}
	named := map[string]bool{}
	for _, s := range healthScript {
		named[s.list] = healthGolden(t, s.list) != ""
		if s.not != "" {
			named[s.not] = healthGolden(t, s.not) != ""
		}
	}
	if files, _ := filepath.Glob(filepath.Join("..", "health_mgmtapi", "expected_list", "*-list.json")); len(files) != 16 || len(named) != 16 {
		t.Errorf("%d golden files, of which the script's steps name %d: want 16 and 16", len(files), len(named))
	}
	if got := healthScriptChanges(); got != 38 {
		t.Errorf("the script changes an alert's flags %d times, want 38", got)
	}

	// a step's guard: the answer when the step names one, whether the file was written, its bytes when named
	view := healthText(200, healthMsgReset) + "\nthe silencers file, written: " + strconv.Quote(healthSilencersJSON(false, "None")) +
		"\nthe silencers file's mode: 0660"
	if got := healthManageBody(view); got != healthMsgReset {
		t.Errorf("a request's body: %q", got)
	}
	for name, c := range map[string]struct {
		step healthManage
		ok   bool
	}{
		"as it is":           {healthSaved("cmd=RESET", healthMsgReset, healthSilencersJSON(false, "None")), true},
		"no answer named":    {healthManage{saves: true}, true},
		"another answer":     {healthSaved("cmd=RESET", healthMsgSilence, healthSilencersJSON(false, "None")), false},
		"another type":       {healthManage{want: healthJSONAnswer(healthMsgReset), saves: true}, false},
		"other bytes":        {healthSaved("cmd=RESET", healthMsgReset, healthSilencersJSON(true, "None")), false},
		"not to be written":  {healthManage{want: healthText(200, healthMsgReset)}, false},
		"no bytes named":     {healthManage{want: healthText(200, healthMsgReset), saves: true}, true},
		"a LIST's own guard": {healthListed(healthSilencersJSON(false, "None")), false},
		"the mode":           {healthManage{saves: true, mode: "0660"}, true},
		"another mode":       {healthManage{saves: true, mode: "0644"}, false},
	} {
		if err := c.step.guard(view); (err == nil) != c.ok {
			t.Errorf("a step's guard, %s: %v", name, err)
		}
	}
	if err := healthListed(healthSilencersJSON(false, "None"), healthMsgNoSelector).guard(healthJSONAnswer(healthSilencersJSON(false, "None")+healthMsgNoSelector) +
		"\nthe silencers file, not written: open {run}/lib/health.silencers.json: no such file or directory\nthe silencers file's mode: none"); err != nil {
		t.Errorf("a LIST's guard: %v", err)
	}

	// the flags' guard: the alerts in any order, no other alert, the answer a 200
	flags := "HTTP 200, application/json; charset=utf-8\nb disabled=false silenced=true\na disabled=false silenced=false"
	for name, c := range map[string]struct {
		view string
		want []string
		ok   bool
	}{
		"in another order": {flags, []string{"a disabled=false silenced=false", "b disabled=false silenced=true"}, true},
		"another flag":     {flags, []string{"a disabled=false silenced=false", "b disabled=true silenced=false"}, false},
		"an alert more":    {flags, []string{"a disabled=false silenced=false"}, false},
		"an alert less":    {flags, []string{"a disabled=false silenced=false", "b disabled=false silenced=true", "c disabled=false silenced=false"}, false},
		"no 200":           {strings.Replace(flags, "200", "404", 1), []string{"a disabled=false silenced=false", "b disabled=false silenced=true"}, false},
	} {
		if err := healthFlagsWant(c.want...)(c.view); (err == nil) != c.ok {
			t.Errorf("the flags' guard, %s: %v", name, err)
		}
	}

	// the records: by thread, each thread's in file order; the directory, the clock, the thread's number and a
	// session's key masked; the errno kept; other records left out
	run := "/ndt/x/oracle"
	key := "0123abcd-0000-4000-8000-0000000c0de5"
	log := []string{
		`time=2026-10-04T18:26:08.282Z comm=netdata source=daemon level=info errno="2, No such file or directory" tid=10  msg="Cannot open the file ` + run + `/lib/health.silencers.json, so Netdata will work with the default health configuration."`,
		`time=2026-10-04T18:26:08.283Z comm=netdata source=daemon level=info tid=10  msg="Creating archived hosts"`,
		`time=2026-10-04T18:26:09.000Z comm=netdata source=daemon level=info tid=30 thread=HEALTH msg="Alarm silencing changed for host 'h' alarm 'a': Disabled false->true Silenced false->false"`,
		`time=2026-10-04T18:26:09.100Z comm=netdata source=daemon level=info tid=21 thread=WEB[3] src_transport=http role=none permissions=0x8 src_ip=localhost src_port=49270 req_method=GET conn=2 transaction=c98e73ee4ce5497b97c0ba21123af9c9 request="/api/v1/manage/health?cmd=RESET" msg="Silencer changes written to ` + run + `/lib/health.silencers.json"`,
		`time=2026-10-04T18:26:09.200Z comm=netdata source=daemon level=info tid=10  msg="You can still continue to use the alarm management API using the authorization token ` + key + ` during this Netdata session only."`,
		`time=2026-10-04T18:26:09.300Z comm=netdata source=daemon level=debug tid=30 thread=HEALTH msg="Skipping health checks, because all alarms are disabled via API command."`,
	}
	wantRecords := []string{
		`time=T comm=netdata source=daemon level=info errno="2, No such file or directory" tid=N  msg="Cannot open the file <RUN>/lib/health.silencers.json, so Netdata will work with the default health configuration."`,
		`time=T comm=netdata source=daemon level=info tid=N  msg="You can still continue to use the alarm management API using the authorization token <KEY> during this Netdata session only."`,
		`time=T comm=netdata source=daemon level=info tid=N thread=WEB[n] src_transport=http role=none permissions=0x8 src_ip=localhost src_port=P req_method=GET conn=N transaction=X request="/api/v1/manage/health?cmd=RESET" msg="Silencer changes written to <RUN>/lib/health.silencers.json"`,
		`time=T comm=netdata source=daemon level=info tid=N thread=HEALTH msg="Alarm silencing changed for host 'h' alarm 'a': Disabled false->true Silenced false->false"`,
		`time=T comm=netdata source=daemon level=debug tid=N thread=HEALTH msg="Skipping health checks, because all alarms are disabled via API command."`,
	}
	records := healthSilencerRecordsOf(log, run)
	if !slices.Equal(records, wantRecords) {
		t.Errorf("the records:\n got %q\nwant %q", records, wantRecords)
	}
	want := []string{healthRecNoFile, healthRecKeySession, healthRecWritten,
		healthRecChanged("h", "a", [2]bool{false, true}, [2]bool{false, false}), healthRecSkipping}
	if err := healthRecordsAre(want...)(records); err != nil {
		t.Errorf("the records' guard: %v", err)
	}
	if err := healthRecordsAre(want[:4]...)(records); err == nil {
		t.Errorf("the records' guard passes a record more")
	}
	if err := healthRecordsAre(want[0], want[1], want[2], healthRecChanged("h", "a", [2]bool{false, false}, [2]bool{false, true}), want[4])(records); err == nil {
		t.Errorf("the records' guard passes another record")
	}
	// a part the record must not hold: an errno on the first record, where the guard wants one, and none elsewhere
	if err := healthRecordsAre(healthRecParsed[:len("level=info … !errno= … ")]+want[0][len(`level=info errno="2, No such file or directory" … `):],
		want[1], want[2], want[3], want[4])(records); err == nil {
		t.Errorf("the records' guard passes an errno where none is wanted")
	}
	if got := healthSessionKeyRe.FindStringSubmatch(log[4]); got == nil || got[2] != key {
		t.Errorf("the session key of a record: %q", got)
	}

	// the key file's states: what healthMakeKey lays out, as healthKeyStateOf names it
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "real.key"), []byte(healthKey), 0o644); err != nil {
		t.Fatal(err)
	}
	for name, c := range map[string]struct {
		state string
		mode  os.FileMode
		laid  string
		want  string
	}{
		"the fixed key": {"", 0, healthKey, "a regular file of 36 bytes, mode 0600: " + healthKeyLaid},
		"none":          {healthKeyNone, 0, healthKeyNone, "no file"},
		"a directory":   {healthKeyDir, 0, healthKeyDir, "a directory"},
		"a link":        {healthLink + "real.key", 0, healthLink + "real.key", "a symbolic link to real.key"},
		"a text":        {"abc\n", 0, "abc\n", "a regular file of 4 bytes, mode 0600: " + healthKeyLaid},
		"read-only":     {"abc", 0o444, "abc", "a regular file of 3 bytes, mode 0444: " + healthKeyLaid},
		"a key made":    {"0123abcd-0000-4000-8000-0000000c0de5", 0, healthKey, "a regular file of 36 bytes, mode 0600: " + healthKeyNew},
		"upper case":    {strings.ToUpper(healthKey), 0, healthKey, `a regular file of 36 bytes, mode 0600: the text "` + strings.ToUpper(healthKey) + `"`},
	} {
		file := filepath.Join(dir, name, "netdata.api.key")
		if name == "a link" {
			file = filepath.Join(dir, "netdata.api.key")
		}
		if err := healthMakeKey(file, c.state, c.mode); err != nil {
			t.Errorf("the key file, %s: %v", name, err)
		}
		if got := healthKeyStateOf(file, c.laid); got != c.want {
			t.Errorf("the key file, %s: %q, want %q", name, got, c.want)
		}
	}
}
