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
	binaries := map[Role]string{
		Oracle:    os.Getenv("PARITY_ORACLE"),
		Candidate: os.Getenv("PARITY_CANDIDATE"),
	}
	for role, bin := range binaries {
		if bin == "" {
			t.Fatalf("parity: set PARITY_ORACLE and PARITY_CANDIDATE (missing the %s binary)", role)
		}
	}

	p := &Pair{}
	for _, role := range []Role{Oracle, Candidate} {
		o := opts
		o.Binary = binaries[role]
		o.RunDir = runDir(t, role)
		o.Identity = &id
		d, err := daemon.Start(o)
		if err != nil {
			t.Fatalf("parity: start %s (%s): %v", role, o.Binary, err)
		}
		t.Cleanup(func() {
			if err := d.Stop(); err != nil {
				t.Errorf("parity: stop %s: %v", role, err)
			}
		})
		if role == Oracle {
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
