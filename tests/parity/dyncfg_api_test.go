// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// TestDynCfgAPI (check `dyncfg.api`, M8 commit 8, D169; plan §5.2): the fake plugin's DynCfg nodes registered with
// CONFIG and driven through `/api/v1|v3/config` and `/api/v1|v3/function`. Compared per case: every exchange (status
// line, headers through fnHTTPMask, body; wall-clock values through dcClock), the DynCfg directory's files, every
// plugin start's stdin byte for byte (the agent's own call ids masked), the plugin threads' records, the access
// records, the web workers' call records and the DynCfg records.
func TestDynCfgAPI(t *testing.T) {
	runDynCfgCases(t, dcAPICases())
}

// dcReady are the usual nodes after their registration and initial echoes (the single and the job registered
// `accepted`, so `running` shows the echo's 200 was processed, dyncfg-echo.c:39-40).
var dcReady = map[string]string{dcS: "running", dcT: "accepted", dcJ1: "running"}

// treeLacks polls the tree until it no longer lists id.
func (x *dcSide) treeLacks(t *testing.T, id string) bool {
	t.Helper()
	key := `"` + id + `":{`
	return x.until(t, id+" still listed", "action=tree", func(b []byte) bool { return !strings.Contains(string(b), key) })
}

// dcFlagRe finds a node's boolean member in a tree (the node's own: the members are in DynCfg's order, so the first
// after the id is the node's).
func dcFlagRe(id, flag string, v bool) *regexp.Regexp {
	return regexp.MustCompile(`(?s)"` + regexp.QuoteMeta(id) + `":\{.*?"` + flag + `":` + fmt.Sprint(v))
}

func dcAPICases() map[string]dcCase {
	cases := map[string]dcCase{}
	tree := func(t *testing.T, x *dcSide, label string, n int, headers ...string) string {
		return x.get(t, label, "action=tree", n, headers...)
	}
	fn := func(query string) string { return "/api/v1/function?" + query }

	// CONFIG from the plugin (pluginsd_dyncfg.c:8-70, dyncfg.c:389-512): the usual nodes; unknown words (status `none`
	// kept, type single, source internal, cmds sanitized to schema); access 0 (C's defaults 0x23/0x47) with cmds
	// sanitized (NOTICE); the real plugins' two lines (systemd-journal all quoted, scripts.d bare `0x0000`); statuses
	// (`bogus` refused, an unknown id silent); a delete of an unknown id; an unknown action (WARNING to the collectors
	// log); FUNCTION_DEL of a DynCfg method (refused, nrpc-registry.c:712-719). A refused create ends the plugin's run
	// (pluginsd_parser.h:258-273, K7): a job without its template (start 1, its two echoes then failing with the
	// transport), an id with a space (2), one with a quote (3); start 4 registers the usual nodes again (a merge: the
	// methods return, the echoes run again).
	{
		counted := "CONFIG nope status running\n"
		create1 := dcCreates +
			plugin.ConfigCreate("difftest:w", "bogus", "nope", "/difftest/w", "nope", "words", "nope", 0x8, 0x8) +
			plugin.ConfigCreate("difftest:z", "running", "single", "/difftest/z", "user", "/etc/z.conf", "get add remove enable", 0, 0) +
			"CONFIG 'systemd-journal:monitored-directories' create 'running' 'single' '/logs/systemd-journal' 'internal' 'internal' 'get schema update' 0x0 0x0\n" +
			"CONFIG scripts.d:collector:nagios create accepted template /collectors/scripts.d/Jobs internal 'internal' 'add schema enable disable test userconfig' 0x0000 0x0000\n" +
			"CONFIG difftest:s status running\nCONFIG difftest:s status bogus\nCONFIG nope status running\nCONFIG nope delete\n" +
			"CONFIG difftest:s bogus\nFUNCTION_DEL GLOBAL \"config difftest:s\"\n" +
			plugin.ConfigCreate("difftest:u:x", "accepted", "job", "/difftest/u", "internal", "internal", "get", 0x8, 0x8) +
			"CONFIG never reached\n"
		start := func(emit string) plugin.Start {
			return plugin.Start{Steps: []plugin.Step{dcServe(), {Emit: emit}}}
		}
		cases["create"] = dcCase{
			starts: 4,
			// start 1 is killed on its refused line with its three echoes in flight: how much of its stdin it recorded
			// (QUIT, the last echo line), what it served and how it ended race the kill in C too (api1, r2, final1,
			// final2 in `.local/scratch-h11/`: QUIT on one side only, 0 or 1 echo served, the third echo line missing on
			// one side), so its view keeps its arguments and steps; the DynCfg records pin the three echoes (their 503s)
			views: func(vs []plugin.View) {
				if len(vs) > 0 {
					vs[0].Stdin, vs[0].EOF, vs[0].End, vs[0].Served = "", false, "", nil
				}
			},
			sc: plugin.Scenario{Starts: []plugin.Start{
				start(create1),
				start(counted + plugin.ConfigCreate("a b", "accepted", "single", "/difftest/ab", "internal", "internal", "get", 0x8, 0x8)),
				start(counted + `CONFIG "a'b" create 'accepted' 'single' '/difftest/q' 'internal' 'internal' 'get' 0x8 0x8` + "\n"),
				start(dcCreates),
			}},
			play: func(t *testing.T, x *dcSide) []string {
				if !x.served(t, 4, 2, "enable") || !x.waitTree(t, dcReady) {
					return nil
				}
				return []string{
					tree(t, x, "tree", 1),
					tree(t, x, "tree, admin", 2, dcAdmin),
				}
			},
			want: []string{
				"DYNCFG: id 'difftest:w' was declared with cmds: , but they have sanitized to: schema",
				"DYNCFG: id 'difftest:z' was declared with cmds: ",
				"DYNCFG: status provided to id 'difftest:s' is invalid. Ignoring it.",
				"DYNCFG: unknown action 'bogus' received from plugin",
				"refusing to unregister dyncfg method 'config difftest:s' via FUNCTION_DEL",
				"DYNCFG: job id 'difftest:u:x' does not have a registered template. Ignoring dynamic configuration for it.",
				"DYNCFG: id 'a b' is invalid. Ignoring dynamic configuration for it.",
				"DYNCFG: id 'a'b' is invalid. Ignoring dynamic configuration for it.",
				"parser_action('CONFIG') failed on line",
				"DYNCFG: received response code 503 on request to id 'difftest:z', cmd: enable",
				`FUNCTION RANDOM 10 "config difftest:s enable" "0x7ff" ""` + "\n",
			},
			wantNot: []string{"CONFIG never reached"},
		}
	}

	// CONFIG delete (dyncfg.c:471-493): a node never saved is gone; a saved one stays as an orphan (`["remove"]`,
	// dyncfg-tree.c:30), its file kept; the catch-all answers it (dyncfg-tree.c:216-290): get and update 404 "Unknown
	// config id given." (P4), remove deletes node and file (200, an empty message)
	{
		single := func(id string) string {
			return plugin.ConfigCreate(id, "running", "single", "/difftest/del", "internal", "internal", "get update restart", 0x8, 0x8)
		}
		goRule := dcRule("go", `^config difftest:d0 restart$`, "200", dcOK)
		goRule.Then = "CONFIG difftest:d0 delete\nCONFIG difftest:d1 delete\nCONFIG nope delete\n"
		cases["delete"] = dcCase{
			sc:    dcScenarioServing(dcServe(goRule), single("difftest:d0")+single("difftest:d1")),
			ready: map[string]string{"difftest:d0": "running", "difftest:d1": "running"},
			play: func(t *testing.T, x *dcSide) []string {
				out := []string{x.send(t, "update d1", "/api/v1/config?action=update&id=difftest:d1", 11, `{"d1":1}`)}
				out = append(out, x.files(t, "files after the update")...)
				out = append(out, x.get(t, "restart d0 (the plugin deletes both)", "action=restart&id=difftest:d0", 12))
				x.treeLacks(t, "difftest:d0")
				return append(out,
					tree(t, x, "tree after the deletes", 13),
					x.get(t, "get the orphan", "action=get&id=difftest:d1", 14),
					x.send(t, "update the orphan", "/api/v1/config?action=update&id=difftest:d1", 15, `{"d1":2}`),
					x.get(t, "remove the orphan", "action=remove&id=difftest:d1", 16),
					tree(t, x, "tree after the remove", 17),
				)
			},
			want: []string{
				fnHTTPError(404, "Unknown config id given."),
				"DYNCFG: unknown config id 'difftest:d1' in call: 'config difftest:d1 get'",
				fnQ(`{"status":200,"message":""}`),
			},
		}
	}

	// the tree (dyncfg-tree.c:64-170): every node of localhost sorted by path then id, anonymous (a dyncfg source
	// hidden) and admin; v3; a path filter, a raw prefix (K15); the id filter (the node and the jobs of a template); a
	// quote in the id (400), an unknown id; through `/api/v1/function`; v2 has none. attention: a 299 update
	// (restart_required), a rejected echo (plugin_rejected: the plugin re-registers difftest:r after its user update and
	// refuses the update echo), `status failed` and `incomplete`.
	{
		single := func(id, path, status, cmds string) string {
			return plugin.ConfigCreate(id, status, "single", path, "internal", "internal", cmds, 0x8, 0x8)
		}
		rOK := dcTimes(dcRule("r-ok", `^config difftest:r update$`, "200", dcOK), 1)
		rOK.Then = single("difftest:r", "/difftest", "accepted", "get update enable disable")
		cases["tree"] = dcCase{
			sc: dcScenarioServing(dcServe(dcTimes(dcRule("s-299", `^config difftest:s update$`, "299", dcOK), 1), rOK,
				dcRule("r-reject", `^config difftest:r update$`, "400", `{"status":400,"message":"difftest rejects"}`+"\n")),
				dcCreates+single("difftest:sub", "/difftest/sub", "running", "get update")+
					single("difftest:f", "/difftest", "running", "get")+single("difftest:i", "/difftest", "running", "get")+
					single("difftest:r", "/difftest", "accepted", "get update enable disable")+
					"CONFIG difftest:f status failed\nCONFIG difftest:i status incomplete\n"),
			ready: map[string]string{dcS: "running", dcT: "accepted", dcJ1: "running", "difftest:sub": "running",
				"difftest:f": "failed", "difftest:i": "incomplete", "difftest:r": "running"},
			play: func(t *testing.T, x *dcSide) []string {
				out := []string{
					x.send(t, "admin update s (299)", "/api/v1/config?action=update&id=difftest:s", 21, `{"s":1}`, dcAdmin),
					x.send(t, "update r (its echo is refused)", "/api/v1/config?action=update&id=difftest:r", 22, `{"r":1}`),
				}
				x.until(t, "difftest:r not rejected", "action=tree", func(b []byte) bool {
					return dcFlagRe("difftest:r", "plugin_rejected", true).Match(b)
				})
				return append(out,
					tree(t, x, "tree", 23),
					tree(t, x, "tree, admin", 24, dcAdmin),
					x.do(t, "v3", fnHTTPGet("/api/v3/config?action=tree", dcTx(25))),
					x.do(t, "v2", fnHTTPGet("/api/v2/config?action=tree", dcTx(26))),
					x.get(t, "path", "action=tree&path=/difftest/sub", 27),
					x.get(t, "raw prefix", "action=tree&path=/difftest/s", 28),
					x.get(t, "template id", "action=tree&id=difftest:t", 29),
					x.get(t, "path and id", "action=tree&path=/difftest/tmpl&id=difftest:t:j1", 30),
					x.get(t, "quoted id", "action=tree&id=a%27b", 31),
					x.get(t, "unknown id", "action=tree&id=nope", 32),
					x.do(t, "function", fnHTTPGet(fn("function=config%20tree"), dcTx(33))),
					x.do(t, "function, path and id", fnHTTPGet(fn("function=config%20tree%20%27/difftest%27%20%27difftest:s%27"), dcTx(34))),
				)
			},
			want: []string{
				fnQ(`"source":"User details hidden in anonymous mode. Sign in to access configuration details."`),
				fnQ(`"attention":{"degraded":true,"restart_required":1,"plugin_rejected":1,"status_failed":1,"status_incomplete":1}`),
				"DYNCFG: received response code 400 on request to id 'difftest:r', cmd: update",
			},
		}
	}

	// a single node's commands, forwarded to the plugin (dyncfg-intercept.c:538-566) with the request's transaction and
	// its 120 s; the update answers 200, 202, 298 and 299 set the status (dyncfg-internals.h:156-177) and save the file;
	// a user's enable and disable toggle user_disabled (saved when it changes, never the status); restart, userconfig
	// with a name; test without a name (the id split at its last colon: `config difftest test x`, the catch-all's
	// 404) and with one
	{
		x1 := plugin.ConfigCreate("difftest:x", "accepted", "single", "/difftest", "internal", "internal",
			"get schema update enable disable restart test userconfig", 0x8, 0x8)
		upd := func(code string) plugin.ServeRule {
			return dcTimes(dcRule("update-"+code, `^config difftest:x update$`, code, dcOK), 1)
		}
		cases["single"] = dcCase{
			sc:    dcScenarioServing(dcServe(upd("200"), upd("202"), upd("298"), upd("299")), x1),
			ready: map[string]string{"difftest:x": "running"},
			play: func(t *testing.T, x *dcSide) []string {
				q := "action=tree&id=difftest:x"
				upd := func(label string, n int, body string) string {
					return x.send(t, label, "/api/v1/config?action=update&id=difftest:x", n, body)
				}
				return []string{
					x.get(t, "get", "action=get&id=difftest:x", 41),
					x.get(t, "schema (no file: forwarded)", "action=schema&id=difftest:x", 42),
					upd("update 200", 43, `{"v":1}`),
					x.get(t, "tree after 200", q, 44),
					upd("update 202", 45, `{"v":2}`),
					upd("update 298", 46, `{"v":3}`),
					x.get(t, "tree after 298", q, 47),
					upd("update 299", 48, "{\"v\":4}\n"),
					x.get(t, "tree after 299", q, 49),
					x.get(t, "disable", "action=disable&id=difftest:x", 50),
					x.get(t, "tree after disable", q, 51),
					x.get(t, "disable again", "action=disable&id=difftest:x", 52),
					x.get(t, "enable", "action=enable&id=difftest:x", 53),
					x.get(t, "restart", "action=restart&id=difftest:x", 54),
					x.send(t, "userconfig", "/api/v1/config?action=userconfig&id=difftest:x&name=u1", 55, `{"u":1}`),
					x.send(t, "test without a name", "/api/v1/config?action=test&id=difftest:x", 56, `{"t":1}`),
					x.send(t, "test", "/api/v1/config?action=test&id=difftest:x&name=t1", 57, `{"t":2}`),
					x.get(t, "tree at the end", q, 58),
				}
			},
			want: []string{
				`FUNCTION ` + dcTx(41) + ` 120 "config difftest:x get" "0x8" "method=none,role=any,permissions=0x8,ip=localhost"` + "\n",
				`FUNCTION_PAYLOAD ` + dcTx(43) + ` 120 "config difftest:x update" "0x8" "method=none,role=any,permissions=0x8,ip=localhost" "application/json"` + "\n{\"v\":1}\nFUNCTION_PAYLOAD_END\n",
				fnQ(`"restart_required":true`),
				fnQ(`"status":"disabled"`),
				fnQ(`"user_disabled":true`),
				"DYNCFG: unknown config id 'difftest' in call: 'config difftest test x'",
			},
		}
	}

	// errors: the plugin's 400, 404 and 500 relayed (ERR "plugin returned code", dyncfg-intercept.c:227-228), nothing
	// saved; DynCfg's own refusals before the plugin (dyncfg-intercept.c:378-453): a payload where none belongs, none
	// where one does, a command the node does not have (restart; add, which sanitize keeps off a single)
	{
		e := plugin.ConfigCreate("difftest:e", "running", "single", "/difftest", "internal", "internal", "get schema update", 0x8, 0x8)
		text500 := dcTimes(dcRule("get-500", `^config difftest:e get$`, "500", "boom\n"), 1)
		text500.Type = "text/plain"
		cases["single-errors"] = dcCase{
			sc: dcScenarioServing(dcServe(
				dcTimes(dcRule("get-400", `^config difftest:e get$`, "400", `{"status":400,"message":"difftest says no"}`+"\n"), 1),
				text500,
				dcTimes(dcRule("update-404", `^config difftest:e update$`, "404", `{"status":404,"message":"difftest lost it"}`+"\n"), 1),
				dcTimes(dcRule("update-500", `^config difftest:e update$`, "500", `{"status":500,"message":"difftest broke"}`+"\n"), 1),
			), e),
			ready: map[string]string{"difftest:e": "running"},
			play: func(t *testing.T, x *dcSide) []string {
				return []string{
					x.get(t, "get 400", "action=get&id=difftest:e", 61),
					x.get(t, "get 500", "action=get&id=difftest:e", 62),
					x.send(t, "update 404", "/api/v1/config?action=update&id=difftest:e", 63, `{"e":1}`),
					x.send(t, "update 500", "/api/v1/config?action=update&id=difftest:e", 64, `{"e":2}`),
					x.send(t, "get with a payload", "/api/v1/config?action=get&id=difftest:e", 65, `{"e":3}`),
					x.get(t, "update without a payload", "action=update&id=difftest:e", 66),
					x.get(t, "restart (not in cmds)", "action=restart&id=difftest:e", 67),
					x.send(t, "add on a single", "/api/v1/config?action=add&id=difftest:e&name=n", 68, `{"e":4}`),
					x.get(t, "tree", "action=tree&id=difftest:e", 69),
				}
			},
			want: []string{
				"DYNCFG: plugin returned code 400 to user initiated call: config difftest:e get",
				"DYNCFG: plugin returned code 404 to user initiated call: config difftest:e update",
				fnQ(`{"status":400,"message":"dyncfg functions intercept: this action does not require a payload"}`),
				fnQ(`{"status":400,"message":"dyncfg functions intercept: this action requires a payload"}`),
				fnQ(`{"status":400,"message":"dyncfg functions intercept: this command is not supported by this configuration node"}`),
				"DYNCFG: this command is not supported by the configuration node: config difftest:e restart",
			},
		}
	}

	// a template (dyncfg-intercept.c:349-400, :112-177): add without a name (`/config`'s "Invalid name"; the function
	// route reaches the intercept's "requires a name"), with an existing name; j2 added (the plugin registers it after
	// its answer, so it runs: K6's gap is the next line); j4 added and never registered by the plugin (K6: a job without
	// a method, orphan, the catch-all's 404); test j3 and userconfig j3 (the template's handler, :390-395); test without
	// a name on a job id (P5: `/config` splits the id); a job's schema (the template's file; none, so forwarded); the
	// template disabled: its fan-out echoes j4 too, which the catch-all answers 404 with two ERRs (K5)
	cases["template"] = dcCase{
		sc:    dcScenarioServing(dcServe(dcRule("add-unregistered", `^config difftest:t add j4$`, "200", dcOK)), dcCreates),
		ready: dcReady,
		play: func(t *testing.T, x *dcSide) []string {
			out := []string{
				x.send(t, "add without a name", "/api/v1/config?action=add&id=difftest:t", 71, `{"j":0}`),
				x.send(t, "add without a name, function", fn("function=config%20difftest:t%20add"), 72, `{"j":0}`),
				x.send(t, "add an existing name", "/api/v1/config?action=add&id=difftest:t&name=j1", 73, `{"j":1}`),
				x.send(t, "add j2", "/api/v1/config?action=add&id=difftest:t&name=j2", 74, `{"j":2}`),
			}
			x.waitTree(t, map[string]string{"difftest:t:j2": "running"})
			out = append(out,
				x.send(t, "add j4 (never registered)", "/api/v1/config?action=add&id=difftest:t&name=j4", 75, `{"j":4}`),
				x.send(t, "test j3", "/api/v1/config?action=test&id=difftest:t&name=j3", 76, `{"j":3}`),
				x.send(t, "test a job id without a name", "/api/v1/config?action=test&id=difftest:t:j9", 77, `{"j":9}`),
				x.send(t, "userconfig j3", "/api/v1/config?action=userconfig&id=difftest:t&name=j3", 78, `{"j":3}`),
				x.get(t, "a job's schema", "action=schema&id=difftest:t:j1", 79),
				x.get(t, "get j4", "action=get&id=difftest:t:j4", 80),
				x.get(t, "tree", "action=tree&id=difftest:t", 81),
				x.get(t, "disable the template (j4 has no method)", "action=disable&id=difftest:t", 82),
			)
			// the fan-out's echoes: j1's and j2's reach the plugin, j4's the catch-all (K5)
			x.served(t, 1, 2, "disable")
			x.waitTree(t, map[string]string{dcJ1: "disabled", "difftest:t:j2": "disabled"})
			return out
		},
		want: []string{
			fnHTTPError(400, "Invalid name"),
			fnQ(`{"status":400,"message":"dyncfg functions intercept: this action requires a name"}`),
			fnQ(`{"status":400,"message":"dyncfg functions intercept: a configuration with this name already exists"}`),
			`FUNCTION_PAYLOAD ` + dcTx(74) + ` 120 "config difftest:t add j2" "0x8"`,
			`FUNCTION_PAYLOAD ` + dcTx(77) + ` 120 "config difftest:t test j9" "0x8"`,
			`FUNCTION ` + dcTx(79) + ` 120 "config difftest:t:j1 schema" "0x8"`,
			"DYNCFG: unknown config id 'difftest:t:j4' in call: 'config difftest:t:j4 disable'",
			"DYNCFG: received response code 404 on request to id 'difftest:t:j4', cmd: disable",
		},
	}

	// a template's enable, disable and restart (dyncfg-intercept.c:479-518, :252-287): the plugin never sees them; the
	// template's user_disabled (its file saved when it changes) and one echo per job of it in dictionary order
	// (`enable` to a user-disabled job is `disable`); a job of a disabled template cannot be enabled; the progress row
	// of the fan-out's call (P6)
	cases["template-fanout"] = dcCase{
		sc:    dcScenario(dcCreates),
		ready: dcReady,
		play: func(t *testing.T, x *dcSide) []string {
			out := []string{x.send(t, "add j2", "/api/v1/config?action=add&id=difftest:t&name=j2", 91, `{"j":2}`)}
			x.waitTree(t, map[string]string{"difftest:t:j2": "running"})
			out = append(out,
				x.get(t, "disable j2", "action=disable&id=difftest:t:j2", 92),
				x.get(t, "disable the template", "action=disable&id=difftest:t", 93),
				x.report(t, "the template disable's progress", "/api/v2/progress?transaction="+dcTx(93)))
			x.served(t, 1, 3, "disable")
			x.waitTree(t, map[string]string{dcJ1: "disabled", "difftest:t:j2": "disabled"})
			out = append(out,
				x.get(t, "tree after the disable", "action=tree&id=difftest:t", 94),
				x.get(t, "enable j1", "action=enable&id=difftest:t:j1", 95),
				x.get(t, "enable the template", "action=enable&id=difftest:t", 96))
			x.served(t, 1, 4, "disable")
			x.waitTree(t, map[string]string{dcJ1: "running"})
			out = append(out, x.get(t, "restart the template", "action=restart&id=difftest:t", 97))
			x.served(t, 1, 2, "restart")
			return append(out, x.get(t, "tree at the end", "action=tree&id=difftest:t", 98))
		},
		want: []string{
			fnQ(`{"status":200,"message":"applied to all template job"}`),
			fnQ(`{"status":400,"message":"dyncfg functions intercept: this job belongs to disabled template"}`),
			"DYNCFG: cannot enable a job of a disabled template: config difftest:t:j1 enable",
			`FUNCTION RANDOM 10 "config difftest:t:j1 disable" "0x7ff"`,
			`FUNCTION RANDOM 10 "config difftest:t:j1 restart" "0x7ff"`,
		},
	}

	// a user-added job (dyncfg source, its file): get, update, disable; remove refused by the plugin (500: kept), then
	// accepted (file and node deleted; the plugin's following delete finds nothing, so `config <job>` stays registered,
	// D135.11, K2: the next get is the intercept's 404 "id is not found")
	cases["job"] = dcCase{
		sc:    dcScenarioServing(dcServe(dcTimes(dcRule("remove-500", `^config \S+ remove$`, "500", `{"status":500,"message":"difftest keeps it"}`+"\n"), 1)), dcCreates),
		ready: dcReady,
		play: func(t *testing.T, x *dcSide) []string {
			j2 := "difftest:t:j2"
			out := []string{x.send(t, "add j2", "/api/v1/config?action=add&id=difftest:t&name=j2", 101, `{"j":2}`)}
			x.waitTree(t, map[string]string{j2: "running"})
			out = append(out,
				x.get(t, "get", "action=get&id="+j2, 102),
				x.send(t, "update", "/api/v1/config?action=update&id="+j2, 103, `{"j":22}`),
				x.get(t, "disable", "action=disable&id="+j2, 104))
			out = append(out, x.files(t, "files before the removes")...)
			return append(out,
				x.get(t, "remove (500)", "action=remove&id="+j2, 105),
				x.get(t, "tree after the refused remove", "action=tree&id=difftest:t", 106),
				x.get(t, "remove", "action=remove&id="+j2, 107),
				x.get(t, "get after the remove", "action=get&id="+j2, 108),
				x.get(t, "tree after the remove", "action=tree&id=difftest:t", 109),
			)
		},
		want: []string{
			"DYNCFG: plugin returned code 500 to user initiated call: config difftest:t:j2 remove",
			fnQ(`{"status":404,"message":"dyncfg functions intercept: id is not found"}`),
		},
	}

	// schemas (dyncfg-intercept.c:519-533, dyncfg-files.c:345-371): the user config's `schema.d/<escaped id>.json`, then
	// its raw-named file, then the stock directory's (systemd-journal's ships there); a job takes its template's; none
	// found is forwarded; the answer is the file, application/json, expiring now
	{
		journal := plugin.ConfigCreate("systemd-journal:monitored-directories", "running", "single", "/logs/systemd-journal",
			"internal", "internal", "get schema update", 0x8, 0x8)
		single := func(id string) string {
			return plugin.ConfigCreate(id, "running", "single", "/difftest", "internal", "internal", "get schema", 0x8, 0x8)
		}
		cases["schema"] = dcCase{
			prepare: func(t *testing.T, runDir string) {
				dir := filepath.Join(runDir, "etc", "schema.d")
				dcWriteFile(t, filepath.Join(dir, "difftest%3As.json"), []byte("{\"title\":\"user schema of s\"}\n"))
				dcWriteFile(t, filepath.Join(dir, "difftest:r.json"), []byte(`{"title":"raw-named schema of r"}`))
				dcWriteFile(t, filepath.Join(dir, "difftest%3At.json"), []byte("{\"title\":\"user schema of t\"}\n"))
			},
			sc:    dcScenario(dcCreates + journal + single("difftest:r") + single("difftest:n")),
			ready: map[string]string{dcS: "running", dcJ1: "running", "difftest:r": "running", "difftest:n": "running"},
			play: func(t *testing.T, x *dcSide) []string {
				return []string{
					x.get(t, "escaped name", "action=schema&id=difftest:s", 111),
					x.get(t, "raw name", "action=schema&id=difftest:r", 112),
					x.get(t, "template", "action=schema&id=difftest:t", 113),
					x.get(t, "job", "action=schema&id=difftest:t:j1", 114),
					x.get(t, "stock", "action=schema&id=systemd-journal:monitored-directories", 115),
					x.get(t, "none (forwarded)", "action=schema&id=difftest:n", 116),
					x.do(t, "function", fnHTTPGet(fn("function=config%20difftest:s%20schema"), dcTx(117))),
				}
			},
			want:    []string{fnQ(`{"title":"user schema of s"}`), fnQ(`{"title":"raw-named schema of r"}`), `"config difftest:n schema"`},
			wantNot: []string{`"config difftest:s schema"`, `"config difftest:t:j1 schema"`},
		}
	}

	// `/api/v1/config`'s parameters (api_v1_config.c:5-81): a missing, spaced or quoted id; an unknown action; an
	// invalid name; a multi-word action (K1, P3: `config <id> (null)`); timeout 3 (raised to 10), garbage (10), 500;
	// an empty action (skipped: the tree, filtered by the id); `&&` and an empty timeout skipped; the last value wins;
	// an unknown id (the catch-all's 404)
	cases["params"] = dcCase{
		sc:    dcScenario(dcCreates),
		ready: dcReady,
		play: func(t *testing.T, x *dcSide) []string {
			reqs := []struct{ label, query string }{
				{"no id", "action=get"},
				{"spaced id", "action=get&id=a%20b"},
				{"quoted id", "action=get&id=a%27b"},
				{"unknown action", "action=bogus&id=difftest:s"},
				{"invalid name", "action=add&id=difftest:t&name=a%20b"},
				{"multi-word action", "action=get%20update&id=difftest:s"},
				{"timeout 3", "action=get&id=difftest:s&timeout=3"},
				{"timeout garbage", "action=get&id=difftest:s&timeout=abc"},
				{"timeout 500", "action=get&id=difftest:s&timeout=500"},
				{"empty action", "action=&id=difftest:s"},
				{"empty pairs", "&&action=get&&id=difftest:s&timeout="},
				{"last wins", "action=get&id=difftest:s&action=schema&id=difftest:t"},
				{"unknown id", "action=get&id=nope"},
				{"no parameters", ""},
			}
			var out []string
			for i, r := range reqs {
				out = append(out, x.get(t, r.label, r.query, 121+i))
			}
			return out
		},
		want: []string{
			fnHTTPError(400, "Invalid id"),
			fnHTTPError(400, "Invalid action"),
			fnHTTPError(400, "Invalid name"),
			`FUNCTION ` + dcTx(127) + ` 10 "config difftest:s get"`,
			`FUNCTION ` + dcTx(129) + ` 500 "config difftest:s get"`,
		},
	}

	// the same through `/api/v1|v3/function` (ACL FUNCTIONS; DynCfg methods are not restricted, nrpc-catalog.c:44-53):
	// the method's 120 s or the request's timeout, no id or action check before the intercept (its own texts); `config`
	// alone (the catch-all's "expected: config tree"); a quoted multi-word command (K1b); a payload; an unknown id; no
	// command; a name after get (forwarded as written); an add; v2 has no function
	cases["function-api"] = dcCase{
		sc:    dcScenario(dcCreates),
		ready: dcReady,
		play: func(t *testing.T, x *dcSide) []string {
			out := []string{
				x.do(t, "get", fnHTTPGet(fn("function=config%20difftest:s%20get"), dcTx(141))),
				x.do(t, "get, v3", fnHTTPGet("/api/v3/function?function=config%20difftest:s%20get", dcTx(142))),
				x.do(t, "get, timeout", fnHTTPGet(fn("function=config%20difftest:s%20get&timeout=5"), dcTx(143))),
				x.do(t, "config alone", fnHTTPGet(fn("function=config"), dcTx(144))),
				x.do(t, "quoted multi-word", fnHTTPGet(fn("function=config%20difftest:s%20%27get%20schema%27"), dcTx(145))),
				x.send(t, "update", fn("function=config%20difftest:s%20update"), 146, `{"f":1}`),
				x.do(t, "unknown id", fnHTTPGet(fn("function=config%20bogus%20get"), dcTx(147))),
				x.do(t, "no command", fnHTTPGet(fn("function=config%20difftest:s"), dcTx(148))),
				x.do(t, "a name after get", fnHTTPGet(fn("function=config%20difftest:s%20get%20extra"), dcTx(149))),
				x.send(t, "add", fn("function=config%20difftest:t%20add%20j5"), 150, `{"j":5}`),
				x.do(t, "v2", fnHTTPGet("/api/v2/function?function=config%20tree", dcTx(151))),
			}
			// j5's registration and its echo, before the stop
			x.served(t, 1, 3, "enable")
			return out
		},
		want: []string{
			`FUNCTION ` + dcTx(141) + ` 120 "config difftest:s get"`,
			`FUNCTION ` + dcTx(143) + ` 5 "config difftest:s get"`,
			fnQ(`{"status":400,"message":"invalid function call, expected: config tree"}`),
			fnQ(`{"status":500,"message":"dyncfg: permissions for this command are not set"}`),
			fnQ(`{"status":400,"message":"dyncfg functions intercept: invalid command received"}`),
		},
	}

	// access (dyncfg-intercept.c:405-439, nrpc-calls.c:504-579): difftest:a views with 0x8 and edits with 0x13
	// (signed-in, same-space, sensitive data); difftest:b registered 0x0/0x0 takes C's defaults (0x23/0x47; the method
	// 0x3), so nRPC refuses anonymous (412) and member (403) clients before DynCfg; the dashboard ACL (`allow dashboard
	// from = localhost`, a client from 127.0.0.2) refuses `/api/v1/config` and the function route
	cases["access"] = dcCase{
		adjust: func(o *daemon.Options) { o.WebExtra = "    allow dashboard from = localhost\n" },
		sc: dcScenario(plugin.ConfigCreate("difftest:a", "running", "single", "/difftest", "internal", "internal", "get update", 0x8, 0x13) +
			plugin.ConfigCreate("difftest:b", "running", "single", "/difftest", "internal", "internal", "get update", 0, 0)),
		ready: map[string]string{"difftest:a": "running", "difftest:b": "running"},
		play: func(t *testing.T, x *dcSide) []string {
			from := func(label, target string, n int) string {
				b, err := rawExchangeFrom("127.0.0.2", x.d.Addr, fnHTTPGet(target, dcTx(n)), fnWait)
				if err != nil {
					t.Errorf("%s: %s: %v", x.role, label, err)
				}
				return label + ": " + fmt.Sprintf("%q", fnHTTPMask(b))
			}
			return []string{
				x.get(t, "anonymous get a", "action=get&id=difftest:a", 161),
				x.send(t, "anonymous update a", "/api/v1/config?action=update&id=difftest:a", 162, `{"a":1}`),
				x.send(t, "member update a", "/api/v1/config?action=update&id=difftest:a", 163, `{"a":2}`, dcMember),
				x.send(t, "admin update a", "/api/v1/config?action=update&id=difftest:a", 164, `{"a":3}`, dcAdmin),
				x.get(t, "anonymous get b", "action=get&id=difftest:b", 165),
				x.get(t, "member get b", "action=get&id=difftest:b", 166, dcMember),
				x.get(t, "admin get b", "action=get&id=difftest:b", 167, dcAdmin),
				x.send(t, "admin update b", "/api/v1/config?action=update&id=difftest:b", 168, `{"b":1}`, dcAdmin),
				x.get(t, "tree", "action=tree", 169),
				from("tree from 127.0.0.2", "/api/v1/config?action=tree", 170),
				from("function from 127.0.0.2", "/api/v1/function?function=config%20tree", 171),
			}
		},
		want: []string{
			fnQ(`{"status":403,"message":"dyncfg: you don't have enough edit permissions to execute this command"}`),
			fnHTTPError(412, "You need to be authenticated via Netdata Cloud Single-Sign-On (SSO) to access this feature. "+
				"Sign-in on this dashboard, or access your Netdata via https://app.netdata.cloud."),
			`"config difftest:a update" "0x7ff" "method=api-bearer,role=admin,permissions=0x7ff,user=fnhttp-admin,`,
			fnQ(`"view":["signed-in","same-space","view-config"]`),
		},
	}

	// orphans (dyncfg-tree.c:93-94, :216-262): start 1 registers and exits on the release; start 2 waits before its
	// registration. Before: a saved single, an admin-saved node with C's default access, a user-added job. As orphans:
	// get and update are the catch-all's 404 (P4); an anonymous remove of the default-access orphan is refused (edit
	// 0x47), an admin's deletes it; a loaded orphan (seeded, no access in its file: 0, K8) is removed by an anonymous
	// client (P11, Q12); the job's remove (its template's 0x8). Start 2 registers again: the saved single's echoes.
	{
		p := plugin.ConfigCreate("difftest:p", "running", "single", "/difftest", "internal", "internal", "get update", 0, 0)
		cases["orphan"] = dcCase{
			starts: 2,
			prepare: func(t *testing.T, runDir string) {
				dcWriteFile(t, filepath.Join(dcConfigDir(runDir), "difftest%3Aseeded.dyncfg"), dcFile("difftest:seeded", "",
					"/difftest", "single", "dyncfg", "seeded source", 1600000000000001, 1600000000000002, false, 1, "get update ",
					`{"seeded":1}`))
			},
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{dcServe(), {Emit: dcCreates + p}, {WaitFile: "exit"}, {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{dcServe(), {WaitFile: "go"}, {Emit: dcCreates}}},
			}},
			ready: map[string]string{dcS: "running", dcJ1: "running", "difftest:p": "running", "difftest:seeded": "orphan"},
			play: func(t *testing.T, x *dcSide) []string {
				out := []string{
					x.get(t, "tree with the loaded orphan", "action=tree", 181),
					x.send(t, "update s", "/api/v1/config?action=update&id=difftest:s", 182, `{"s":1}`),
					x.send(t, "admin update p", "/api/v1/config?action=update&id=difftest:p", 183, `{"p":1}`, dcAdmin),
					x.send(t, "add j2", "/api/v1/config?action=add&id=difftest:t&name=j2", 184, `{"j":2}`),
				}
				x.waitTree(t, map[string]string{"difftest:t:j2": "running"})
				x.release(t, x.l, "exit")
				x.step(t, x.l, "start 2 is not waiting", func(s [][]plugin.Record) bool {
					return len(s) >= 2 && plugin.Has(s[1], "waiting", "go")
				})
				out = append(out,
					x.get(t, "tree of orphans", "action=tree", 185),
					x.get(t, "get an orphan", "action=get&id=difftest:s", 186),
					x.send(t, "update an orphan", "/api/v1/config?action=update&id=difftest:s", 187, `{"s":2}`),
					x.get(t, "anonymous remove of p", "action=remove&id=difftest:p", 188),
					x.get(t, "admin remove of p", "action=remove&id=difftest:p", 189, dcAdmin),
					x.get(t, "anonymous remove of the loaded orphan", "action=remove&id=difftest:seeded", 190),
					x.get(t, "remove of the job", "action=remove&id=difftest:t:j2", 191),
					x.get(t, "tree after the removes", "action=tree", 192))
				out = append(out, x.files(t, "files after the removes")...)
				x.release(t, x.l, "go")
				x.served(t, 2, 1, "update")
				x.waitTree(t, dcReady)
				return append(out, x.get(t, "tree after start 2", "action=tree", 193))
			},
			want: []string{
				fnQ(`"difftest:seeded":{"type":"single","status":"orphan","cmds":["remove"],"access":{"view":[],"edit":[]}`),
				fnQ(`{"status":403,"message":"dyncfg: you don't have enough edit permissions to execute this command"}`),
				"DYNCFG: unknown config id 'difftest:s' in call: 'config difftest:s get'",
			},
		}
	}

	// a vnode's scope does not move a CONFIG (pluginsd_dyncfg.c:10: always localhost); the vnode has no `config`
	// (dyncfg-tree.c:304-323 only on localhost), so its routes are nRPC's 404
	cases["vnode"] = dcCase{
		sc: dcScenario(vnodeDefine(vnodeGUID, vnodeName, vnodeLabels...) + "HOST " + vnodeGUID + "\n" +
			plugin.ConfigCreate("difftest:v", "running", "single", "/difftest/v", "internal", "internal", "get update", 0x8, 0x8) +
			"HOST localhost\n"),
		ready: map[string]string{"difftest:v": "running"},
		play: func(t *testing.T, x *dcSide) []string {
			v := "/host/" + vnodeName
			return []string{
				x.do(t, "vnode tree", fnHTTPGet(v+"/api/v1/config?action=tree", dcTx(201))),
				x.do(t, "vnode get", fnHTTPGet(v+"/api/v1/config?action=get&id=difftest:v", dcTx(202))),
				x.do(t, "vnode function", fnHTTPGet(v+"/api/v1/function?function=config%20difftest:v%20get", dcTx(203))),
				x.get(t, "localhost tree", "action=tree", 204),
				x.get(t, "localhost get", "action=get&id=difftest:v", 205),
			}
		},
		want: []string{fnHTTPError(404, "This feature is not available on this host at this time.")},
	}

	// the user-action record (dyncfg-intercept.c:39-107, P7): forwarded commands from the plugin's thread, a template's
	// from the web worker; a bearer user with X-Forwarded-For (one with a comma: user-auth.c's parser keeps the first
	// part), anonymous and member clients; add and test with a name; none for get, schema, userconfig
	cases["logs"] = dcCase{
		sc:    dcScenario(dcCreates),
		ready: dcReady,
		play: func(t *testing.T, x *dcSide) []string {
			out := []string{
				x.send(t, "admin update, forwarded", "/api/v1/config?action=update&id=difftest:s", 211, `{"l":1}`, dcAdmin,
					"X-Forwarded-For: 10.1.2.3"),
				x.send(t, "admin update, forwarded with a comma", "/api/v1/config?action=update&id=difftest:s", 212, `{"l":2}`,
					dcAdmin, "X-Forwarded-For: 10.1.2.3, 10.4.5.6"),
				x.get(t, "anonymous disable", "action=disable&id=difftest:s", 213),
				x.get(t, "member enable", "action=enable&id=difftest:s", 214, dcMember),
				x.get(t, "get", "action=get&id=difftest:s", 215),
				x.get(t, "schema", "action=schema&id=difftest:s", 216),
				x.send(t, "userconfig", "/api/v1/config?action=userconfig&id=difftest:t&name=u", 217, `{"u":1}`),
				x.send(t, "add j2", "/api/v1/config?action=add&id=difftest:t&name=j2", 218, `{"j":2}`),
			}
			x.waitTree(t, map[string]string{"difftest:t:j2": "running"})
			out = append(out,
				x.send(t, "test j3", "/api/v1/config?action=test&id=difftest:t&name=j3", 219, `{"j":3}`),
				x.get(t, "template disable", "action=disable&id=difftest:t", 220, dcAdmin),
				x.get(t, "template enable", "action=enable&id=difftest:t", 221))
			x.served(t, 1, 3, "disable")
			x.served(t, 1, 6, "enable")
			return out
		},
		want: []string{
			"DYNCFG USER ACTION 'update' on 'difftest:s' by user 'fnhttp-admin', IP '10.1.2.3'",
			"DYNCFG USER ACTION 'add' j2 on template 'difftest:t'",
			"DYNCFG USER ACTION 'disable' on template 'difftest:t' by user 'fnhttp-admin'",
		},
		wantNot: []string{"DYNCFG USER ACTION 'get'", "DYNCFG USER ACTION 'schema'", "DYNCFG USER ACTION 'userconfig'"},
	}

	// an empty id (P10, K10): `CONFIG '' create` passes the id check (inicfg/dyncfg.c:203-212), but C's dictionary
	// takes no empty name, so the add reports "the dyncfg registry is not available" (dyncfg.c:424-431) and is refused:
	// the plugin's run ends (K7) and its nodes are orphans until start 2 registers them again; the catch-all `config`
	// is untouched (the plan's hijack does not happen)
	{
		trigger := dcRule("empty", `^config difftest:s restart$`, "200", dcOK)
		trigger.Then = "CONFIG '' create 'running' 'single' '/difftest/empty' 'internal' 'internal' 'get' 0x8 0x8\n"
		cases["empty-id"] = dcCase{
			starts: 2,
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{dcServe(trigger), {Emit: dcCreates}}},
				{Steps: []plugin.Step{dcServe(), {Emit: dcCreates}}},
			}},
			ready: dcReady,
			play: func(t *testing.T, x *dcSide) []string {
				out := []string{x.get(t, "restart s (the plugin registers the empty id)", "action=restart&id=difftest:s", 231)}
				if !x.served(t, 2, 2, "enable") || !x.waitTree(t, dcReady) {
					return out
				}
				return append(out,
					x.get(t, "tree", "action=tree", 232),
					x.do(t, "function tree", fnHTTPGet(fn("function=config%20tree"), dcTx(233))),
					x.get(t, "get s", "action=get&id=difftest:s", 234),
					x.get(t, "get an unknown id", "action=get&id=nope", 235),
				)
			},
			want: []string{
				"DYNCFG: cannot add configuration '' - the dyncfg registry is not available",
				"parser_action('CONFIG') failed on line",
				fnQ(`{"status":404,"errorMessage":"Unknown config id given."}`),
			},
		}
	}
	return cases
}
