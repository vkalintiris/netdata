// SPDX-License-Identifier: GPL-3.0-or-later

// Package daemon boots a completely stock netdata binary with a generated
// test configuration (dbengine under a scratch run dir, all collectors and
// subsystems off) and provides the corpus driver primitives: HTTP queries,
// the retention settle barrier, restart.
package daemon

import (
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"
)

const (
	autoPortAttempts = 5
	defaultTermWait  = 30 * time.Second
	defaultKillWait  = 5 * time.Second
)

var errProcessNotReaped = errors.New("daemon: failed startup process is not reaped")

// Options configures the daemon under test.
type Options struct {
	Binary           string // path to the stock netdata binary
	RunDir           string // scratch directory for etc/cache/lib/log
	Port             int    // 0 picks a free port
	StorageTiers     int    // defaults to 3
	DBEnginePageType string // empty keeps the stock Gorilla default; also accepts gorilla or raw
	StreamMemoryMode string // empty defaults to dbengine; also accepts ram or alloc
	// TierRetentionMB caps a tier's dbengine disk quota (index = tier), so
	// its oldest datafiles rotate out while the tiers above keep more
	// history — the plan-switching scenario. Retention TIME knobs are
	// unusable at the fixed 2023 fixture epoch (wall-clock enforced), so
	// rotation has to be driven by VOLUME. The engine floors any quota at
	// RRDENG_MIN_DISK_SPACE_MB (25MiB).
	TierRetentionMB [3]int
	// TierGrouping sets "dbengine tier N update every iterations" (index
	// 1..2, default 60 each). Lowering it brings the tiers closer together
	// in points-per-second, which is what makes a tier ABOVE tier0 fill its
	// own quota with a fixture of practical size: at the default 60, tier1
	// would need ~60x more data than tier0 to rotate at all.
	TierGrouping [3]int
	// ReplicationStepSeconds, when non-zero, bounds the parent's per-request
	// replication window, so a streaming fixture can generate rows per
	// request instead of materializing millions of points.
	ReplicationStepSeconds int
	// Identity, when set, replaces the generated hostname and stream key and
	// pre-seeds the machine GUID, so two daemons (e.g. two implementations
	// compared by tests/parity) boot with the same identity.
	Identity *Identity
	// WebDir, when set, is the [directories] web path, so two daemons built
	// with different install prefixes serve the same static files.
	WebDir string
	// WebExtra is appended to the [web] section verbatim (one "key = value" per line).
	WebExtra string
	// GlobalExtra is appended to the [global] section verbatim (one "key = value" per line).
	GlobalExtra string
	// LogsExtra, when set, is written as a [logs] section at the end of netdata.conf (one "key = value" per line).
	LogsExtra string
	// HostLabels, when set, is written as a [host labels] section at the end of netdata.conf (one "key = value"
	// per line).
	HostLabels string
	// PluginsDir, when set, is the [directories] plugins path (e.g. an empty directory: no system-info.sh).
	PluginsDir string
	// StreamExtra is appended to stream.conf verbatim (e.g. per-child [<machine guid>] sections of a parent).
	StreamExtra string
	// StreamTo, when set, makes the daemon a streaming child of that destination.
	StreamTo *StreamTo
	// BindTo, when set, is the [web] bind to value, with {port} replaced by Port and {run} by RunDir. It must keep
	// a listener on 127.0.0.1:{port}, which the readiness probe uses. Empty is 127.0.0.1:{port}.
	BindTo string
	// PipeName, when set, is the daemon's NETDATA_PIPENAME with {run} replaced by RunDir. Empty is
	// {run}/netdata.pipe, so daemons never share the default command pipe.
	PipeName string
	// DBMode is the [db] db value; empty is dbengine.
	DBMode string
	// SeedCache, when set, is a directory copied into the cache directory before the first start (writable, as the
	// daemon's own would be): the databases and dbengine files of an earlier run.
	SeedCache string
	// PulseOff sets `[plugins] netdata pulse = no`: the C agent's own charts stop writing into its database.
	PulseOff bool
}

// StreamTo is a child's [stream] section.
type StreamTo struct {
	Destination string // host:port
	APIKey      string
	Compression bool // enable compression (the parent picks the algorithm)
}

// Identity is a fixed daemon identity. StreamKey and MachineGUID must be UUIDs.
type Identity struct {
	Hostname    string
	StreamKey   string
	MachineGUID string
}

// Daemon is one running netdata under test.
type Daemon struct {
	Opts      Options
	BaseURL   string
	Addr      string // host:port for streaming connections
	Hostname  string
	StreamKey string
	PipeName  string // the command pipe (NETDATA_PIPENAME) netdatacli reaches the daemon on

	LaunchStartedAt time.Time
	process         daemonProcess
	processPID      int
	waitCh          chan error

	// Tests shorten these bounds; zero selects the production defaults.
	termTimeout time.Duration
	killTimeout time.Duration
}

type daemonProcess interface {
	Signal(os.Signal) error
	Kill() error
}

const netdataConfTemplate = `[global]
    hostname = %[2]s
%[9]s
[directories]
    config = %[1]s/etc
    cache = %[1]s/cache
    lib = %[1]s/lib
    log = %[1]s/log
    home = %[1]s/lib
%[7]s
[web]
    bind to = %[10]s
%[8]s
[db]
    db = %[11]s
    update every = 1
    storage tiers = %[4]d
    replication period = 3650d
    replication step = %[5]s
    dbengine tier 0 retention time = 0
    dbengine tier 1 retention time = 0
    dbengine tier 2 retention time = 0
%[6]s
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
%[12]s`

const streamConfTemplate = `[stream]
    enabled = no

[%[1]s]
    enabled = yes
    type = api
    default memory mode = %[2]s
    health enabled by default = no
    replication period = 3650d
`

func validateOptions(o Options) error {
	switch o.DBEnginePageType {
	case "", "gorilla", "raw":
	default:
		return fmt.Errorf("daemon: invalid dbengine page type %q", o.DBEnginePageType)
	}

	switch o.StreamMemoryMode {
	case "", "dbengine", "ram", "alloc":
	default:
		return fmt.Errorf("daemon: invalid stream memory mode %q", o.StreamMemoryMode)
	}

	return nil
}

func streamMemoryMode(o Options) string {
	if o.StreamMemoryMode == "" {
		return "dbengine"
	}
	return o.StreamMemoryMode
}

// freePort asks the kernel for an unused localhost TCP port.
func freePort() (int, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return 0, err
	}
	port := l.Addr().(*net.TCPAddr).Port
	return port, l.Close()
}

func newDaemonIdentity() (hostname, streamKey string, err error) {
	var raw [16]byte
	if _, err := rand.Read(raw[:]); err != nil {
		return "", "", fmt.Errorf("daemon: generate identity: %w", err)
	}
	raw[6] = raw[6]&0x0f | 0x40
	raw[8] = raw[8]&0x3f | 0x80

	hostname = fmt.Sprintf("query-corpus-%x", raw[:8])
	streamKey = fmt.Sprintf(
		"%x-%x-%x-%x-%x",
		raw[0:4], raw[4:6], raw[6:8], raw[8:10], raw[10:16])
	return hostname, streamKey, nil
}

// resolveIdentity returns the fixed identity when Options carries one, writing
// its machine GUID where the daemon reads it first (lib/registry), and a fresh
// random identity otherwise.
func resolveIdentity(o Options) (hostname, streamKey string, err error) {
	if o.Identity == nil {
		return newDaemonIdentity()
	}
	id := o.Identity
	if id.Hostname == "" || id.StreamKey == "" || id.MachineGUID == "" {
		return "", "", fmt.Errorf("daemon: fixed identity needs hostname, stream key and machine GUID")
	}
	registry := filepath.Join(o.RunDir, "lib", "registry")
	if err := os.MkdirAll(registry, 0o755); err != nil {
		return "", "", fmt.Errorf("daemon: registry dir: %w", err)
	}
	guidPath := filepath.Join(registry, "netdata.public.unique.id")
	if err := os.WriteFile(guidPath, []byte(id.MachineGUID), 0o644); err != nil {
		return "", "", fmt.Errorf("daemon: write machine GUID: %w", err)
	}
	return id.Hostname, id.StreamKey, nil
}

// Start writes the test configuration under RunDir, boots the daemon in the
// foreground and waits until the HTTP API answers.
func Start(o Options) (*Daemon, error) {
	if o.StorageTiers <= 0 {
		o.StorageTiers = 3
	}
	if err := validateOptions(o); err != nil {
		return nil, err
	}

	for _, sub := range []string{"etc", "cache", "lib", "log"} {
		if err := os.MkdirAll(filepath.Join(o.RunDir, sub), 0o755); err != nil {
			return nil, fmt.Errorf("daemon: run dir: %w", err)
		}
	}

	if o.SeedCache != "" {
		cache := filepath.Join(o.RunDir, "cache")
		err := filepath.WalkDir(o.SeedCache, func(path string, e os.DirEntry, err error) error {
			if err != nil {
				return err
			}
			rel, _ := filepath.Rel(o.SeedCache, path)
			if e.IsDir() {
				return os.MkdirAll(filepath.Join(cache, rel), 0o755)
			}
			if !e.Type().IsRegular() {
				return nil
			}
			b, err := os.ReadFile(path)
			if err != nil {
				return err
			}
			return os.WriteFile(filepath.Join(cache, rel), b, 0o644)
		})
		if err != nil {
			return nil, fmt.Errorf("daemon: seed cache: %w", err)
		}
	}

	hostname, streamKey, err := resolveIdentity(o)
	if err != nil {
		return nil, err
	}
	streamConf := fmt.Sprintf(streamConfTemplate, streamKey, streamMemoryMode(o))
	if o.StreamTo != nil {
		streamConf = strings.Replace(streamConf, "[stream]\n    enabled = no\n", fmt.Sprintf(
			"[stream]\n    enabled = yes\n    destination = %s\n    api key = %s\n    enable compression = %s\n",
			o.StreamTo.Destination, o.StreamTo.APIKey, yesNo(o.StreamTo.Compression)), 1)
	}
	streamConf += o.StreamExtra
	if err := os.WriteFile(filepath.Join(o.RunDir, "etc", "stream.conf"), []byte(streamConf), 0o644); err != nil {
		return nil, fmt.Errorf("daemon: write stream.conf: %w", err)
	}
	// Opt out of anonymous statistics before first boot.
	if err := os.WriteFile(filepath.Join(o.RunDir, "etc", ".opt-out-from-anonymous-statistics"), nil, 0o644); err != nil {
		return nil, fmt.Errorf("daemon: write opt-out: %w", err)
	}

	return startWithPortRetries(
		o,
		func(attempt Options) (*Daemon, error) {
			return startAttempt(attempt, hostname, streamKey)
		},
		freePort,
		func(err error) bool {
			return !errors.Is(err, errProcessNotReaped) &&
				startupLogShowsBindCollision(o.RunDir)
		},
	)
}

func startWithPortRetries(
	o Options,
	start func(Options) (*Daemon, error),
	pickPort func() (int, error),
	retryable func(error) bool,
) (*Daemon, error) {
	if o.Port != 0 {
		return start(o)
	}

	var lastErr error
	for attempt := 1; attempt <= autoPortAttempts; attempt++ {
		port, err := pickPort()
		if err != nil {
			return nil, fmt.Errorf("daemon: select free port: %w", err)
		}
		candidate := o
		candidate.Port = port
		d, err := start(candidate)
		if err == nil {
			return d, nil
		}
		lastErr = err
		if !retryable(err) {
			return nil, err
		}
	}
	return nil, fmt.Errorf(
		"daemon: automatic port selection exhausted after %d bind collisions: %w",
		autoPortAttempts, lastErr)
}

func startAttempt(o Options, hostname, streamKey string) (*Daemon, error) {
	step := "3650d"
	if o.ReplicationStepSeconds > 0 {
		step = fmt.Sprintf("%ds", o.ReplicationStepSeconds)
	}
	extraDB := ""
	for tier, mb := range o.TierRetentionMB {
		if mb > 0 {
			extraDB += fmt.Sprintf("    dbengine tier %d retention size = %dMiB\n", tier, mb)
		}
	}
	for tier, every := range o.TierGrouping {
		if tier > 0 && every > 0 {
			extraDB += fmt.Sprintf("    dbengine tier %d update every iterations = %d\n", tier, every)
		}
	}
	if o.DBEnginePageType != "" {
		extraDB += fmt.Sprintf("    dbengine page type = %s\n", o.DBEnginePageType)
	}
	extraDirs := ""
	if o.WebDir != "" {
		extraDirs = fmt.Sprintf("    web = %s\n", o.WebDir)
	}
	if o.PluginsDir != "" {
		extraDirs += fmt.Sprintf("    plugins = %s\n", o.PluginsDir)
	}
	bindTo := o.BindTo
	if bindTo == "" {
		bindTo = "127.0.0.1:{port}"
	}
	bindTo = strings.NewReplacer("{port}", strconv.Itoa(o.Port), "{run}", o.RunDir).Replace(bindTo)
	dbMode := o.DBMode
	if dbMode == "" {
		dbMode = "dbengine"
	}
	pulse := ""
	if o.PulseOff {
		pulse = "    netdata pulse = no\n"
	}
	conf := fmt.Sprintf(netdataConfTemplate, o.RunDir, hostname, o.Port, o.StorageTiers, step, extraDB, extraDirs, o.WebExtra,
		o.GlobalExtra, bindTo, dbMode, pulse)
	if o.LogsExtra != "" {
		conf += "\n[logs]\n" + o.LogsExtra
	}
	if o.HostLabels != "" {
		conf += "\n[host labels]\n" + o.HostLabels
	}
	confPath := filepath.Join(o.RunDir, "etc", "netdata.conf")
	if err := os.WriteFile(confPath, []byte(conf), 0o644); err != nil {
		return nil, fmt.Errorf("daemon: write netdata.conf: %w", err)
	}

	d := &Daemon{
		Opts:      o,
		BaseURL:   fmt.Sprintf("http://127.0.0.1:%d", o.Port),
		Addr:      fmt.Sprintf("127.0.0.1:%d", o.Port),
		Hostname:  hostname,
		StreamKey: streamKey,
	}
	if err := d.launch(); err != nil {
		if d.process != nil {
			err = errors.Join(err, errProcessNotReaped)
		}
		return nil, err
	}
	return d, nil
}

// PID is the daemon's process ID, 0 once it has exited.
func (d *Daemon) PID() int { return d.processPID }

func (d *Daemon) launch() error {
	confPath := filepath.Join(d.Opts.RunDir, "etc", "netdata.conf")
	cmd := exec.Command(d.Opts.Binary, "-D", "-c", confPath)
	d.PipeName = filepath.Join(d.Opts.RunDir, "netdata.pipe")
	if d.Opts.PipeName != "" {
		d.PipeName = strings.ReplaceAll(d.Opts.PipeName, "{run}", d.Opts.RunDir)
	}
	cmd.Env = append(os.Environ(), "NETDATA_PIPENAME="+d.PipeName)
	stdout, err := os.Create(filepath.Join(d.Opts.RunDir, "log", "stdout.log"))
	if err != nil {
		return fmt.Errorf("daemon: stdout log: %w", err)
	}
	cmd.Stdout = stdout
	cmd.Stderr = stdout
	d.LaunchStartedAt = time.Now()
	if err := cmd.Start(); err != nil {
		stdout.Close()
		return fmt.Errorf("daemon: start %s: %w", d.Opts.Binary, err)
	}
	d.process = cmd.Process
	d.processPID = cmd.Process.Pid
	d.waitCh = make(chan error, 1)
	go func() {
		d.waitCh <- cmd.Wait()
		stdout.Close()
	}()

	// wait for the HTTP API; the probe needs its own timeout so a daemon
	// that accepts TCP but never answers cannot hang the readiness loop
	client := &http.Client{Timeout: 5 * time.Second}
	deadline := time.Now().Add(60 * time.Second)
	var lastProbeErr error
	for {
		info, err := getJSONWithClient(client, d.BaseURL+"/api/v1/info")
		if err == nil {
			if err := infoHasDaemonIdentity(info, d.Hostname); err == nil {
				return nil
			} else {
				lastProbeErr = err
			}
		} else {
			lastProbeErr = err
		}
		select {
		case werr := <-d.waitCh:
			// the process is already reaped; make a later Stop() a no-op
			// instead of blocking forever on the drained wait channel
			d.process = nil
			d.processPID = 0
			return fmt.Errorf(
				"daemon: exited during startup: %v; last readiness probe: %v (see %s/log/stdout.log)",
				werr, lastProbeErr, d.Opts.RunDir)
		default:
		}
		if time.Now().After(deadline) {
			stopErr := d.Stop()
			return errors.Join(
				fmt.Errorf(
					"daemon: correct HTTP API identity not ready after 60s: %v (see %s/log/stdout.log)",
					lastProbeErr, d.Opts.RunDir),
				stopErr)
		}
		time.Sleep(200 * time.Millisecond)
	}
}

func infoHasDaemonIdentity(doc map[string]any, hostname string) error {
	uid, ok := doc["uid"].(string)
	if !ok || uid == "" {
		return fmt.Errorf("daemon: /api/v1/info has no local uid")
	}
	statuses, ok := doc["mirrored_hosts_status"].([]any)
	if !ok {
		return fmt.Errorf("daemon: /api/v1/info has no mirrored_hosts_status array")
	}

	matches := 0
	for i, statusAny := range statuses {
		status, ok := statusAny.(map[string]any)
		if !ok {
			return fmt.Errorf("daemon: mirrored_hosts_status[%d] is not an object", i)
		}
		gotHostname, hostOK := status["hostname"].(string)
		hops, hopsOK := status["hops"].(float64)
		reachable, reachableOK := status["reachable"].(bool)
		guid, guidOK := status["guid"].(string)
		if !hostOK || !hopsOK || math.IsNaN(hops) || math.IsInf(hops, 0) ||
			math.Trunc(hops) != hops || !reachableOK || !guidOK {
			return fmt.Errorf("daemon: malformed mirrored_hosts_status[%d]: %v", i, status)
		}
		if gotHostname == hostname && hops == 0 && reachable && guid == uid {
			matches++
		}
	}
	if matches != 1 {
		return fmt.Errorf(
			"daemon: /api/v1/info has %d reachable local entries for hostname %q, want exactly one",
			matches, hostname)
	}
	return nil
}

func startupLogShowsBindCollision(runDir string) bool {
	log, err := os.ReadFile(filepath.Join(runDir, "log", "stdout.log"))
	if err != nil {
		return false
	}
	text := string(log)
	return strings.Contains(text, "Cannot bind to ip") ||
		(strings.Contains(text, "bind() on ip") && strings.Contains(text, "failed"))
}

// Stop terminates the daemon gracefully, escalating to SIGKILL.
func (d *Daemon) Stop() error {
	if d.process == nil {
		return nil
	}

	termWait := d.termTimeout
	if termWait <= 0 {
		termWait = defaultTermWait
	}
	killWait := d.killTimeout
	if killWait <= 0 {
		killWait = defaultKillWait
	}

	termErr := d.process.Signal(syscall.SIGTERM)
	if errors.Is(termErr, os.ErrProcessDone) {
		termErr = nil
	}
	select {
	case waitErr := <-d.waitCh:
		d.process = nil
		d.processPID = 0
		return errors.Join(termErr, waitErr)
	case <-time.After(termWait):
	}

	killErr := d.process.Kill()
	if errors.Is(killErr, os.ErrProcessDone) {
		killErr = nil
	}
	select {
	case <-d.waitCh:
		d.process = nil
		d.processPID = 0
		return errors.Join(termErr, killErr)
	case <-time.After(killWait):
		return errors.Join(
			termErr,
			killErr,
			fmt.Errorf("daemon: process PID %d did not deliver reap result within %s after SIGKILL",
				d.processPID, killWait))
	}
}

// WaitExit waits for a daemon that exits by itself (a shutdown command, a fatal error) and returns its exit status.
func (d *Daemon) WaitExit(timeout time.Duration) (int, error) {
	if d.process == nil {
		return 0, errors.New("daemon: not running")
	}
	select {
	case werr := <-d.waitCh:
		d.process = nil
		d.processPID = 0
		if werr == nil {
			return 0, nil
		}
		var exitErr *exec.ExitError
		if errors.As(werr, &exitErr) {
			return exitErr.ExitCode(), nil
		}
		return -1, werr
	case <-time.After(timeout):
		return -1, fmt.Errorf("daemon: PID %d still running after %s", d.processPID, timeout)
	}
}

// Restart stops the daemon and boots it again on the same run dir and port,
// exercising the journal-replay read path.
func (d *Daemon) Restart() error {
	if err := d.Stop(); err != nil {
		return err
	}
	return d.launch()
}

// queryClient bounds every corpus query: no legitimate corpus query takes
// more than a few seconds, so a stalled daemon fails the test crisply
// instead of hanging it until the go test framework panics.
var queryClient = &http.Client{Timeout: 30 * time.Second}

// getRawBody performs a bounded GET and returns the raw response body.
func getRawBody(u string) ([]byte, error) {
	return getRawBodyWithClient(queryClient, u)
}

func getRawBodyWithClient(client *http.Client, u string) ([]byte, error) {
	resp, err := client.Get(u)
	if err != nil {
		return nil, fmt.Errorf("daemon: GET %s: %w", u, err)
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("daemon: read %s: %w", u, err)
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("daemon: GET %s: HTTP %d: %s", u, resp.StatusCode, body)
	}
	return body, nil
}

// getJSON performs a bounded GET and parses the JSON response.
func getJSON(u string) (map[string]any, error) {
	return getJSONWithClient(queryClient, u)
}

func getJSONWithClient(client *http.Client, u string) (map[string]any, error) {
	body, err := getRawBodyWithClient(client, u)
	if err != nil {
		return nil, err
	}
	var doc map[string]any
	if err := json.Unmarshal(body, &doc); err != nil {
		return nil, fmt.Errorf("daemon: parse %s: %w (body %q)", u, err, truncate(body, 300))
	}
	return doc, nil
}

// DataV1Raw queries /host/<host>/api/v1/data and returns the raw response
// body — the classic formatter surface (csv, tsv, ssv, html, arrays…)
// asserted byte-level by the formatter layer.
func (d *Daemon) DataV1Raw(host string, params url.Values) (string, error) {
	body, err := getRawBody(fmt.Sprintf("%s/host/%s/api/v1/data?%s", d.BaseURL, url.PathEscape(host), params.Encode()))
	return string(body), err
}

// DataV3All queries /api/v3/data (all nodes of the agent) with the given
// parameters — the multi-node query surface used by group-by layers.
func (d *Daemon) DataV3All(params url.Values) (map[string]any, error) {
	return getJSON(fmt.Sprintf("%s/api/v3/data?%s", d.BaseURL, params.Encode()))
}

// DataV3 queries /host/<host>/api/v3/data with the given parameters and
// returns the parsed JSON document.
func (d *Daemon) DataV3(host string, params url.Values) (map[string]any, error) {
	return getJSON(fmt.Sprintf("%s/host/%s/api/v3/data?%s", d.BaseURL, url.PathEscape(host), params.Encode()))
}

// HostJSON queries an arbitrary /host/<host>/<endpoint> API path (e.g.
// "api/v2/weights") and returns the parsed JSON document.
func (d *Daemon) HostJSON(host, endpoint string, params url.Values) (map[string]any, error) {
	return getJSON(fmt.Sprintf("%s/host/%s/%s?%s", d.BaseURL, url.PathEscape(host), endpoint, params.Encode()))
}

func truncate(b []byte, n int) string {
	if len(b) <= n {
		return string(b)
	}
	return string(b[:n]) + "..."
}

// DataParams returns the corpus defaults for a tier0 read-back query on one
// context: absolute window (after, before] with the given bucket count
// (points = window / update_every for identity read-back).
func DataParams(context string, after, before, points int64) url.Values {
	return url.Values{
		"scope_contexts": {context},
		"after":          {strconv.FormatInt(after, 10)},
		"before":         {strconv.FormatInt(before, 10)},
		"points":         {strconv.FormatInt(points, 10)},
		"time_group":     {"average"},
		"group_by":       {"dimension"},
		"aggregation":    {"avg"},
		"format":         {"json2"},
		"options":        {"jsonwrap"},
	}
}

// DataParamsTier returns a forced-tier read-back query: tier=N pins the
// query plan to that tier (RRDR_OPTION_SELECTED_TIER — no tier switching, no
// cross-tier gap filling) and natural points snap the view update_every to
// the tier granularity, so an aligned window reads back one bucket per tier
// point. timeGroup selects which STORAGE_POINT field the value carries
// (query-execute.c tier fetch): sum, min, max, or average (= sum/count).
func DataParamsTier(context string, tier int, after, before, points int64, timeGroup string) url.Values {
	p := DataParams(context, after, before, points)
	p.Set("tier", strconv.Itoa(tier))
	p.Set("time_group", timeGroup)
	return p
}

// Retention is the db window the daemon reports for one query.
type Retention struct {
	FirstEntry int64
	LastEntry  int64
}

// QueryRetention extracts db.first_entry/db.last_entry from a json2 reply.
func QueryRetention(doc map[string]any) (Retention, bool) {
	db, ok := doc["db"].(map[string]any)
	if !ok {
		return Retention{}, false
	}
	first, ok1 := jsonInt64(db["first_entry"])
	last, ok2 := jsonInt64(db["last_entry"])
	if !ok1 || !ok2 {
		return Retention{}, false
	}
	return Retention{FirstEntry: first, LastEntry: last}, true
}

func jsonInt64(value any) (int64, bool) {
	number, ok := value.(float64)
	if !ok || math.Trunc(number) != number ||
		number < -float64(uint64(1)<<63) || number >= float64(uint64(1)<<63) {
		return 0, false
	}
	return int64(number), true
}

// ringEntries is the size of a child's ring in a ram or alloc stream memory mode (0: not a ring). The template leaves
// [db] history at its default, 3600; ram rounds it up to whole pages (align_entries_to_pagesize, src/database/rrd.c)
// and alloc keeps it.
func ringEntries(mode string) int64 {
	const history, sizeofStorageNumber = 3600, 4
	switch mode {
	case "ram":
		page := int64(os.Getpagesize())
		size := (history*sizeofStorageNumber + page - 1) / page * page
		return size / sizeofStorageNumber
	case "alloc":
		return history
	}
	return 0
}

// ExpectedFirstEntry is db.first_entry for a contiguous series whose first and last samples are at first and last,
// ue apart: dbengine reports the first sample, a ring the start of its oldest stored interval
// (rrddim_query_oldest_time_s, src/database/ram/rrddim_mem.c).
func (d *Daemon) ExpectedFirstEntry(first, last, ue int64) int64 {
	n := ringEntries(streamMemoryMode(d.Opts))
	if n == 0 {
		return first
	}
	return max(first-ue, last-n*ue)
}

// WaitRetention polls the context on host until the daemon reports exactly
// the expected retention window — the corpus settle barrier. first and last
// are the fixture's first and last samples; a ring's first entry is derived
// from them with the context's db.update_every. It returns the last observed
// retention on timeout.
func (d *Daemon) WaitRetention(host, context string, first, last int64, timeout time.Duration) (Retention, error) {
	deadline := time.Now().Add(timeout)
	var seen Retention
	want := first
	ring := ringEntries(streamMemoryMode(d.Opts)) > 0
	for {
		doc, err := d.DataV3(host, DataParams(context, first-1, last, last-first+1))
		if err == nil {
			if ret, ok := QueryRetention(doc); ok {
				seen = ret
				ready := true
				if ring {
					db, _ := doc["db"].(map[string]any)
					ue, ok := jsonInt64(db["update_every"])
					if ready = ok && ue > 0; ready {
						want = d.ExpectedFirstEntry(first, last, ue)
					}
				}
				if ready && ret.FirstEntry == want && ret.LastEntry == last {
					return ret, nil
				}
			}
		}
		if time.Now().After(deadline) {
			return seen, fmt.Errorf("daemon: retention not settled on %s/%s after %s: have [%d,%d] want [%d,%d]",
				host, context, timeout, seen.FirstEntry, seen.LastEntry, want, last)
		}
		time.Sleep(200 * time.Millisecond)
	}
}

func yesNo(b bool) string {
	if b {
		return "yes"
	}
	return "no"
}
