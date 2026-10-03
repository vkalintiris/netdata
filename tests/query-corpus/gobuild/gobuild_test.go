// SPDX-License-Identifier: GPL-3.0-or-later

package gobuild

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// A package is built once per process, a missing one fails with the compiler's text (kept for the next call too),
// and Cleanup removes what was built.
func TestBuildOncePerPackage(t *testing.T) {
	t.Cleanup(Cleanup)
	first, err := Build("./plugin/cmd/difftest")
	if err != nil {
		t.Fatal(err)
	}
	st, err := os.Stat(first)
	if err != nil || st.Mode().Perm()&0o111 == 0 || filepath.Base(first) != "difftest" {
		t.Fatalf("%s: %v %v", first, st, err)
	}
	again, err := Build("./plugin/cmd/difftest")
	if err != nil || again != first {
		t.Errorf("the second call: %q, %v; want %q", again, err, first)
	}
	if st2, err := os.Stat(again); err != nil || !st2.ModTime().Equal(st.ModTime()) {
		t.Errorf("the binary was built again: %v %v", st2, err)
	}

	for range 2 {
		if _, err := Build("./gobuild/no-such-package"); err == nil || !strings.Contains(err.Error(), "building ./gobuild/no-such-package") {
			t.Errorf("a missing package: %v", err)
		}
	}

	Cleanup()
	if _, err := os.Stat(first); !os.IsNotExist(err) {
		t.Errorf("after Cleanup: %v", err)
	}
}
