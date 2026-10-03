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
		"conf extra last": {
			func(o *Options) {
				o.HostLabels = "    a = b\n"
				o.ConfExtra = "[plugin:difftest]\n    update every = 1\n"
			},
			defaultNetdataConf + "\n[host labels]\n    a = b\n\n[plugin:difftest]\n    update every = 1\n",
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
