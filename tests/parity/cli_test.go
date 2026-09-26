// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"errors"
	"os"
	"os/exec"
	"regexp"
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
	errText := string(bytes.ReplaceAll(stderr.Bytes(), []byte(bin), []byte("<argv0>")))
	// the records logged while a -W get loads the configuration carry the clock and the thread id
	errText = cliTimeRe.ReplaceAllString(errText, "time=T ")
	errText = cliTidRe.ReplaceAllString(errText, " tid=N")
	return stdout.String(), errText, code
}

var (
	cliTimeRe = regexp.MustCompile(`time=\S+ `)
	cliTidRe  = regexp.MustCompile(` tid=\d+`)
)

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
		// -W options that print, or set what a later -W get prints
		"w-simple-pattern-positive": {"-W", "simple-pattern", "!veth0 veth*", "veth12"},
		"w-simple-pattern-negative": {"-W", "simple-pattern", "!veth0 veth*", "veth0"},
		"w-simple-pattern-not":      {"-W", "simple-pattern", "a*", "b"},
		"w-simple-pattern-usage":    {"-W", "simple-pattern", "x"},
		"w-set-usage":               {"-W", "set", "a", "b"},
		"w-set2-usage":              {"-W", "set2", "a", "b", "c"},
		"w-get-usage":               {"-W", "get"},
		"w-get2-usage":              {"-W", "get2", "a"},
		"w-get-hostname":            {"-W", "get", "global", "hostname", "x"},
		"w-get-db-default":          {"-W", "get", "db", "update every", "7"},
		"w-get-missing":             {"-W", "get", "nosuch", "key", "fallback"},
		"w-get2-cloud":              {"-W", "get2", "cloud", "global", "enabled", "yes"},
		"w-set-then-get":            {"-W", "set", "web", "default port", "12345", "-W", "get", "web", "default port", "1"},
		"w-set2-then-get2":          {"-W", "set2", "cloud", "global", "enabled", "no", "-W", "get2", "cloud", "global", "enabled", "yes"},
		"w-stacksize-then-get":      {"-W", "stacksize=1048576", "-W", "get", "global", "pthread stack size", "0"},
		"w-debug-flags-then-get":    {"-W", "debug_flags=0x10", "-W", "get", "logs", "debug flags", "0"},
		"w-unknown":                 {"-W", "nosuch"},
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
