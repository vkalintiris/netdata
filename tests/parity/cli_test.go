// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"errors"
	"os"
	"os/exec"
	"testing"
	"time"
)

// runPrint runs a binary with arguments that print and exit, returning stdout, stderr (with the binary's own path
// replaced, since glibc's getopt messages name argv[0]) and the exit code.
func runPrint(t *testing.T, bin string, args ...string) (string, string, int) {
	t.Helper()
	cmd := exec.Command(bin, args...)
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	done := make(chan error, 1)
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	go func() { done <- cmd.Wait() }()
	code := 0
	select {
	case err := <-done:
		var exit *exec.ExitError
		if errors.As(err, &exit) {
			code = exit.ExitCode()
		} else if err != nil {
			t.Fatal(err)
		}
	case <-time.After(30 * time.Second):
		_ = cmd.Process.Kill()
		t.Fatalf("%s %v did not exit", bin, args)
	}
	return stdout.String(), string(bytes.ReplaceAll(stderr.Bytes(), []byte(bin), []byte("<argv0>"))), code
}

// TestCLIPrintOptions compares the options that print and exit without starting the daemon. Both binaries must be
// built with the same install paths, because -h names CONFIG_DIR.
func TestCLIPrintOptions(t *testing.T) {
	oracle, candidate := os.Getenv("PARITY_ORACLE"), os.Getenv("PARITY_CANDIDATE")
	if oracle == "" || candidate == "" {
		t.Fatal("parity: set PARITY_ORACLE and PARITY_CANDIDATE")
	}
	cases := map[string][]string{
		"version":          {"-v"},
		"version-upper":    {"-V"},
		"help":             {"-h"},
		"invalid-option":   {"-x"},
		"missing-argument": {"-p"},
		// Arguments are bytes: a non-UTF-8 operand is ignored, a non-UTF-8 option byte is named as glibc does.
		"non-utf8-operand": {"\xff", "-v"},
		"non-utf8-option":  {"-\xff"},
	}
	for name, args := range cases {
		t.Run(name, func(t *testing.T) {
			oOut, oErr, oCode := runPrint(t, oracle, args...)
			cOut, cErr, cCode := runPrint(t, candidate, args...)
			if oOut != cOut {
				t.Errorf("stdout differs\noracle:    %q\ncandidate: %q", oOut, cOut)
			}
			if oErr != cErr {
				t.Errorf("stderr differs\noracle:    %q\ncandidate: %q", oErr, cErr)
			}
			if oCode != cCode {
				t.Errorf("exit code: oracle %d, candidate %d", oCode, cCode)
			}
		})
	}
}
