// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// settingsRow is one request of the settings check and, when file is set, the settings file both sides must hold
// after it: the oracle's must be content exactly (empty: no file).
type settingsRow struct {
	exactReq
	file, content string
}

// settingsPair is dashPair's options with fnWriteTokens' two bearer tokens laid out before each side starts (the
// admin rows).
func settingsPair(t *testing.T) *Pair {
	t.Helper()
	return startPairWith(t, dashPairOptions(daemon.Options{}), parentIdentity, binaries(t), [2]string{},
		[2]Role{Oracle, Candidate}, fnWriteTokens)
}

// settingsFiles compares both sides' `<varlib>/settings/<file>` (bytes and mode) and the directory's mode, once the
// oracle's are judged: the file must hold content exactly with no `<file>.new` left (api_v3_settings.c:240-270), the
// directory have mode 0750 (C makes it on the first PUT, :193, paths.c:182-193) and the file 0660 (0666 under the
// daemon's umask 0007, :136, :268, daemon.c:482); with content empty, neither may exist (a GET makes nothing,
// :82-109).
func settingsFiles(t *testing.T, p *Pair, file, content string) {
	t.Helper()
	type state struct {
		dir, mode fs.FileMode
		data      []byte
		tmp       bool
	}
	var got [2]state
	for i, side := range p.Each() {
		dir := filepath.Join(side.Daemon.Opts.RunDir, "lib", "settings")
		var s state
		if fi, err := os.Lstat(dir); err == nil {
			s.dir = fi.Mode()
		}
		if fi, err := os.Lstat(filepath.Join(dir, file)); err == nil {
			s.mode = fi.Mode()
		}
		s.data, _ = os.ReadFile(filepath.Join(dir, file))
		_, err := os.Lstat(filepath.Join(dir, file+".new"))
		s.tmp = !errors.Is(err, fs.ErrNotExist)
		got[i] = s
	}
	want := state{dir: fs.ModeDir | 0o750, mode: 0o660}
	if content == "" {
		want = state{}
	}
	if o := got[0]; string(o.data) != content || o.tmp || o.dir != want.dir || o.mode != want.mode {
		t.Fatalf("oracle: settings file %s holds %q (mode %v, directory %v, %s.new left: %v), want %q (mode %v, "+
			"directory %v)", file, o.data, o.mode, o.dir, file, o.tmp, content, want.mode, want.dir)
	}
	if got[0].dir != got[1].dir || got[0].mode != got[1].mode || !bytes.Equal(got[0].data, got[1].data) ||
		got[0].tmp != got[1].tmp {
		t.Errorf("settings file %s differs: oracle dir %v, file %v %q, %s.new left %v; candidate dir %v, file %v %q, "+
			"%s.new left %v", file, got[0].dir, got[0].mode, got[0].data, file, got[0].tmp, got[1].dir, got[1].mode,
			got[1].data, file, got[1].tmp)
	}
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
	setV2      = `{ "version": 2, "value": { "preferred_node_ids": [ "5a1e0000-0000-4000-8000-0000000000bb" ] } }`
	setV3      = `{ "version": 3, "value": { "preferred_node_ids": [ "5a1e0000-0000-4000-8000-0000000000bb" ] } }`
)

// The dashboard's PUT of its preferred nodes (installed v3 app), on the fresh file (stored as setV2) and on setV2.
var (
	setPutV1 = []byte(`{"version":1,"value":{"preferred_node_ids":["` + childHost.MachineGUID + `"]}}`)
	setPutV2 = []byte(`{"version":2,"value":{"preferred_node_ids":["` + childHost.MachineGUID + `"]}}`)
)

// setRow is a settings row whose answer the oracle must give with the status line and the body exactly.
func setRow(name, method, target string, body []byte, status, answer string, headers ...string) exactReq {
	return exactReq{name: name, method: method, target: target, body: body, headers: headers,
		want: [2]string{status, "\r\n\r\n" + answer}}
}

// TestSettingsAPI (check `api.settings`, M10 commit 7, D224): `/api/v3/settings`, the dashboard's per-agent settings
// file, asked in order on one pair (the file is state): GETs and PUTs of the default file through its versions (409 for
// a stale version), the payload and file-name errors (400), the other methods, a routed host, and an admin's other
// file. Every answer is compared raw after maskRaw; after the first GET and each PUT, both sides' file (bytes and
// mode), its directory's mode, and no temporary file left, the oracle's judged first (settingsFiles). Red on Rust
// until commit 7.
func TestSettingsAPI(t *testing.T) {
	const ok, bad, conflict = "HTTP/1.1 200 OK\r\n", "HTTP/1.1 400 Bad Request\r\n", "HTTP/1.1 409 Conflict\r\n"
	def := setPath + "?file=default"
	put := func(name string, body []byte, status, answer, content string, headers ...string) settingsRow {
		return settingsRow{setRow(name, "PUT", def, body, status, answer, headers...), "default", content}
	}
	get := func(name, target, status, answer string, headers ...string) settingsRow {
		return settingsRow{exactReq: setRow(name, "", target, nil, status, answer, headers...)}
	}
	// a GET makes neither the directory nor the file (api_v3_settings.c:82-109)
	fresh := get("get-fresh", def, ok, setFresh)
	fresh.file = "default"
	other := `{ "version": 2, "value": { "url": "http:\/\/x\/y", "n": 1.50, "on": true, "off": null, "list": [ ] } }`
	rows := []settingsRow{
		fresh,
		put("put-v1", setPutV1, ok, setOK, setV2),
		get("get-v2", def, ok, setV2),
		// the stored version is 2 now (C bumps it on every PUT, :233-235)
		put("put-stale", setPutV1, conflict,
			`{"status":409,"errorMessage":"Payload version does not match the version of the stored object"}`, setV2),
		put("put-v2", setPutV2, ok, setOK, setV3),
		get("get-v3", def, ok, setV3),
		put("put-not-json", []byte("not json"), bad,
			`{"status":400,"errorMessage":"Payload cannot be parsed as a JSON object"}`, setV3),
		put("put-no-version", []byte(`{"value":1}`), bad,
			`{"status":400,"errorMessage":"Field version is not found in payload"}`, setV3),
		put("put-array", []byte(`[1]`), bad, `{"status":400,"errorMessage":"Field version is not found in payload"}`,
			setV3),
		put("put-empty", []byte{}, bad, `{"status":400,"errorMessage":"Settings API PUT action requires a payload."}`,
			setV3),
		get("get-other", setPath+"?file=other", bad,
			`{"status":400,"errorMessage":"Only the 'default' settings file is allowed for anonymous users"}`),
		get("get-dot", setPath+"?file=a.b", bad, setInvalid),
		get("get-dotdot", setPath+"?file=../x", bad, setInvalid),
		get("get-none", setPath, bad, setInvalid),
		// a POST would store a version 4 if it were taken for a PUT
		{setRow("post", "POST", def, []byte(`{"version":3}`), bad, setMode), "default", setV3},
		{setRow("delete", "DELETE", def, nil, bad, setMode), "default", setV3},
		get("host", "/host/"+childHost.Hostname+def, bad,
			`{"status":400,"errorMessage":"Settings API is only allowed for the agent node."}`),
		get("admin-get", setPath+"?file=other", ok, setFresh, dcAdmin),
		// the stored bytes are json-c's (:238): its escapes, numbers and empty containers as GET returns them
		{setRow("admin-put", "PUT", setPath+"?file=other",
			[]byte(`{"version":1,"value":{"url":"http://x/y","n":1.50,"on":true,"off":null,"list":[]}}`), ok, setOK,
			dcAdmin), "other", other},
		get("admin-get2", setPath+"?file=other", ok, other, dcAdmin),
	}
	t.Run("rows", func(t *testing.T) {
		p := settingsPair(t)
		dashChild(t, p, dashBase())
		for _, r := range rows {
			t.Run(r.name, func(t *testing.T) {
				compareExact(t, p, r.exactReq)
				if r.file != "" {
					settingsFiles(t, p, r.file, r.content)
				}
			})
		}
	})
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
