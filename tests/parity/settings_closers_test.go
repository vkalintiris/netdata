// SPDX-License-Identifier: GPL-3.0-or-later

package parity

// The settings check's closer rows (BACKLOG, "From M10 commit 7's close"): the errno of C's record of a stored file
// that gives no version, end to end, where json-c's reading of the file sets none (what the request left before it)
// and where it sets one (EINVAL). A pair of their own (`errno`), after the check's two: the anonymous client's rows
// need the `default` file, which the `rows` pair's rows hold.

// settingsCloserPairs are the check's pairs after settingsPairs, each with rows of its own (settingsCheckRows).
var settingsCloserPairs = []string{"errno"}

// settingsCheckRows are the rows of one of the check's pairs: the closer pairs' (settingsErrnoRows), or
// settingsRowsOf's.
func settingsCheckRows(pair string) []settingsRow {
	if pair == "errno" {
		return settingsErrnoRows()
	}
	return settingsRowsOf(pair)
}

// setUnknownBearer is a bearer token no agent of the check has: a well-formed UUID, so C looks it up and, not finding
// it in memory, tries its file (http_auth.c:302-308, read_txt_file_to_buffer), whose open fails with ENOENT.
const setUnknownBearer = "Authorization: Bearer 5a1e0000-0000-4000-8000-00000000dead"

// setAnonymousParts are more parts of a settings record: the client of a request that carries no token the agent
// knows (settingsFirstRows' `anonymous-get`).
const setAnonymousParts = " … role=none permissions=0x8 … !account= … !user="

// settingsErrnoRows (pair `errno`): C's logger attaches the thread's errno to a record (nd_log.c:429, :341-342), so
// the record of a stored file without a version (api_v3_settings.c:96-101) carries what the last call that set it
// left.
//
//   - `control-get`: an anonymous GET of `default` laid as `not json`: json-c touches no errno reading it (R109's
//     probe of json-c 0.18), the request set none before it: no errno (settingsFirstRows' `anonymous-get`).
//   - `bearer-get`, `bearer-put`: the same with a bearer token no one has (setUnknownBearer): the token's file is
//     looked for and not found, which leaves ENOENT, and nothing clears it before the record (D242's known limit:
//     the port writes no errno there, daemon/src/settings.rs:66; DEFECTS, the settings block's last entry). The PUT
//     reads the stored version through the GET's path (api_v3_settings.c:201) and replaces the file.
//   - `string-get`, `string-put`: an admin's file laid as `{"version":"x"}`: json-c reads the string's digits with
//     strtoll, which leaves EINVAL (json_object_get_int, the C-made vectors' errno column,
//     text/tests/vectors/jsonc_doc.tsv:39-40): the record carries it, and the PUT replaces the file.
func settingsErrnoRows() []settingsRow {
	def := setFile("default")
	const notJSON, strx = "not json", `{"version":"x"}`
	return []settingsRow{
		setGet("control-get", def, setStatusOK, setFresh).on("default", setStored(notJSON)).
			laid(setLayFile("default", notJSON, 0o660)).logs(setVersionRecord("GET", "default") + setAnonymousParts),
		setGet("bearer-get", def, setStatusOK, setFresh, setUnknownBearer).on("default", setStored(notJSON)).
			logs(setErrnoRecord("2, No such file or directory", "GET", "default") + setAnonymousParts),
		setPut("bearer-put", def, `{"version":1}`, setStatusOK, setOK, setUnknownBearer).
			on("default", setStored(`{ "version": 2 }`)).
			logs(setErrnoRecord("2, No such file or directory", "PUT", "default") + setAnonymousParts),
		setGet("string-get", setFile("st-strx"), setStatusOK, setFresh, dcAdmin).on("st-strx", setStored(strx)).
			laid(setLayFile("st-strx", strx, 0o660)).logs(setErrnoRecord("22, Invalid argument", "GET", "st-strx")),
		setPut("string-put", setFile("st-strx"), `{"version":1}`, setStatusOK, setOK, dcAdmin).
			on("st-strx", setStored(`{ "version": 2 }`)).logs(setErrnoRecord("22, Invalid argument", "PUT", "st-strx")),
	}
}
