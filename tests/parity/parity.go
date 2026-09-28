// SPDX-License-Identifier: GPL-3.0-or-later

// Package parity runs differential checks between two netdata daemons: the C
// agent (oracle) and an implementation under test (candidate). Each check boots
// both with the same identity, feeds them identical inputs through the
// tests/query-corpus primitives, and compares what they answer.
//
// Binaries come from the environment, never from defaults:
//
//	PARITY_ORACLE     the C netdata binary judged correct
//	PARITY_CANDIDATE  the binary under test (the oracle itself for null checks)
//	PARITY_KEEP=1     keep the daemons' run directories after the test
package parity

import (
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
)

// ScrubEnvironment removes variables that would make a daemon under test act
// on the outside world. NETDATA_CLAIM_* would claim every test daemon into a
// real Netdata Cloud room. Call it from TestMain before any daemon starts.
func ScrubEnvironment() {
	for _, name := range []string{
		"NETDATA_CLAIM_TOKEN", "NETDATA_CLAIM_ROOMS", "NETDATA_CLAIM_URL",
		"NETDATA_CLAIM_PROXY", "NETDATA_CLAIM_INSECURE",
		// the ACLK proxy resolution reads them; C and Rust log what they find
		"http_proxy", "https_proxy",
		// libcurl's (the crash report): a proxy or a no-proxy match would send it elsewhere (TestMain sets a guard)
		"HTTPS_PROXY", "all_proxy", "ALL_PROXY", "no_proxy", "NO_PROXY",
		// nd_is_running_under_ci(): set, even empty, they change whether a crash report goes out
		"CI", "CONTINUOUS_INTEGRATION", "BUILD_NUMBER", "RUN_ID", "TRAVIS", "GITHUB_ACTIONS", "GITHUB_TOKEN",
		"GITLAB_CI", "CIRCLECI", "APPVEYOR", "BITBUCKET_BUILD_NUMBER", "SYSTEM_TEAMFOUNDATIONCOLLECTIONURI", "TF_BUILD",
		"BAMBOO_BUILDKEY", "GO_PIPELINE_NAME", "HUDSON_URL", "TEAMCITY_VERSION", "CI_NAME", "CI_WORKER", "CI_SERVER",
		"HEROKU_TEST_RUN_ID", "BUILDKITE", "DRONE", "SEMAPHORE", "NETLIFY", "NOW_BUILDER",
	} {
		os.Unsetenv(name)
	}
}

// Role names one side of a pair.
type Role string

const (
	Oracle    Role = "oracle"
	Candidate Role = "candidate"
)

// Pair is an oracle and a candidate daemon booted with the same identity.
type Pair struct {
	Oracle    *daemon.Daemon
	Candidate *daemon.Daemon
}

// Each returns the pair's daemons in a fixed order.
func (p *Pair) Each() []struct {
	Role   Role
	Daemon *daemon.Daemon
} {
	return []struct {
		Role   Role
		Daemon *daemon.Daemon
	}{{Oracle, p.Oracle}, {Candidate, p.Candidate}}
}

// StartPair boots the oracle and the candidate with identical options and
// identity, and stops both when the test ends.
func StartPair(t *testing.T, opts daemon.Options, id daemon.Identity) *Pair {
	t.Helper()
	return startPair(t, opts, id, binaries(t), [2]string{opts.SeedCache, opts.SeedCache}, [2]Role{Oracle, Candidate})
}

// stopBoth stops both daemons at once (neither side's exit waits for the other's) and returns their caches.
func stopBoth(t *testing.T, p *Pair) [2]string {
	t.Helper()
	var wg sync.WaitGroup
	var errs [2]error
	for i, side := range p.Each() {
		wg.Add(1)
		go func() {
			defer wg.Done()
			errs[i] = side.Daemon.Stop()
		}()
	}
	wg.Wait()
	for i, side := range p.Each() {
		if errs[i] != nil {
			t.Fatalf("stop %s: %v", side.Role, errs[i])
		}
	}
	return [2]string{filepath.Join(p.Oracle.Opts.RunDir, "cache"), filepath.Join(p.Candidate.Opts.RunDir, "cache")}
}

// binaries are the oracle's and the candidate's executables.
func binaries(t *testing.T) [2]string {
	t.Helper()
	bins := [2]string{os.Getenv("PARITY_ORACLE"), os.Getenv("PARITY_CANDIDATE")}
	for i, role := range []Role{Oracle, Candidate} {
		if bins[i] == "" {
			t.Fatalf("parity: set PARITY_ORACLE and PARITY_CANDIDATE (missing the %s binary)", role)
		}
	}
	return bins
}

// startPair boots two daemons with the same options and identity, each with its own binary and seed cache, in run
// directories named after the roles, and stops both when the test ends. The first is the pair's Oracle side.
func startPair(t *testing.T, opts daemon.Options, id daemon.Identity, bins, seeds [2]string, roles [2]Role) *Pair {
	t.Helper()
	return startPairWith(t, opts, id, bins, seeds, roles, nil)
}

// startPairWith is startPair with `prepare` run on each side's run directory before its daemon starts (it may lay
// out the cache itself: the seed is copied after it).
func startPairWith(t *testing.T, opts daemon.Options, id daemon.Identity, bins, seeds [2]string, roles [2]Role,
	prepare func(t *testing.T, runDir string)) *Pair {
	t.Helper()
	p := &Pair{}
	for i, role := range roles {
		o := opts
		o.Binary = bins[i]
		o.SeedCache = seeds[i]
		o.RunDir = runDir(t, role)
		o.Identity = &id
		if prepare != nil {
			prepare(t, o.RunDir)
		}
		d, err := daemon.Start(o)
		if err != nil {
			t.Fatalf("parity: start %s (%s): %v", role, o.Binary, err)
		}
		t.Cleanup(func() {
			if err := d.Stop(); err != nil {
				t.Errorf("parity: stop %s: %v", role, err)
			}
		})
		if i == 0 {
			p.Oracle = d
		} else {
			p.Candidate = d
		}
	}
	return p
}

func runDir(t *testing.T, role Role) string {
	if os.Getenv("PARITY_KEEP") != "1" {
		return filepath.Join(t.TempDir(), string(role))
	}
	dir, err := os.MkdirTemp("", "parity-"+string(role)+"-")
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("parity: %s run dir kept: %s", role, dir)
	return dir
}

// Response is one HTTP answer.
type Response struct {
	Status      int
	ContentType string
	Body        []byte
}

var client = &http.Client{Timeout: 30 * time.Second}

// Get fetches path (with params) from one daemon.
func Get(d *daemon.Daemon, path string, params url.Values) (Response, error) {
	u := d.BaseURL + path
	if len(params) > 0 {
		u += "?" + params.Encode()
	}
	resp, err := client.Get(u)
	if err != nil {
		return Response{}, err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return Response{}, err
	}
	return Response{Status: resp.StatusCode, ContentType: resp.Header.Get("Content-Type"), Body: body}, nil
}

// Rules declare how a check compares one endpoint.
type Rules struct {
	Masks     []Mask   // values hidden on both sides
	Unordered []string // object paths whose member order is not a contract
	// Settle, when non-zero, re-fetches both sides until they agree or the
	// deadline passes, for answers that converge after startup (a daemon's
	// own charts appear over its first seconds). A persistent divergence is
	// still reported once the deadline passes.
	Settle time.Duration
}

// CompareJSON fetches path from both daemons and returns every divergence in
// status, content type and masked body.
func (p *Pair) CompareJSON(path string, params url.Values, rules Rules) ([]Difference, error) {
	deadline := time.Now().Add(rules.Settle)
	for {
		diffs, err := p.compareJSONOnce(path, params, rules)
		if err != nil || len(diffs) == 0 || time.Now().After(deadline) {
			return diffs, err
		}
		time.Sleep(time.Second)
	}
}

func (p *Pair) compareJSONOnce(path string, params url.Values, rules Rules) ([]Difference, error) {
	var got [2]Response
	for i, side := range p.Each() {
		r, err := Get(side.Daemon, path, params)
		if err != nil {
			return nil, fmt.Errorf("parity: GET %s from %s: %w", path, side.Role, err)
		}
		got[i] = r
	}
	var diffs []Difference
	if got[0].Status != got[1].Status {
		diffs = append(diffs, Difference{Path: "<status>", Oracle: fmt.Sprint(got[0].Status), Candidate: fmt.Sprint(got[1].Status)})
	}
	if got[0].ContentType != got[1].ContentType {
		diffs = append(diffs, Difference{Path: "<content-type>", Oracle: got[0].ContentType, Candidate: got[1].ContentType})
	}
	var docs [2]Value
	for i, r := range got {
		v, err := ParseJSON(r.Body)
		if err != nil {
			return nil, fmt.Errorf("parity: %s answered %s with invalid JSON: %w", p.Each()[i].Role, path, err)
		}
		docs[i] = ApplyMasks(v, rules.Masks)
	}
	return append(diffs, Compare(docs[0], docs[1], rules.Unordered...)...), nil
}
