// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// cOnlyHostLabels are localhost labels only the oracle has: they come from C's daemon status file, not ported yet
// (decisions D49 point 3).
var cOnlyHostLabels = regexp.MustCompile(`^_hw_(product_id|product_name|sys_vendor|product_type)$`)

// infoIdentity splits an /api/v1/info answer into what must match byte for byte (version, uid, and the members from
// "alarms" up to "host_labels") and the host labels, compared as a map (C orders them by heap address).
func infoIdentity(t *testing.T, addr, path string, oracle bool) (string, map[string]any) {
	t.Helper()
	b, err := rawExchange(addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 10*time.Second)
	if err != nil {
		t.Fatalf("%s: %v", path, err)
	}
	body := httpBody(b)
	var doc struct {
		Version    string         `json:"version"`
		UID        string         `json:"uid"`
		HostLabels map[string]any `json:"host_labels"`
	}
	if err := json.Unmarshal(body, &doc); err != nil {
		t.Fatalf("%s: %v: %s", path, err, body)
	}
	from := bytes.Index(body, []byte(`"alarms":`))
	to := bytes.Index(body, []byte(`"host_labels":`))
	if from < 0 || to < from {
		t.Fatalf("%s: no alarms..host_labels members: %s", path, body)
	}
	if oracle {
		for k := range doc.HostLabels {
			if cOnlyHostLabels.MatchString(k) {
				delete(doc.HostLabels, k)
			}
		}
	}
	return doc.Version + " " + doc.UID + "\n" + string(body[from:to]), doc.HostLabels
}

// contextsHostLabels are the host labels /api/v1/contexts shows for localhost.
func contextsHostLabels(t *testing.T, addr string, oracle bool) map[string]any {
	t.Helper()
	b, err := rawExchange(addr, []byte("GET /api/v1/contexts?options=labels HTTP/1.1\r\n\r\n"), 10*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	var doc struct {
		HostLabels map[string]any `json:"host_labels"`
	}
	if err := json.Unmarshal(httpBody(b), &doc); err != nil {
		t.Fatalf("contexts: %v", err)
	}
	if oracle {
		for k := range doc.HostLabels {
			if cOnlyHostLabels.MatchString(k) {
				delete(doc.HostLabels, k)
			}
		}
	}
	return doc.HostLabels
}

// identityRecords are the main-thread records of the identity steps, in order, from a daemon's log.
func identityRecords(t *testing.T, d *daemon.Daemon) []string {
	t.Helper()
	msgRe := regexp.MustCompile(`msg="((?:[^"\\]|\\.)*)"`)
	var out []string
	for _, l := range logLines(t, d.Opts.RunDir, "daemon.log") {
		m := msgRe.FindStringSubmatch(l)
		if m == nil || threadOf(l) != "" {
			continue
		}
		for _, prefix := range []string{"SYSTEM INFO:", "RRDLABEL:", "Kubernetes pod label", "ACLK: proxy"} {
			if strings.HasPrefix(m[1], prefix) {
				out = append(out, m[1])
			}
		}
	}
	return out
}

func compareIdentity(t *testing.T, p *Pair, stage, path string) {
	t.Helper()
	var text [2]string
	var labels [2]map[string]any
	for i, side := range p.Each() {
		text[i], labels[i] = infoIdentity(t, side.Daemon.Addr, path, side.Role == Oracle)
	}
	if text[0] != text[1] {
		t.Errorf("%s %s: differs\n%s", stage, path, firstDifference([]byte(text[0]), []byte(text[1])))
	}
	if !reflect.DeepEqual(labels[0], labels[1]) {
		t.Errorf("%s %s: host labels differ\noracle:    %v\ncandidate: %v", stage, path, labels[0], labels[1])
	}
}

// TestLocalhostIdentity compares what both daemons say about localhost (system info and host labels in
// /api/v1/info and /api/v1/contexts) and about a connected child, before, while and after a child streams in (for
// `_is_parent`), with `[host labels]` that expand environment variables; then the same without any plugin scripts,
// with the records of the failures. Check `api.localhost-identity`.
func TestLocalhostIdentity(t *testing.T) {
	// a set variable, one the system-info script exports, and an unset one
	t.Setenv("SP2_TEST_VAR", "from-env")
	labels := "    sp2_label = hello\n    sp2_env = ${SP2_TEST_VAR:-fallback}\n    sp2_sys = ${NETDATA_SYSTEM_KERNEL_NAME}\n" +
		"    sp2_unset = ${SP2_UNSET_VAR}\n"
	p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, HostLabels: labels}, parentIdentity)
	compareIdentity(t, p, "standalone", "/api/v1/info")
	var contexts [2]map[string]any
	for i, side := range p.Each() {
		contexts[i] = contextsHostLabels(t, side.Daemon.Addr, side.Role == Oracle)
	}
	if !reflect.DeepEqual(contexts[0], contexts[1]) {
		t.Errorf("contexts host labels differ\noracle:    %v\ncandidate: %v", contexts[0], contexts[1])
	}
	if contexts[1]["_is_parent"] != "false" || contexts[1]["sp2_env"] != "from-env" || contexts[1]["sp2_sys"] != "Linux" {
		t.Errorf("candidate labels: %v", contexts[1])
	}
	var conns []*stream.Conn
	for _, side := range p.Each() {
		conn, err := stream.Connect(side.Daemon.Addr, side.Daemon.StreamKey, childHost, stream.CapsLive)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		conns = append(conns, conn)
	}
	time.Sleep(time.Second)
	compareIdentity(t, p, "child connected", "/api/v1/info")
	compareIdentity(t, p, "child connected", "/host/"+childHost.Hostname+"/api/v1/info")
	if got := contextsHostLabels(t, p.Candidate.Addr, false)["_is_parent"]; got != "true" {
		t.Errorf("candidate _is_parent with a child: %v", got)
	}
	for _, c := range conns {
		_ = c.Close()
	}
	time.Sleep(2 * time.Second)
	compareIdentity(t, p, "child disconnected", "/api/v1/info")
	var records [2][]string
	for i, side := range p.Each() {
		records[i] = identityRecords(t, side.Daemon)
	}
	if !reflect.DeepEqual(records[0], records[1]) {
		t.Errorf("identity records differ\noracle:    %q\ncandidate: %q", records[0], records[1])
	}
	unset := "RRDLABEL: environment variable 'SP2_UNSET_VAR' is not set and no default provided"
	if !slices.Contains(records[1], unset) {
		t.Errorf("candidate records lack %q: %q", unset, records[1])
	}

	t.Run("no-scripts", func(t *testing.T) {
		plugins := filepath.Join(t.TempDir(), "plugins.d")
		if err := os.Mkdir(plugins, 0o755); err != nil {
			t.Fatal(err)
		}
		p := StartPair(t, daemon.Options{StreamMemoryMode: "ram", StorageTiers: 1, PluginsDir: plugins}, parentIdentity)
		compareIdentity(t, p, "no scripts", "/api/v1/info")
		var records [2][]string
		for i, side := range p.Each() {
			records[i] = identityRecords(t, side.Daemon)
		}
		if !reflect.DeepEqual(records[0], records[1]) {
			t.Errorf("identity records differ\noracle:    %q\ncandidate: %q", records[0], records[1])
		}
		if len(records[1]) < 3 {
			t.Errorf("candidate failure records: %q", records[1])
		}
	})
}
