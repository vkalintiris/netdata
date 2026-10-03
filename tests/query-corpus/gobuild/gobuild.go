// SPDX-License-Identifier: GPL-3.0-or-later

// Package gobuild builds the harness's own helper programs (the fake plugin's engine, the recording notifier) for the
// checks that install them in a run directory: static, stdlib only, offline, each once per process.
package gobuild

import (
	"fmt"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"runtime"
	"sync"
)

var built struct {
	mu   sync.Mutex
	dir  string
	bins map[string]result
}

type result struct {
	path string
	err  error
}

// Build builds a main package of this module (e.g. "./plugin/cmd/difftest") with `go` from PATH and returns the
// binary's path; a second call for the same package returns the first one's result. Cleanup removes the binaries.
func Build(pkg string) (string, error) {
	built.mu.Lock()
	defer built.mu.Unlock()
	if r, ok := built.bins[pkg]; ok {
		return r.path, r.err
	}
	out, err := build(pkg)
	if built.bins == nil {
		built.bins = map[string]result{}
	}
	built.bins[pkg] = result{out, err}
	return out, err
}

func build(pkg string) (string, error) {
	goBin, err := exec.LookPath("go")
	if err != nil {
		return "", fmt.Errorf("gobuild: %s needs go on PATH: %w", pkg, err)
	}
	_, self, _, _ := runtime.Caller(0)
	module := filepath.Dir(filepath.Dir(self))
	if built.dir == "" {
		dir, err := os.MkdirTemp("", "harness-gobuild-")
		if err != nil {
			return "", err
		}
		built.dir = dir
	}
	out := filepath.Join(built.dir, path.Base(pkg))
	cmd := exec.Command(goBin, "build", "-trimpath", "-o", out, pkg)
	cmd.Dir = module
	cmd.Env = append(os.Environ(), "CGO_ENABLED=0", "GOPROXY=off", "GOTOOLCHAIN=local", "GOWORK=off")
	if b, err := cmd.CombinedOutput(); err != nil {
		return "", fmt.Errorf("gobuild: building %s: %v: %s", pkg, err, b)
	}
	return out, nil
}

// Cleanup removes the built binaries (the run directories keep their links) and forgets them.
func Cleanup() {
	built.mu.Lock()
	defer built.mu.Unlock()
	if built.dir != "" {
		_ = os.RemoveAll(built.dir)
	}
	built.dir, built.bins = "", nil
}
