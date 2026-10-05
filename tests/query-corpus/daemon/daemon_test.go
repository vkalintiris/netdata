// SPDX-License-Identifier: GPL-3.0-or-later

package daemon

import (
	"errors"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

func TestNetdataConfigDisablesUnlistedPlugins(t *testing.T) {
	const disabled = "\n    enable running new plugins = no\n"
	if !strings.Contains(netdataConfTemplate, disabled) {
		t.Fatal("generated [plugins] configuration allows unlisted installed collectors")
	}
}

// defaultNetdataConf is renderNetdataConf's text for RunDir /r, port 1, one tier and hostname h.
const defaultNetdataConf = `[global]
    hostname = h

[directories]
    config = /r/etc
    cache = /r/cache
    lib = /r/lib
    log = /r/log
    home = /r/lib

[web]
    bind to = 127.0.0.1:1

[db]
    db = dbengine
    update every = 1
    storage tiers = 1
    replication period = 3650d
    replication step = 3650d
    dbengine tier 0 retention time = 0
    dbengine tier 1 retention time = 0
    dbengine tier 2 retention time = 0

[ml]
    enabled = no

[health]
    enabled = no

[registry]
    enabled = no

[plugins]
    enable running new plugins = no
    proc = no
    diskspace = no
    cgroups = no
    tc = no
    idlejitter = no
    statsd = no
    apps = no
    go.d = no
    charts.d = no
    python.d = no
    debugfs = no
    perf = no
    slabinfo = no
    ioping = no
    ebpf = no
    systemd-journal = no
    network-viewer = no
    timex = no
    profile = no
`

func TestNetdataConfRendering(t *testing.T) {
	base := Options{RunDir: "/r", Port: 1, StorageTiers: 1}
	cases := map[string]struct {
		edit func(*Options)
		want string
	}{
		"default": {func(*Options) {}, defaultNetdataConf},
		"plugins dir with run": {
			func(o *Options) { o.PluginsDir = `"/stock" "{run}/plugins.d"` },
			strings.Replace(defaultNetdataConf, "    home = /r/lib\n", "    home = /r/lib\n    plugins = \"/stock\" \"/r/plugins.d\"\n", 1),
		},
		"plugins extra after pulse": {
			func(o *Options) {
				o.PulseOff = true
				o.PluginsExtra = "    difftest = yes\n    check for new plugins every = 1\n"
			},
			defaultNetdataConf + "    netdata pulse = no\n    difftest = yes\n    check for new plugins every = 1\n",
		},
		"plugins stock": {
			func(o *Options) {
				o.PluginsStock = true
				o.PulseOff = true
				o.PluginsExtra = "    apps = yes\n"
			},
			defaultNetdataConf[:strings.Index(defaultNetdataConf, "    enable running new plugins")] +
				"    enable running new plugins = yes\n    proc = no\n    diskspace = no\n    cgroups = no\n    tc = no\n    idlejitter = no\n    statsd = no\n" +
				"    timex = no\n    profile = no\n    netdata pulse = no\n    apps = yes\n",
		},
		"update every, no home, stock dirs": {
			func(o *Options) {
				o.UpdateEvery = "2"
				o.NoHomeDir = true
				o.StockConfigDir = "{run}/sc"
				o.StockDataDir = "{run}/sd"
			},
			strings.Replace(strings.Replace(defaultNetdataConf, "    home = /r/lib\n",
				"    stock config = /r/sc\n    stock data = /r/sd\n", 1), "    update every = 1\n", "    update every = 2\n", 1),
		},
		"health on, the script under the run directory": {
			func(o *Options) {
				o.HealthOn = true
				o.HealthExtra = "    script to execute on alarm = {run}/notify/stub\n    run at least every = 1s\n"
			},
			strings.Replace(defaultNetdataConf, "[health]\n    enabled = no\n",
				"[health]\n    enabled = yes\n    script to execute on alarm = /r/notify/stub\n    run at least every = 1s\n", 1),
		},
		"health extra with run, health off": {
			func(o *Options) { o.HealthExtra = "    script to execute on alarm = {run}/x\n" },
			strings.Replace(defaultNetdataConf, "[health]\n    enabled = no\n",
				"[health]\n    enabled = no\n    script to execute on alarm = /r/x\n", 1),
		},
		"conf extra last": {
			func(o *Options) {
				o.HostLabels = "    a = b\n"
				o.ConfExtra = "[plugin:difftest]\n    update every = 1\n"
			},
			defaultNetdataConf + "\n[host labels]\n    a = b\n\n[plugin:difftest]\n    update every = 1\n",
		},
		"conf extra with run": {
			func(o *Options) {
				o.ConfExtra = "[registry]\n    netdata management api key file = {run}/lib/other.key\n"
			},
			defaultNetdataConf + "\n[registry]\n    netdata management api key file = /r/lib/other.key\n",
		},
	}
	for name, c := range cases {
		t.Run(name, func(t *testing.T) {
			o := base
			c.edit(&o)
			if got := renderNetdataConf(o, "h"); got != c.want {
				t.Errorf("got:\n%s\nwant:\n%s", got, c.want)
			}
		})
	}
}

// PluginsExtra may not set a key the template sets: every installed plugin stays off.
func TestPluginsExtraCannotEnableTemplatePlugins(t *testing.T) {
	got := map[string]bool{}
	for _, extra := range []string{"    proc = yes\n", "enable running new plugins = yes", "    netdata pulse = yes\n",
		"    difftest = yes\n    check for new plugins every = 1\n"} {
		got[extra] = validateOptions(Options{PluginsExtra: extra}) == nil
	}
	want := map[string]bool{"    proc = yes\n": false, "enable running new plugins = yes": false,
		"    netdata pulse = yes\n": false, "    difftest = yes\n    check for new plugins every = 1\n": true}
	if fmt.Sprint(got) != fmt.Sprint(want) {
		t.Errorf("accepted: %v, want %v", got, want)
	}
}

// Health's rails: HealthOn is the only switch, and it is refused without a notifier script. The netdata.conf an agent
// would read is what is checked, so no option's text can reopen [health] (C takes a later duplicate of the section and
// of a key). A rule's own `exec` may only name a path under the side's notifier directory or one nothing can run.
func TestHealthOnRails(t *testing.T) {
	const script = "    script to execute on alarm = {run}/notify/stub\n"
	const reopen = "[health]\n    enabled = yes\n"
	cases := map[string]struct {
		o    Options
		want string // a part of the error; empty: accepted
	}{
		"off, no extra":             {Options{}, ""},
		"off, other keys":           {Options{HealthExtra: "    run at least every = 1s\n    enabled alarms = a b\n"}, ""},
		"on with a script":          {Options{HealthOn: true, HealthExtra: script + "    run at least every = 1s\n"}, ""},
		"on without a script":       {Options{HealthOn: true, HealthExtra: "    run at least every = 1s\n"}, "HealthOn without"},
		"on, no extra":              {Options{HealthOn: true}, "HealthOn without"},
		"on, an empty script":       {Options{HealthOn: true, HealthExtra: "    script to execute on alarm =\n"}, "HealthOn without"},
		"enabled yes in the extra":  {Options{HealthExtra: "    enabled = yes\n" + script}, "HealthOn is the switch"},
		"enabled no in the extra":   {Options{HealthOn: true, HealthExtra: script + "enabled=no"}, "HealthOn is the switch"},
		"conf extra reopens health": {Options{ConfExtra: reopen}, "2 [health] sections"},
		// the other texts rendered after the template's [health]
		"logs extra reopens health":    {Options{LogsExtra: "    level = debug\n" + reopen}, "2 [health] sections"},
		"plugins extra reopens health": {Options{PluginsExtra: "    difftest = yes\n" + reopen}, "2 [health] sections"},
		"host labels reopen health":    {Options{HostLabels: "    a = b\n" + reopen}, "2 [health] sections"},
		"reopened with a script, on":   {Options{HealthOn: true, HealthExtra: script, LogsExtra: reopen + script}, "2 [health] sections"},
		// rendered before the template's [health]: its `enabled` is not health's
		"global extra sets enabled": {Options{GlobalExtra: "    enabled = yes\n"}, ""},
		// C does not trim a section's name: `[ health ]` is another section
		"another section's name": {Options{ConfExtra: "[ health ]\n    enabled = yes\n"}, ""},
	}
	for name, c := range cases {
		t.Run(name, func(t *testing.T) {
			c.o.RunDir, c.o.Port, c.o.StorageTiers = "/r", 1, 1
			err := validateOptions(c.o)
			if err == nil {
				err = validateHealthConf(renderNetdataConf(c.o, "h"), c.o.HealthOn)
			}
			switch {
			case c.want == "" && err != nil:
				t.Errorf("refused: %v", err)
			case c.want != "" && (err == nil || !strings.Contains(err.Error(), c.want)):
				t.Errorf("got %v, want an error naming %q", err, c.want)
			}
		})
	}

	// a rule's own notifier: only under the side's notifier directory, or where nothing can be executed
	rule := func(lines ...string) string {
		return " alarm: a\n    on: c.a\n  calc: $a\n" + strings.Join(lines, "\n") + "\n  warn: $this > 1\n"
	}
	for name, c := range map[string]struct {
		text string
		ok   bool
	}{
		"no exec":                          {rule(), true},
		"the stub":                         {rule("  exec: /r/notify/stub"), true},
		"a name that does not exist":       {rule("  exec: /r/notify/absent"), true},
		"under the null device":            {rule(`  exec: "/dev/null/x" 'arg'`), true},
		"the installed notifier":           {rule("  exec: /usr/libexec/netdata/plugins.d/alarm-notify.sh"), false},
		"another program":                  {rule("  exec: /bin/true"), false},
		"the key in upper case":            {rule("  EXEC : /bin/true"), false},
		"quoted":                           {rule(`  exec: "/bin/true"`), false},
		"a sibling of the directory":       {rule("  exec: /r/notify-other/stub"), false},
		"a relative name":                  {rule("  exec: notify/stub"), false},
		"continued on the next line":       {rule("  exec: \\", "/r/notify/stub"), false},
		"the key before a continued colon": {rule("  exec\\", "  : /bin/true"), false},
		"a comment ends a continued key":   {rule("  exec\\", "# a comment", "  : /bin/true"), true},
		"continued at the text's end":      {" alarm: a\n  exec\\\n  : /bin/true", false},
		"continued under the directory":    {rule("  exec\\", "  : /r/notify/stub"), true},
		"a key cut in two is another key":  {rule("  ex\\", "ec: /bin/true"), true},
		"the second rule's":                {rule("  exec: /r/notify/stub") + rule("  exec: /bin/true"), false},
		"in a comment":                     {rule("# exec: /bin/true"), true},
		"another key that holds it":        {rule("  info: exec: /bin/true"), true},
		"another key ending in it":         {rule("  noexec: /bin/true"), true},
	} {
		err := ValidateHealthRules(c.text, "/r/notify/", "/dev/null/")
		if (err == nil) != c.ok {
			t.Errorf("a rule's exec, %s: %v, want accepted: %v", name, err, c.ok)
		}
	}
	if err := ValidateHealthRules(rule("  exec: /bin/true"), ""); err == nil {
		t.Errorf("an empty prefix allows every exec")
	}
}

// A DynCfg payload's rail: a payload names its own notifier in `action.execute`, which no rail on a file or on
// netdata.conf sees. Only an empty one, or a file directly under the side's notifier directory, may be sent to an
// agent or laid out as a saved DynCfg file.
func TestHealthPayloadRails(t *testing.T) {
	rule := func(action string) string {
		return `{"format_version":1,"rules":[{"enabled":true,"type":"instance","config":{"match":{"on":"c.a"},"action":` + action + `}}]}`
	}
	for name, c := range map[string]struct {
		body string
		ok   bool
	}{
		"no action":                          {`{"format_version":1,"rules":[{"enabled":true,"type":"instance","config":{}}]}`, true},
		"no execute":                         {rule(`{"recipient":"root"}`), true},
		"an empty execute":                   {rule(`{"execute":""}`), true},
		"a null execute":                     {rule(`{"execute":null}`), true},
		"the stub":                           {rule(`{"execute":"/r/notify/stub"}`), true},
		"a name that does not exist":         {rule(`{"execute":"/r/notify/absent"}`), true},
		"a name with a dot and a dash":       {rule(`{"execute":"/r/notify/a-b_c.d"}`), true},
		"the installed notifier":             {rule(`{"execute":"/usr/libexec/netdata/plugins.d/alarm-notify.sh"}`), false},
		"another program":                    {rule(`{"execute":"/bin/true"}`), false},
		"a relative name":                    {rule(`{"execute":"notify/stub"}`), false},
		"a sibling of the directory":         {rule(`{"execute":"/r/notify-other/stub"}`), false},
		"the directory itself":               {rule(`{"execute":"/r/notify/"}`), false},
		"the directory without a slash":      {rule(`{"execute":"/r/notify"}`), false},
		"a subdirectory":                     {rule(`{"execute":"/r/notify/sub/stub"}`), false},
		"up and out":                         {rule(`{"execute":"/r/notify/../../bin/true"}`), false},
		"a hidden name":                      {rule(`{"execute":"/r/notify/.stub"}`), false},
		"dots alone":                         {rule(`{"execute":"/r/notify/.."}`), false},
		"an argument after the stub":         {rule(`{"execute":"/r/notify/stub x"}`), false},
		"a second command":                   {rule(`{"execute":"/r/notify/stub;/bin/true"}`), false},
		"a substitution":                     {rule(`{"execute":"/r/notify/$(true)"}`), false},
		"a quote":                            {rule(`{"execute":"/r/notify/stub'"}`), false},
		"a newline":                          {rule(`{"execute":"/r/notify/stub\n/bin/true"}`), false},
		"a space before":                     {rule(`{"execute":" /r/notify/stub"}`), false},
		"a number (C reads its text)":        {rule(`{"execute":5}`), false},
		"true (C reads its text)":            {rule(`{"execute":true}`), false},
		"an object":                          {rule(`{"execute":{"a":"/r/notify/stub"}}`), false},
		"an array":                           {rule(`{"execute":["/r/notify/stub"]}`), false},
		"the name written with an escape":    {rule(`{"\u0065xecute":"/bin/true"}`), false},
		"the value written with an escape":   {rule(`{"execute":"\/bin\/true"}`), false},
		"an escaped value under the dir":     {rule(`{"execute":"\/r\/notify\/stub"}`), true},
		"repeated, the first one bad":        {rule(`{"execute":"/bin/true","execute":"/r/notify/stub"}`), false},
		"repeated, the last one bad":         {rule(`{"execute":"/r/notify/stub","execute":"/bin/true"}`), false},
		"at the root":                        {`{"execute":"/bin/true","format_version":1,"rules":[]}`, false},
		"deep in a member no reader names":   {`{"format_version":1,"x":[[{"y":{"execute":"/bin/true"}}]]}`, false},
		"the second rule's":                  {`{"rules":[{"config":{"action":{"execute":""}}},{"config":{"action":{"execute":"/bin/true"}}}]}`, false},
		"as a value, not a name":             {`{"info":"execute","summary":"execute: /bin/true"}`, true},
		"another name that holds it":         {`{"executes":"/bin/true","noexecute":"/bin/true"}`, true},
		"the name in upper case":             {`{"EXECUTE":"/bin/true"}`, true},
		"no document: plain text":            {`not json`, true},
		"no document: cut short":             {`{"format_version":1,"rules":[`, true},
		"no document: empty":                 {``, true},
		"no document, but the name is there": {`{"action":{"execute":"/r/notify/stub"}`, false},
		"a comment, as json-c takes one":     {`{/* c */ "action":{"execute":"/bin/true"}}`, false},
		"single quotes, as json-c takes":     {`{'action':{'execute':'/bin/true'}}`, false},
		"a trailing comma, the name there":   {`{"action":{"execute":"/bin/true"},}`, false},
		"a trailing comma, no such name":     {`{"format_version":1,}`, true},
		"no document, a backslash":           {`{'\u0065xecute':'/bin/true'}`, false},
		"a second document after the first":  {`{"format_version":1}{"execute":"/bin/true"}`, false},
		"a second document, harmless":        {`{"format_version":1} {"a":1}`, true},
		"a NUL, then the name":               {"{\"format_version\":1}\x00{\"execute\":\"/bin/true\"}", false},
		"a scalar":                           {`5`, true},
		"an array of rules":                  {`[{"execute":"/bin/true"}]`, false},
	} {
		err := ValidateHealthPayload([]byte(c.body), "/r/notify/")
		if (err == nil) != c.ok {
			t.Errorf("a payload's execute, %s: %v, want accepted: %v", name, err, c.ok)
		}
	}
	// the directory with and without its slash; no directory, or an empty one, allows only an empty execute
	stub := []byte(rule(`{"execute":"/r/notify/stub"}`))
	if err := ValidateHealthPayload(stub, "/r/notify"); err != nil {
		t.Errorf("the directory without a slash: %v", err)
	}
	if err := ValidateHealthPayload(stub, "/other/notify/", "/r/notify/"); err != nil {
		t.Errorf("the second directory: %v", err)
	}
	if ValidateHealthPayload(stub) == nil || ValidateHealthPayload(stub, "") == nil || ValidateHealthPayload(stub, "/") == nil {
		t.Errorf("no directory, an empty one or the root allows an execute")
	}
	if err := ValidateHealthPayload([]byte(rule(`{"execute":""}`))); err != nil {
		t.Errorf("an empty execute without a directory: %v", err)
	}

	// a saved DynCfg file: its payload, what follows the first `---` line
	file := func(payload string) []byte {
		return []byte("version=1\nid=health:alert:prototype:a\nsource=x\ncmds=get \ncontent_type=application/json\ncontent_length=" +
			fmt.Sprint(len(payload)) + "\n---\n" + payload)
	}
	for name, c := range map[string]struct {
		text []byte
		ok   bool
	}{
		"no payload":                 {[]byte("version=1\nid=health:alert:prototype:a\nuser_disabled=true\ncmds=get \n"), true},
		"no payload, the name in it": {[]byte("version=1\nsource=execute=/bin/true\n"), true},
		"the stub":                   {file(rule(`{"execute":"/r/notify/stub"}`)), true},
		"another program":            {file(rule(`{"execute":"/bin/true"}`)), false},
		"a payload health refuses":   {file(`{"format_version":2,"rules":[]}`), true},
		"a payload cut short":        {file(`{"format_version":1,"rules":[{"config":{"action":{"execute":"/bin/true"`), false},
		"the separator first":        {[]byte("---\n" + rule(`{"execute":"/bin/true"}`)), false},
		"a second separator":         {file("{}\n---\n" + rule(`{"execute":"/bin/true"}`)), false},
	} {
		if err := ValidateDynCfgFile(c.text, "/r/notify/"); (err == nil) != c.ok {
			t.Errorf("a saved file, %s: %v, want accepted: %v", name, err, c.ok)
		}
	}
}

// In PluginsStock mode PluginsExtra may name installed plugins, never the internal collectors or the mode's own keys.
func TestPluginsStockMayNameInstalledPlugins(t *testing.T) {
	got := map[string]bool{}
	for _, extra := range []string{"    apps = no\n", "    slabinfo = yes\n", "    proc = yes\n", "    netdata pulse = yes\n",
		"enable running new plugins = no"} {
		got[extra] = validateOptions(Options{PluginsStock: true, PluginsExtra: extra}) == nil
	}
	want := map[string]bool{"    apps = no\n": true, "    slabinfo = yes\n": true, "    proc = yes\n": false,
		"    netdata pulse = yes\n": false, "enable running new plugins = no": false}
	if fmt.Sprint(got) != fmt.Sprint(want) {
		t.Errorf("accepted: %v, want %v", got, want)
	}
}

// Env's {run} is the run directory; entries without it pass as they are.
func TestEnvExpandsRun(t *testing.T) {
	got := expandEnv(Options{RunDir: "/r", Env: []string{"A={run}/otel", "B=x", "C={run}:{run}"}})
	want := []string{"A=/r/otel", "B=x", "C=/r:/r"}
	if fmt.Sprint(got) != fmt.Sprint(want) {
		t.Errorf("got %q, want %q", got, want)
	}
}

func TestStorageOptionsAreStrictAndRenderExactly(t *testing.T) {
	for _, tc := range []struct {
		name string
		o    Options
	}{
		{name: "default"},
		{name: "gorilla-dbengine", o: Options{DBEnginePageType: "gorilla", StreamMemoryMode: "dbengine"}},
		{name: "raw-ram", o: Options{DBEnginePageType: "raw", StreamMemoryMode: "ram"}},
		{name: "raw-alloc", o: Options{DBEnginePageType: "raw", StreamMemoryMode: "alloc"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if err := validateOptions(tc.o); err != nil {
				t.Fatalf("valid options rejected: %v", err)
			}
		})
	}

	for _, tc := range []struct {
		name string
		o    Options
	}{
		{name: "unknown page type", o: Options{DBEnginePageType: "array"}},
		{name: "unknown memory mode", o: Options{StreamMemoryMode: "heap"}},
		{name: "case variant", o: Options{DBEnginePageType: "Raw"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if err := validateOptions(tc.o); err == nil {
				t.Fatal("invalid storage option accepted")
			}
		})
	}

	if got := streamMemoryMode(Options{}); got != "dbengine" {
		t.Fatalf("default stream memory mode = %q, want dbengine", got)
	}
	if got := fmt.Sprintf(streamConfTemplate, "key", "alloc"); !strings.Contains(got, "[key]") || !strings.Contains(got, "default memory mode = alloc") {
		t.Fatalf("stream configuration did not render selected memory mode:\n%s", got)
	}
}

func TestQueryRetentionRejectsMalformedNumbers(t *testing.T) {
	doc := func(first, last any) map[string]any {
		return map[string]any{"db": map[string]any{
			"first_entry": first,
			"last_entry":  last,
		}}
	}

	maxInt64Float := math.Nextafter(float64(uint64(1)<<63), 0)
	valid := []struct {
		name        string
		first, last float64
		want        Retention
	}{
		{"ordinary", 10, 20, Retention{FirstEntry: 10, LastEntry: 20}},
		{"int64 bounds", -float64(uint64(1) << 63), maxInt64Float,
			Retention{FirstEntry: -1 << 63, LastEntry: int64(maxInt64Float)}},
	}
	for _, tc := range valid {
		t.Run(tc.name, func(t *testing.T) {
			got, ok := QueryRetention(doc(tc.first, tc.last))
			if !ok || got != tc.want {
				t.Fatalf("QueryRetention() = %+v/%v, want %+v/true", got, ok, tc.want)
			}
		})
	}

	invalid := []struct {
		name        string
		first, last any
	}{
		{"fractional first", 10.5, 20.0},
		{"fractional last", 10.0, 20.5},
		{"nan", math.NaN(), 20.0},
		{"positive infinity", 10.0, math.Inf(1)},
		{"negative infinity", math.Inf(-1), 20.0},
		{"above int64", 10.0, float64(uint64(1) << 63)},
		{"below int64", math.Nextafter(-float64(uint64(1)<<63), math.Inf(-1)), 20.0},
		{"wrong type", "10", 20.0},
	}
	for _, tc := range invalid {
		t.Run(tc.name, func(t *testing.T) {
			if got, ok := QueryRetention(doc(tc.first, tc.last)); ok {
				t.Fatalf("QueryRetention() accepted malformed numbers: %+v", got)
			}
		})
	}
}

func TestExpectedFirstEntryFollowsTheMemoryMode(t *testing.T) {
	ram := ringEntries("ram")
	// The fewest whole pages holding 3600 4-byte storage numbers.
	if page := int64(os.Getpagesize()); ram*4%page != 0 || ram < 3600 || ram*4-page >= 3600*4 {
		t.Fatalf("ringEntries(ram) = %d, want 3600 rounded up to whole %d-byte pages", ram, page)
	}
	cases := map[string]struct {
		mode              string
		first, last, ue   int64
		wantFirstReported int64
	}{
		"dbengine":           {"", 1000, 1990, 10, 1000},
		"ram":                {"ram", 1000, 1990, 10, 990},
		"alloc":              {"alloc", 1000, 1990, 10, 990},
		"ram wrapped":        {"ram", 1000, 1000 + 9999*10, 10, 1000 + 9999*10 - ram*10},
		"alloc wrapped":      {"alloc", 0, 4999, 1, 4999 - 3600},
		"alloc exactly full": {"alloc", 1, 3600, 1, 0},
	}
	for name, tc := range cases {
		t.Run(name, func(t *testing.T) {
			d := &Daemon{Opts: Options{StreamMemoryMode: tc.mode}}
			if got := d.ExpectedFirstEntry(tc.first, tc.last, tc.ue); got != tc.wantFirstReported {
				t.Fatalf("ExpectedFirstEntry(%d, %d, %d) = %d, want %d",
					tc.first, tc.last, tc.ue, got, tc.wantFirstReported)
			}
		})
	}
}

func TestInfoHasDaemonIdentity(t *testing.T) {
	valid := func() map[string]any {
		return map[string]any{
			"uid": "local-guid",
			"mirrored_hosts_status": []any{
				map[string]any{
					"hostname":  "query-corpus-a",
					"hops":      0.0,
					"reachable": true,
					"guid":      "local-guid",
				},
				map[string]any{
					"hostname":  "child",
					"hops":      1.0,
					"reachable": true,
					"guid":      "child-guid",
				},
			},
		}
	}

	if err := infoHasDaemonIdentity(valid(), "query-corpus-a"); err != nil {
		t.Fatalf("valid identity rejected: %v", err)
	}

	tests := map[string]func(map[string]any){
		"missing statuses": func(doc map[string]any) {
			delete(doc, "mirrored_hosts_status")
		},
		"wrong hostname": func(doc map[string]any) {
			status := doc["mirrored_hosts_status"].([]any)[0].(map[string]any)
			status["hostname"] = "query-corpus-b"
		},
		"child hop": func(doc map[string]any) {
			status := doc["mirrored_hosts_status"].([]any)[0].(map[string]any)
			status["hops"] = 1.0
		},
		"unreachable": func(doc map[string]any) {
			status := doc["mirrored_hosts_status"].([]any)[0].(map[string]any)
			status["reachable"] = false
		},
		"guid differs from uid": func(doc map[string]any) {
			status := doc["mirrored_hosts_status"].([]any)[0].(map[string]any)
			status["guid"] = "other-guid"
		},
		"duplicate local identity": func(doc map[string]any) {
			status := doc["mirrored_hosts_status"].([]any)[0].(map[string]any)
			doc["mirrored_hosts_status"] = append(
				doc["mirrored_hosts_status"].([]any),
				map[string]any{
					"hostname": status["hostname"], "hops": 0.0,
					"reachable": true, "guid": status["guid"],
				})
		},
	}
	for name, mutate := range tests {
		t.Run(name, func(t *testing.T) {
			doc := valid()
			mutate(doc)
			if err := infoHasDaemonIdentity(doc, "query-corpus-a"); err == nil {
				t.Fatal("malformed or wrong daemon identity accepted")
			}
		})
	}
}

func TestNewDaemonIdentityIsUnique(t *testing.T) {
	hostnameA, keyA, err := newDaemonIdentity()
	if err != nil {
		t.Fatal(err)
	}
	hostnameB, keyB, err := newDaemonIdentity()
	if err != nil {
		t.Fatal(err)
	}
	if hostnameA == hostnameB || keyA == keyB {
		t.Fatalf("identities repeated: %q/%q and %q/%q", hostnameA, keyA, hostnameB, keyB)
	}
	if !strings.HasPrefix(hostnameA, "query-corpus-") || hostnameA == "" || keyA == "" {
		t.Fatalf("invalid identity %q/%q", hostnameA, keyA)
	}
}

func TestResolveIdentity(t *testing.T) {
	fixed := &Identity{
		Hostname:    "parity-parent",
		StreamKey:   "11111111-2222-4333-8444-555555555555",
		MachineGUID: "66666666-7777-4888-8999-aaaaaaaaaaaa",
	}
	cases := map[string]struct {
		identity *Identity
		wantErr  bool
	}{
		"fixed identity is used and its GUID seeded": {identity: fixed},
		"incomplete fixed identity is rejected":      {identity: &Identity{Hostname: "x"}, wantErr: true},
		"no identity generates one":                  {},
	}
	for name, tc := range cases {
		t.Run(name, func(t *testing.T) {
			o := Options{RunDir: t.TempDir(), Identity: tc.identity}
			hostname, key, err := resolveIdentity(o)
			if tc.wantErr {
				if err == nil {
					t.Fatal("want an error")
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			guidPath := filepath.Join(o.RunDir, "lib", "registry", "netdata.public.unique.id")
			if tc.identity == nil {
				if !strings.HasPrefix(hostname, "query-corpus-") || key == "" {
					t.Fatalf("generated identity %q/%q", hostname, key)
				}
				if _, err := os.Stat(guidPath); !os.IsNotExist(err) {
					t.Fatalf("generated identity seeded a machine GUID: %v", err)
				}
				return
			}
			guid, err := os.ReadFile(guidPath)
			if err != nil {
				t.Fatal(err)
			}
			got := [3]string{hostname, key, string(guid)}
			want := [3]string{fixed.Hostname, fixed.StreamKey, fixed.MachineGUID}
			if got != want {
				t.Fatalf("identity %v, want %v", got, want)
			}
		})
	}
}

func TestStartWithPortRetriesOnlyAutomaticCollisions(t *testing.T) {
	collision := errors.New("bind collision")
	other := errors.New("configuration failure")

	t.Run("automatic collision retries", func(t *testing.T) {
		ports := []int{12001, 12002}
		picks, attempts := 0, 0
		got, err := startWithPortRetries(
			Options{},
			func(o Options) (*Daemon, error) {
				attempts++
				if attempts == 1 {
					return nil, collision
				}
				return &Daemon{Opts: o}, nil
			},
			func() (int, error) {
				port := ports[picks]
				picks++
				return port, nil
			},
			func(err error) bool { return errors.Is(err, collision) },
		)
		if err != nil {
			t.Fatal(err)
		}
		if attempts != 2 || picks != 2 || got.Opts.Port != 12002 {
			t.Fatalf("attempts=%d picks=%d port=%d, want 2/2/12002", attempts, picks, got.Opts.Port)
		}
	})

	t.Run("explicit port fails fast", func(t *testing.T) {
		attempts, picks := 0, 0
		_, err := startWithPortRetries(
			Options{Port: 12003},
			func(Options) (*Daemon, error) {
				attempts++
				return nil, collision
			},
			func() (int, error) {
				picks++
				return 0, nil
			},
			func(err error) bool { return errors.Is(err, collision) },
		)
		if !errors.Is(err, collision) || !errors.Is(err, ErrPortTaken) || attempts != 1 || picks != 0 {
			t.Fatalf("err=%v attempts=%d picks=%d, want a taken port's collision/1/0", err, attempts, picks)
		}
	})

	t.Run("explicit port, other failure", func(t *testing.T) {
		other := errors.New("no such binary")
		_, err := startWithPortRetries(
			Options{Port: 12004},
			func(Options) (*Daemon, error) { return nil, other },
			func() (int, error) { return 0, nil },
			func(err error) bool { return errors.Is(err, collision) },
		)
		if !errors.Is(err, other) || errors.Is(err, ErrPortTaken) {
			t.Fatalf("err=%v, want the failure, not a taken port", err)
		}
	})

	t.Run("non-collision does not retry", func(t *testing.T) {
		attempts := 0
		_, err := startWithPortRetries(
			Options{},
			func(Options) (*Daemon, error) {
				attempts++
				return nil, other
			},
			func() (int, error) { return 12004, nil },
			func(err error) bool { return errors.Is(err, collision) },
		)
		if !errors.Is(err, other) || attempts != 1 {
			t.Fatalf("err=%v attempts=%d, want configuration failure/1", err, attempts)
		}
	})

	t.Run("retry exhaustion is bounded", func(t *testing.T) {
		attempts := 0
		_, err := startWithPortRetries(
			Options{},
			func(Options) (*Daemon, error) {
				attempts++
				return nil, collision
			},
			func() (int, error) { return 12005 + attempts, nil },
			func(err error) bool { return errors.Is(err, collision) },
		)
		if err == nil || attempts != autoPortAttempts {
			t.Fatalf("err=%v attempts=%d, want error/%d", err, attempts, autoPortAttempts)
		}
	})
}

type fakeProcess struct {
	signalErr error
	killErr   error
	signals   []os.Signal
	kills     int
	onSignal  func()
	onKill    func()
}

func (p *fakeProcess) Signal(signal os.Signal) error {
	p.signals = append(p.signals, signal)
	if p.onSignal != nil {
		p.onSignal()
	}
	return p.signalErr
}

func (p *fakeProcess) Kill() error {
	p.kills++
	if p.onKill != nil {
		p.onKill()
	}
	return p.killErr
}

func testStoppingDaemon(process *fakeProcess, waitCh chan error) *Daemon {
	return &Daemon{
		process:     process,
		processPID:  4242,
		waitCh:      waitCh,
		termTimeout: time.Millisecond,
		killTimeout: time.Millisecond,
	}
}

func TestRestartReportsATakenPort(t *testing.T) {
	collision := "LISTENER: Cannot bind to ip '127.0.0.1', port 19999"
	tests := map[string]struct {
		earlier, stdout, log string
		taken                bool
	}{
		"in the daemon log":   {log: collision, taken: true},
		"on stdout":           {stdout: collision, taken: true},
		"an earlier launch's": {earlier: collision, log: "invalid configuration", taken: false},
		"another failure":     {log: "invalid configuration", taken: false},
	}
	for name, tc := range tests {
		t.Run(name, func(t *testing.T) {
			dir := t.TempDir()
			for _, sub := range []string{"log", "lib"} {
				if err := os.MkdirAll(filepath.Join(dir, sub), 0o755); err != nil {
					t.Fatal(err)
				}
			}
			if err := os.WriteFile(filepath.Join(dir, "log", "daemon.log"), []byte(tc.earlier+"\n"), 0o644); err != nil {
				t.Fatal(err)
			}
			status := filepath.Join(dir, "lib", "status-netdata.json")
			if err := os.WriteFile(status, []byte("{}"), 0o644); err != nil {
				t.Fatal(err)
			}
			// a daemon that reports its startup failure where the agents do and exits before it serves; its
			// run directory is the parent of the -c file's directory
			script := "#!/bin/sh\nrun=$(dirname \"$(dirname \"$3\")\")\necho '" + tc.stdout + "'\necho '" + tc.log +
				"' >> \"$run/log/daemon.log\"\nexit 1\n"
			binary := filepath.Join(dir, "netdata")
			if err := os.WriteFile(binary, []byte(script), 0o755); err != nil {
				t.Fatal(err)
			}
			d := &Daemon{Opts: Options{Binary: binary, RunDir: dir}, BaseURL: "http://127.0.0.1:1"}
			err := d.Restart()
			if err == nil || errors.Is(err, ErrPortTaken) != tc.taken {
				t.Fatalf("Restart() = %v, want a failure with a taken port %v", err, tc.taken)
			}
			// a taken port's attempt leaves no status for the next launch to report
			if _, serr := os.Stat(status); (serr == nil) == tc.taken {
				t.Fatalf("the status file after the attempt: %v, taken port %v", serr, tc.taken)
			}
		})
	}
}

func TestALongCommandPipeIsRefusedBeforeTheLaunch(t *testing.T) {
	dir := t.TempDir()
	long := filepath.Join(dir, strings.Repeat("p", maxPipePath-len(dir)-len("/")-len("/netdata.pipe")))
	for run, ok := range map[string]bool{long: true, long + "p": false} {
		d := &Daemon{Opts: Options{Binary: filepath.Join(dir, "missing"), RunDir: run}, BaseURL: "http://127.0.0.1:1"}
		err := d.Restart()
		if refused := err != nil && strings.Contains(err.Error(), "over a unix socket's 107"); refused == ok {
			t.Fatalf("pipe of %d bytes: Restart() = %v", len(run)+len("/netdata.pipe"), err)
		}
	}
}

func TestStopIsCheckedAndBounded(t *testing.T) {
	t.Run("term and reap", func(t *testing.T) {
		waitCh := make(chan error, 1)
		process := &fakeProcess{onSignal: func() { waitCh <- nil }}
		d := testStoppingDaemon(process, waitCh)

		if err := d.Stop(); err != nil {
			t.Fatal(err)
		}
		if len(process.signals) != 1 || process.signals[0] != syscall.SIGTERM || process.kills != 0 {
			t.Fatalf("signals=%v kills=%d", process.signals, process.kills)
		}
	})

	t.Run("graceful wait failure is reported", func(t *testing.T) {
		waitCh := make(chan error, 1)
		waitErr := errors.New("exit status 7")
		process := &fakeProcess{onSignal: func() { waitCh <- waitErr }}
		d := testStoppingDaemon(process, waitCh)

		if err := d.Stop(); !errors.Is(err, waitErr) {
			t.Fatalf("Stop() error = %v, want wait failure", err)
		}
	})

	t.Run("term timeout escalates and reaps", func(t *testing.T) {
		waitCh := make(chan error, 1)
		process := &fakeProcess{onKill: func() { waitCh <- errors.New("signal: killed") }}
		d := testStoppingDaemon(process, waitCh)

		if err := d.Stop(); err != nil {
			t.Fatal(err)
		}
		if process.kills != 1 {
			t.Fatalf("kills=%d, want 1", process.kills)
		}
	})

	t.Run("term failure is reported", func(t *testing.T) {
		waitCh := make(chan error, 1)
		termErr := errors.New("term failed")
		process := &fakeProcess{
			signalErr: termErr,
			onKill:    func() { waitCh <- errors.New("signal: killed") },
		}
		d := testStoppingDaemon(process, waitCh)

		if err := d.Stop(); !errors.Is(err, termErr) {
			t.Fatalf("Stop() error = %v, want TERM failure", err)
		}
	})

	t.Run("kill failure is reported", func(t *testing.T) {
		waitCh := make(chan error, 1)
		killErr := errors.New("kill failed")
		process := &fakeProcess{killErr: killErr}
		d := testStoppingDaemon(process, waitCh)

		if err := d.Stop(); !errors.Is(err, killErr) {
			t.Fatalf("Stop() error = %v, want KILL failure", err)
		}
	})

	t.Run("missing reap is bounded", func(t *testing.T) {
		waitCh := make(chan error)
		process := &fakeProcess{}
		d := testStoppingDaemon(process, waitCh)

		started := time.Now()
		err := d.Stop()
		if err == nil || !strings.Contains(err.Error(), "reap") || !strings.Contains(err.Error(), "PID 4242") {
			t.Fatalf("Stop() error = %v, want reap timeout identifying PID 4242", err)
		}
		if elapsed := time.Since(started); elapsed > 100*time.Millisecond {
			t.Fatalf("Stop() took %v, want bounded completion", elapsed)
		}
	})

	t.Run("already-finished signal is not an error after reap", func(t *testing.T) {
		waitCh := make(chan error, 1)
		waitCh <- nil
		process := &fakeProcess{signalErr: os.ErrProcessDone}
		d := testStoppingDaemon(process, waitCh)

		if err := d.Stop(); err != nil {
			t.Fatalf("Stop() error = %v", err)
		}
	})
}

// stream.conf as the options render it: an empty StreamSection changes nothing, a StreamSection goes under the
// disabled [stream] header, before the key's section, and StreamTo's child section excludes it.
func TestStreamConfRendering(t *testing.T) {
	const key = "5a1e0000-0000-4000-8000-0000000000aa"
	keySection := "\n[" + key + "]\n    enabled = yes\n    type = api\n    default memory mode = dbengine\n" +
		"    health enabled by default = no\n    replication period = 3650d\n"
	to := &StreamTo{Destination: "127.0.0.1:1", APIKey: key, Extra: "    reconnect delay = 5\n"}
	for name, tc := range map[string]struct {
		o    Options
		want string
	}{
		"parent": {o: Options{}, want: "[stream]\n    enabled = no\n" + keySection},
		"no key": {o: Options{NoStreamKey: true}, want: "[stream]\n    enabled = no\n"},
		"child": {
			o: Options{NoStreamKey: true, StreamTo: to},
			want: "[stream]\n    enabled = yes\n    destination = 127.0.0.1:1\n    api key = " + key +
				"\n    enable compression = no\n    reconnect delay = 5\n",
		},
		"extra": {
			o:    Options{NoStreamKey: true, StreamExtra: "[x]\n    a = b\n"},
			want: "[stream]\n    enabled = no\n[x]\n    a = b\n",
		},
		"section": {
			o:    Options{StreamSection: "    reconnect delay = 5\n"},
			want: "[stream]\n    enabled = no\n    reconnect delay = 5\n" + keySection,
		},
		"section without a key": {
			o:    Options{NoStreamKey: true, StreamSection: "    destination = h\n"},
			want: "[stream]\n    enabled = no\n    destination = h\n",
		},
	} {
		if got := renderStreamConf(tc.o, key); got != tc.want {
			t.Errorf("%s:\n%q\nwant\n%q", name, got, tc.want)
		}
	}
	if err := validateOptions(Options{StreamSection: "x", StreamTo: to}); err == nil {
		t.Error("StreamSection with StreamTo is accepted")
	}
}

// The rail on stream.conf (validateStreamHealth; M9 commit 9, D214 F7): a line that may turn a child's health on is
// taken only when netdata.conf's [health] section names the run directory's recording stub as the notifier, with the
// parent's health on or off; `health enabled = no` (and its older name) needs nothing.
func TestStreamHealthRails(t *testing.T) {
	const (
		guid   = "\n[5a1e0000-0000-4000-8000-0000000000c1]\n    type = machine\n"
		stub   = "    script to execute on alarm = {run}/notify/stub\n"
		refuse = "may turn a child's health on"
	)
	// the installed notifier's file name is put together here: no command line of a run may hold it
	installed := "    script to execute on alarm = /usr/libexec/netdata/plugins.d/alarm-" + "notify.sh\n"
	child := func(line string) string { return guid + "    " + line + "\n" }
	key := "5a1e0000-0000-4000-8000-0000000000aa"
	for name, c := range map[string]struct {
		o    Options
		want string // a part of the error; empty: accepted
	}{
		"the template alone":             {Options{}, ""},
		"no stream key":                  {Options{NoStreamKey: true}, ""},
		"a child's section without it":   {Options{StreamExtra: guid + "    postpone alerts on connect = 0\n"}, ""},
		"no, health off":                 {Options{StreamExtra: child("health enabled = no")}, ""},
		"no, health on":                  {Options{HealthOn: true, HealthExtra: stub, StreamExtra: child("health enabled = no")}, ""},
		"no, without spaces":             {Options{StreamExtra: child("health enabled=no")}, ""},
		"no, the older name":             {Options{StreamExtra: child("health enabled by default = no")}, ""},
		"a comment":                      {Options{StreamExtra: child("# health enabled = yes")}, ""},
		"another key":                    {Options{StreamExtra: child("health log retention = 1d")}, ""},
		"yes, no notifier":               {Options{StreamExtra: child("health enabled = yes")}, refuse},
		"yes, health off, the stub":      {Options{HealthExtra: stub, StreamExtra: child("health enabled = yes")}, ""},
		"yes, health on, the stub":       {Options{HealthOn: true, HealthExtra: stub, StreamExtra: child("health enabled = yes")}, ""},
		"auto, no notifier":              {Options{StreamExtra: child("health enabled = auto")}, refuse},
		"auto, the stub":                 {Options{HealthExtra: stub, StreamExtra: child("health enabled = auto")}, ""},
		"on demand":                      {Options{StreamExtra: child("health enabled = on demand")}, refuse},
		"true":                           {Options{StreamExtra: child("health enabled = true")}, refuse},
		"on":                             {Options{StreamExtra: child("health enabled = on")}, refuse},
		"a word C does not know":         {Options{StreamExtra: child("health enabled = maybe")}, refuse},
		"false is not the word":          {Options{StreamExtra: child("health enabled = false")}, refuse},
		"NO is not the word either":      {Options{StreamExtra: child("health enabled = NO")}, refuse},
		"the key in upper case, no":      {Options{StreamExtra: child("Health Enabled = no")}, ""},
		"an empty value":                 {Options{StreamExtra: child("health enabled =")}, refuse},
		"no `=`":                         {Options{StreamExtra: child("health enabled yes")}, refuse},
		"no, then more":                  {Options{StreamExtra: child("health enabled = no yes")}, refuse},
		"the older name, yes":            {Options{StreamExtra: child("health enabled by default = yes")}, refuse},
		"the key in upper case":          {Options{StreamExtra: child("Health Enabled = yes")}, refuse},
		"the key with two spaces":        {Options{StreamExtra: child("health  enabled = yes")}, refuse},
		"a tab before the key":           {Options{StreamExtra: guid + "\thealth enabled = yes\n"}, refuse},
		"a key that holds it":            {Options{StreamExtra: child("my health enabled = yes")}, refuse},
		"the second of two lines":        {Options{StreamExtra: child("health enabled = no") + "    health enabled = yes\n"}, refuse},
		"in the [stream] section":        {Options{StreamSection: "    health enabled = yes\n"}, refuse},
		"in the [stream] section, stub":  {Options{HealthExtra: stub, StreamSection: "    health enabled = yes\n"}, ""},
		"in a child's own [stream]":      {Options{NoStreamKey: true, StreamTo: &StreamTo{Destination: "h:1", APIKey: key, Extra: "    health enabled = yes\n"}}, refuse},
		"yes, another program":           {Options{HealthExtra: "    script to execute on alarm = /bin/true\n", StreamExtra: child("health enabled = yes")}, refuse},
		"yes, the installed notifier":    {Options{HealthExtra: installed, StreamExtra: child("health enabled = yes")}, refuse},
		"yes, an empty script":           {Options{HealthExtra: "    script to execute on alarm =\n", StreamExtra: child("health enabled = yes")}, refuse},
		"yes, a sibling of the stub":     {Options{HealthExtra: "    script to execute on alarm = {run}/notify/stub2\n", StreamExtra: child("health enabled = yes")}, refuse},
		"yes, another directory's stub":  {Options{HealthExtra: "    script to execute on alarm = /other/notify/stub\n", StreamExtra: child("health enabled = yes")}, refuse},
		"yes, the stub then another":     {Options{HealthExtra: stub + "    script to execute on alarm = /bin/true\n", StreamExtra: child("health enabled = yes")}, refuse},
		"yes, another then the stub":     {Options{HealthExtra: "    script to execute on alarm = /bin/true\n" + stub, StreamExtra: child("health enabled = yes")}, ""},
		"yes, the stub in a comment":     {Options{HealthExtra: "#" + stub, StreamExtra: child("health enabled = yes")}, refuse},
		"yes, the stub in [web]":         {Options{WebExtra: stub, StreamExtra: child("health enabled = yes")}, refuse},
		"yes, health on, another script": {Options{HealthOn: true, HealthExtra: "    script to execute on alarm = /bin/true\n", StreamExtra: child("health enabled = yes")}, refuse},
	} {
		t.Run(name, func(t *testing.T) {
			c.o.RunDir, c.o.Port, c.o.StorageTiers = "/r", 1, 1
			err := validateOptions(c.o)
			switch {
			case c.want == "" && err != nil:
				t.Errorf("refused: %v", err)
			case c.want != "" && (err == nil || !strings.Contains(err.Error(), c.want)):
				t.Errorf("got %v, want an error naming %q", err, c.want)
			}
		})
	}
	// without a run directory there is no stub to name
	if err := validateOptions(Options{HealthExtra: "    script to execute on alarm = notify/stub\n", StreamExtra: child("health enabled = yes")}); err == nil {
		t.Error("a child's health on is accepted without a run directory")
	}
	// the texts judged are the ones a start writes
	o := Options{RunDir: "/r", HealthExtra: stub, StreamExtra: child("health enabled = yes")}
	if err := validateStreamHealth(renderStreamConf(o, key), renderNetdataConf(o, "h"), "/r"); err != nil {
		t.Errorf("the rendered texts: %v", err)
	}
	if err := validateStreamHealth(renderStreamConf(o, key), renderNetdataConf(o, "h"), "/other"); err == nil {
		t.Error("the stub of another run directory is accepted")
	}
}
