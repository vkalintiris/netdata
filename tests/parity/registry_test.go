// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// regNode is a host as the registry prints it: its machine GUID and its registry hostname (localhost's is
// `[registry] registry hostname`, rrd.c:152, registry_init.c:83; a child's is the `registry_hostname` it sends,
// rrdhost.c:238, which the fixture child sets to its hostname, stream.go).
type regNode struct{ guid, hostname string }

var (
	regParent  = regNode{parentIdentity.MachineGUID, parentIdentity.Hostname}
	regChildOf = regNode{childHost.MachineGUID, childHost.Hostname}
)

// What C answers `/api/v1/registry` with (registry disabled: `[registry] enabled = no`, the launcher's setting and
// C's default, registry_init.c:59): the status lines, the announced registry's default (registry_init.c:82) and the
// 400 texts (api_v1_registry.c:138, :150, :161, :172, :183, :195; web_api.c:72-77, :103-106).
const (
	regPath     = "/api/v1/registry"
	regAnnounce = "https://registry.my-netdata.io"
	regOK       = "HTTP/1.1 200 OK\r\n"
	regBad      = "HTTP/1.1 400 Bad Request\r\n"
	regBody     = "\r\n\r\n"
	regEnd      = "}\n"
	regNoAction = "Invalid registry request - you need to set an action: hello, access, delete, search"
	regDNT      = "Your web browser is sending 'DNT: 1' (Do Not Track). The registry requires persistent cookies on " +
		"your browser to work."
	regTkN = "\r\nTk: N\r\n"
	regTkT = "\r\nTk: T;cookies\r\n"
	// regCloud is the cloud URL of an agent whose cloud.conf sets none (common.h:268, cloud-conf.c:7-9)
	regCloud = "https://app.netdata.cloud"
)

// regHelloGuard judges a hello (registry.c:178-226): `"status":"ok"`, the routed host's GUID and registry hostname
// in the header, localhost's GUID as the agent's and no bearer protection, the registry's copy of cloud.conf's URL
// (registry.c:201-203), the announced registry, and `nodes` exactly these hosts in this order: C walks its host
// index in insertion order (`rrdhost_root_index` is created without DICT_OPTION_ADD_IN_FRONT, rrdhost.c:121-126;
// registry.c:209-221), localhost first, then the child once it connected. No `node_id` anywhere: a fresh run
// directory has none (rrdhost.c:554).
func regHelloGuard(host regNode, announce, cloud string, nodes ...regNode) func(Value) error {
	var list []string
	for _, n := range nodes {
		list = append(list, fmt.Sprintf(`{"machine_guid":%q,"hostname":%q}`, n.guid, n.hostname))
	}
	return dashGuard(dashMembers(nil, "action", `"hello"`, "status", `"ok"`, "hostname", strconv.Quote(host.hostname),
		"machine_guid", strconv.Quote(host.guid),
		"agent", fmt.Sprintf(`{"machine_guid":%q,"bearer_protection":false}`, regParent.guid),
		"cloud_base_url", strconv.Quote(cloud), "registry", strconv.Quote(announce), "X-Netdata-Auth", "true",
		"nodes", "["+strings.Join(list, ",")+"]"))
}

// regOffGuard judges C's `disabled` document for action, whole (registry.c:58-80): the routed host's header and the
// announced registry.
func regOffGuard(action string, host regNode, announce string) func(Value) error {
	return dashIs(fmt.Sprintf(`{"action":%q,"status":"disabled","hostname":%q,"machine_guid":%q,"registry":%q}`, action,
		host.hostname, host.guid, announce))
}

// regHello is a hello row asked of localhost, judged by regHelloGuard with the default announce and cloud URLs.
func regHello(name, target string, nodes []regNode, headers ...string) exactReq {
	return exactReq{name: name, target: target, headers: headers, want: [2]string{regOK, regEnd},
		guard: regHelloGuard(regParent, regAnnounce, regCloud, nodes...)}
}

// regText is a 400 row with C's exact text.
func regText(name, target, text string, headers ...string) exactReq {
	return exactReq{name: name, target: target, headers: headers, want: [2]string{regBad, regBody + text}}
}

// regOff is a row C answers with its `disabled` document for action, for localhost.
func regOff(name, target, action string, headers ...string) exactReq {
	return exactReq{name: name, target: target, headers: headers, want: [2]string{regOK, regEnd},
		guard: regOffGuard(action, regParent, regAnnounce)}
}

// TestRegistryAPI (check `api.registry`, M10 commit 1, D223, D224, D226): `/api/v1/registry` with the registry
// disabled, C's default. Every answer is compared raw after maskAnswer (each side's clock and expiry read against its
// own flight): the bodies hold no clock and nothing random (registry.c:58-80, :178-226), the hosts are fixed and there
// is no node id on a fresh run directory. Configurations: `default` (then a connected child, then the same child
// gone: its host stays in the index), `custom` (the registry's hostname and announce URL), `dnt` (the Do Not Track
// policy), `reload` (cloud.conf's URL changed, then a claim reload) and `access` (the handler's own ACL checks and
// bearer protection). Green on Rust since milestone 10 commit 1.
func TestRegistryAPI(t *testing.T) {
	t.Run("default", func(t *testing.T) {
		p := dashPair(t, daemon.Options{})
		compareExacts(t, p, regDefaultRows())
		conns := dashChild(t, p, dashBase())
		t.Run("child", func(t *testing.T) { compareExacts(t, p, regChildRows()) })
		for _, c := range conns {
			_ = c.Close()
		}
		regChildGone(t, p)
		t.Run("gone", func(t *testing.T) { compareExacts(t, p, regChildRows()) })
	})
	t.Run("custom", func(t *testing.T) {
		const announce = "http://parity.invalid/registry"
		p := dashPair(t, daemon.Options{ConfExtra: "[registry]\n" +
			"    registry hostname = parity-registry\n" +
			"    registry to announce = " + announce + "\n"})
		host := regNode{regParent.guid, "parity-registry"}
		compareExacts(t, p, []exactReq{
			{name: "hello", target: regPath + "?action=hello", want: [2]string{regOK, regEnd},
				guard: regHelloGuard(host, announce, regCloud, host)},
			{name: "search", target: regPath + "?action=search&for=m", want: [2]string{regOK, regEnd},
				guard: regOffGuard("search", host, announce)},
		})
	})
	t.Run("dnt", func(t *testing.T) {
		// the policy lets `DNT:` set the flag (http_header.c:74-79) and adds `Tk:` to every answer
		// (web_client.c:996-1010): `T;cookies` once the handler asked for tracking (api_v1_registry.c:154)
		p := dashPair(t, daemon.Options{WebExtra: "    respect do not track policy = yes\n"})
		access := regPath + "?action=access&machine=m&url=http://x&name=n"
		parent := []regNode{regParent}
		with := func(r exactReq, holds ...string) exactReq { r.holds = holds; return r }
		compareExacts(t, p, []exactReq{
			// hello ignores DNT but in `anonymous_statistics`, false here either way (the launcher's opt-out file)
			with(regHello("dnt-hello", regPath+"?action=hello", parent, "DNT: 1"), regTkN),
			with(regHello("dnt0-hello", regPath+"?action=hello", parent, "DNT: 0"), regTkN),
			// every other action, a missing one too, is refused before its parameters (api_v1_registry.c:136-140)
			with(regText("dnt-access", access, regDNT, "DNT: 1"), regTkN),
			with(regText("dnt-no-action", regPath, regDNT, "DNT: 1"), regTkN),
			with(regText("dnt-search-missing", regPath+"?action=search", regDNT, "DNT: 1"), regTkN),
			with(regOff("access", access, "access"), regTkT),
			// `DNT: 0` is no refusal (the flag is set for `DNT: 1` alone, http_header.c:74-77): the handler asks for
			// tracking as for a client without the header
			with(regOff("dnt0-search", regPath+"?action=search&for=m", "search", "DNT: 0"), regTkT),
			with(regText("access-missing", regPath+"?action=access&machine=m&url=u",
				"Invalid registry Access request."), regTkN),
		})
	})
	t.Run("reload", func(t *testing.T) {
		// hello's `cloud_base_url` is the registry's copy of cloud.conf's URL, taken at start (registry_init.c:87) and
		// again at a claim reload, after cloud.conf is read again (claim.c:202-204, registry.c:158-166): a changed
		// file alone changes nothing (D223, D226)
		p := dashPair(t, daemon.Options{})
		regCloudConf(t, p, regReloadURL)
		parent := []regNode{regParent}
		compareExacts(t, p, []exactReq{regHello("before", regPath+"?action=hello", parent)})
		regReload(t, p)
		after := regHello("after", regPath+"?action=hello", parent)
		after.guard = regHelloGuard(regParent, regAnnounce, regReloadURL, parent...)
		compareExacts(t, p, []exactReq{after})
	})
	t.Run("access", func(t *testing.T) { accessRows(t, regAccessConfs, regAccessRows) })
}

// regReloadURL is the cloud URL the reload stage writes into cloud.conf (a reserved name: nothing resolves it).
const regReloadURL = "https://parity.invalid"

// regCloudConf writes a cloud.conf whose `[global] url` is url into each side's `<varlib>/cloud.d` (`[directories]
// cloud`'s default, the launcher's `lib`), the file C reads at start and at a claim reload (cloud-conf.c:71-92).
func regCloudConf(t *testing.T, p *Pair, url string) {
	t.Helper()
	for _, side := range p.Each() {
		dir := filepath.Join(side.Daemon.Opts.RunDir, "lib", "cloud.d")
		if err := os.MkdirAll(dir, 0o770); err != nil {
			t.Fatal(err)
		}
		conf := []byte("[global]\n    url = " + url + "\n")
		if err := os.WriteFile(filepath.Join(dir, "cloud.conf"), conf, 0o640); err != nil {
			t.Fatal(err)
		}
	}
}

// regReloadAnswer is what `netdatacli reload-claiming-state` prints for an agent that is not claimed and never tried
// to (commands.c:222-241, claim.c:25-29; cli.c:67-79 prints the message on stdout).
var regReloadAnswer = cliResult{Stdout: "Netdata Agent is not claimed to Netdata Cloud: Agent is not claimed yet\n"}

// regReload runs `netdatacli reload-claiming-state` against each side, the oracle first: the command answers once the
// registry has its new copy (claim.c:197-205, unclaimed: no wait). The oracle's client must print regReloadAnswer
// and exit 0, and the candidate's the same.
func regReload(t *testing.T, p *Pair) {
	t.Helper()
	var got [2]cliResult
	for i, side := range p.Each() {
		got[i] = runCLI(t, side.Daemon, "reload-claiming-state")
	}
	if got[0] != regReloadAnswer {
		t.Fatalf("oracle: netdatacli reload-claiming-state: %+v, want %+v", got[0], regReloadAnswer)
	}
	if got[1] != got[0] {
		t.Errorf("netdatacli reload-claiming-state differs\noracle:    %+v\ncandidate: %+v", got[0], got[1])
	}
}

// regDefaultRows are P23's rows of the default configuration (plan m10 commit 1, 4.2), before any child.
func regDefaultRows() []exactReq {
	parent := []regNode{regParent}
	full := "&machine=m&url=http://x"
	cookie := "Cookie: netdata_registry_id=11111111-2222-3333-4444-555555555555"
	post := regHello("post", regPath+"?action=hello", parent)
	post.method, post.body = "POST", []byte("{}")
	return []exactReq{
		regHello("hello", regPath+"?action=hello", parent),
		regHello("hello-host", "/host/parity-parent"+regPath+"?action=hello", parent),
		regHello("hello-extra", regPath+"?action=hello&machine=m&url=u&name=n&for=f", parent),
		// an unknown action leaves the action as it was (api_v1_registry.c:80-91)
		regHello("hello-repeat", regPath+"?action=hello&action=bogus", parent),
		regHello("hello-repeat2", regPath+"?action=bogus&action=hello", parent),
		regHello("hello-encoded", regPath+"?action=%68ello", parent),
		// the method is not checked and the body is ignored
		post,
		// what the dashboard sends (index.html, credentials "include"); DNT is ignored with the policy off
		regHello("browser", regPath+"?action=hello", parent, cookie, "DNT: 1", "Cache-Control: no-cache"),
		regText("no-action", regPath, regNoAction),
		regText("unknown", regPath+"?action=bogus", regNoAction),
		regText("empty", regPath+"?action=", regNoAction),
		regText("case", regPath+"?action=HELLO", regNoAction),
		regText("access-missing", regPath+"?action=access&machine=m&url=u", "Invalid registry Access request."),
		// `name` before `action` is not the access's name (api_v1_registry.c:102-105)
		regText("access-order", regPath+"?name=n&action=access&machine=m&url=u", "Invalid registry Access request."),
		regText("access-empty", regPath+"?action=access&machine=m&url=u&name=", "Invalid registry Access request."),
		regText("delete-missing", regPath+"?action=delete&machine=m&url=u", "Invalid registry Delete request."),
		regText("search-missing", regPath+"?action=search", "Invalid registry Search request."),
		regText("switch-missing", regPath+"?action=switch&machine=m&url=u", "Invalid registry Switch request."),
		regOff("access", regPath+"?action=access"+full+"&name=n", "access"),
		// the person's cookie changes nothing while the registry is disabled (api_v1_registry.c:47-52)
		regOff("access-cookie", regPath+"?action=access"+full+"&name=n", "access", cookie),
		// the disabled answer comes before the URL's check (registry.c:233-239)
		regOff("access-bad-url", regPath+"?action=access&machine=m&url=bad&name=n", "access"),
		regOff("delete", regPath+"?action=delete"+full+"&delete_url=d", "delete"),
		regOff("search", regPath+"?action=search&for=m", "search"),
		// `DNT: 1` is read only under `[web] respect do not track policy` (http_header.c:74-77): with the policy off
		// the request is `search`'s, not the refusal the `dnt` stage's rows get (api_v1_registry.c:123, :136-140)
		regOff("search-dnt", regPath+"?action=search&for=m", "search", "DNT: 1"),
		regOff("switch", regPath+"?action=switch"+full+"&to=p", "switch"),
		regText("subpath", regPath+"/hello", "API command 'registry' does not support subpaths."),
		{name: "v3", target: "/api/v3/registry?action=hello",
			want: [2]string{"HTTP/1.1 404 Not Found\r\n", regBody + "Unsupported API command: registry"}},
	}
}

// regChildRows are the rows of the child stage: the hello lists both hosts; through `/host/<child>/` the document's
// header is the child's and its `agent` is localhost's (registry.c:58-66, :184-197).
func regChildRows() []exactReq {
	host := "/host/" + childHost.Hostname + regPath
	both := []regNode{regParent, regChildOf}
	return []exactReq{
		regHello("hello", regPath+"?action=hello", both),
		{name: "host-hello", target: host + "?action=hello", want: [2]string{regOK, regEnd},
			guard: regHelloGuard(regChildOf, regAnnounce, regCloud, both...)},
		{name: "host-search", target: host + "?action=search&for=m", want: [2]string{regOK, regEnd},
			guard: regOffGuard("search", regChildOf, regAnnounce)},
	}
}

// regChildGone waits until each side counts the child as no longer received (`/api/v2/info` nodes), within 30 s;
// the oracle must, or the stage did not run; a candidate that does not is reported, and the rows after it show what
// it answers.
func regChildGone(t *testing.T, p *Pair) {
	t.Helper()
	for _, side := range p.Each() {
		var last string
		gone := waitFor(30*time.Second, func() bool {
			b, err := rawExchange(side.Daemon.Addr, []byte("GET /api/v2/info HTTP/1.1\r\nHost: localhost\r\n\r\n"),
				5*time.Second)
			if err != nil {
				return false
			}
			// the answer is pretty-printed: its counts without the white space, as compareInfoV2 reads them
			last = strings.Join(strings.Fields(infoV2NodesRe.FindString(string(b))), "")
			if strings.Contains(last, `"total":2,"receiving":0`) {
				return true
			}
			time.Sleep(time.Second)
			return false
		})
		if gone {
			continue
		}
		if side.Role == Oracle {
			t.Fatalf("oracle: the child is still received: %s", last)
		}
		t.Errorf("candidate: the child is still received after 30 s: %s", last)
	}
}

// The `access` subtest's configurations (D223 F5 B, D224 F1 A; P23 4.3), from 127.0.0.2 but `bearer`: the dashboard
// list without the client (`acl`), the registry list without it (`registry-denied`), every list but the registry's
// without it (`registry-only`: management, netdata.conf and mcp leave it out by default).
var (
	regDenied = accessConf{"registry-denied", "[registry]\n    allow from = localhost\n", "127.0.0.2"}
	regOnly   = accessConf{"registry-only", "    allow dashboard from = localhost\n" +
		"    allow badges from = localhost\n", "127.0.0.2"}
	regAccessConfs = []accessConf{accessACL, regDenied, regOnly, accessBearer}
)

// regAccessRows: hello asks for the dashboard right in the handler, every other action (a missing one too) for the
// registry right (api_v1_registry.c:125-141); the route itself checks nothing (web_api_v1.c:164-175: ACL and access
// NONE), so bearer protection does not refuse it (web_api.c:21-28, :82-89) and hello reports it (registry.c:195); a
// client with the registry right alone passes the web server's filter (web_client.c:1556-1566) but not a static
// file's (web_client.c:487-488).
var regAccessRows = func() []accessRow {
	hello := accessGet(regPath + "?action=hello")
	search := accessGet(regPath + "?action=search&for=m")
	off := [2]string{regOK, regEnd}
	disabled := `"action":"search",` + "\n" + `    "status":"disabled",`
	return []accessRow{
		{conf: accessACL.name, name: "registry-hello", request: hello, want: accessDenied},
		{conf: accessACL.name, name: "registry-search", request: search, want: off, holds: disabled},
		{conf: regDenied.name, name: "registry-hello", request: hello, want: off, holds: `"status":"ok"`},
		{conf: regDenied.name, name: "registry-search", request: search, want: accessDenied},
		{conf: regDenied.name, name: "registry-none", request: accessGet(regPath), want: accessDenied},
		{conf: regOnly.name, name: "registry-hello", request: hello, want: accessDenied},
		{conf: regOnly.name, name: "registry-search", request: search, want: off, holds: disabled},
		{conf: regOnly.name, name: "static", request: accessGet("/"), want: accessDenied},
		{conf: accessBearer.name, name: "registry-hello", request: hello, want: off, holds: `"bearer_protection":true`},
		{conf: accessBearer.name, name: "registry-search", request: search, want: off, holds: disabled},
	}
}()
