// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"os"
	"regexp"
	"strings"
	"testing"
)

// buildinfoDiff is a build info slot where the Rust agent says other than C's production build (D87.1): C's JSON
// value, the candidate's, the candidate's text value, and what closes the difference ("*" accepts any string).
type buildinfoDiff struct{ c, rust, text, closes string }

// buildinfoDiffs are those slots, by `section.key` as -W buildinfojson names them. A milestone that ports the
// capability deletes its entries; a candidate that matches C on a listed slot fails until then.
var buildinfoDiffs = func() map[string]buildinfoDiff {
	no := func(closes string) buildinfoDiff { return buildinfoDiff{"true", "false", "NO", closes} }
	d := map[string]buildinfoDiff{
		// F2 (D87.2): each build's own command
		"package.configure": {"*", "*", "*", "F2"},
		// F3 (D87.3): no stack trace backend
		"libs.stacktraces": {`"libbacktrace (mmap, threads, data)"`, "false", "unknown", "F3"},
		"libs.protobuf":    {`"system"`, "false", "NO", "M11 Cloud"},
	}
	for closes, keys := range map[string][]string{
		"M11 Cloud": {"features.cloud", "connectivity.aclk"},
		"M14 ML":    {"features.ml"},
		// the external plugins' slots follow CMake's build of the plugins, the C sources' removal (D182.10, D135.10)
		"M16 packaging": {"libs.libyaml", "libs.libcap", "libs.libmnl", "plugins.apps", "plugins.charts.d",
			"plugins.debugfs", "plugins.cups", "plugins.ebpf", "plugins.freeipmi", "plugins.network-viewer",
			"plugins.systemd-journal", "plugins.nfacct", "plugins.perf", "plugins.slabinfo", "plugins.xen"},
		"M12 internal collectors": {"plugins.cgroups", "plugins.cgroup-network", "plugins.proc", "plugins.tc",
			"plugins.diskspace", "plugins.timex", "plugins.idlejitter"},
		"M13 statsd and exporting": {"plugins.statsd", "exporters.mongodb", "exporters.openmetrics",
			"exporters.prom-remote-write", "exporters.graphite", "exporters.graphite:http", "exporters.json",
			"exporters.json:http", "exporters.opentsdb", "exporters.opentsdb:http", "exporters.allmetrics",
			"exporters.shell"},
	} {
		for _, key := range keys {
			d[key] = no(closes)
		}
	}
	return d
}()

var (
	decimalRe          = regexp.MustCompile(`^"[0-9]+"$`)
	buildinfoSectionRe = regexp.MustCompile(`^    "([^"]+)":\{$`)
	buildinfoMemberRe  = regexp.MustCompile(`^        "([^"]+)":(.*?)(,?)$`)
)

// buildinfoSlots are a -W buildinfojson document's slots in order, as `section.key` and the value's JSON text.
func buildinfoSlots(t *testing.T, role Role, out string) ([]string, map[string]string) {
	t.Helper()
	v, err := ParseJSON([]byte(out))
	if err != nil {
		t.Fatalf("%s: %v", role, err)
	}
	return buildinfoSlotsOf(v)
}

// buildinfoSlotsOf are the slots of a build info object (`build_info_to_json_object()`).
func buildinfoSlotsOf(v Value) ([]string, map[string]string) {
	var keys []string
	values := map[string]string{}
	for _, section := range v.Members {
		for _, m := range section.Value.Members {
			key := section.Key + "." + m.Key
			keys = append(keys, key)
			values[key] = m.Value.String()
		}
	}
	return keys, values
}

// compareBuildinfoSlots compares two build infos slot by slot: the same slots in order, equal values but for the
// available memory (a decimal string on both sides) and `buildinfoDiffs`, whose values must be the listed ones (all
// equal when the oracle is its own candidate).
func compareBuildinfoSlots(t *testing.T, where string, keys, cKeys []string, oValues, cValues map[string]string) {
	t.Helper()
	same := sameBinary(t)
	if strings.Join(keys, " ") != strings.Join(cKeys, " ") {
		t.Fatalf("%s: slots differ\noracle:    %v\ncandidate: %v", where, keys, cKeys)
	}
	listed := 0
	for _, key := range keys {
		o, c := oValues[key], cValues[key]
		d, ok := buildinfoDiffs[key]
		switch {
		case key == "runtime.mem-available":
			if !decimalRe.MatchString(o) || !decimalRe.MatchString(c) {
				t.Errorf("%s: %s: oracle %s, candidate %s", where, key, o, c)
			}
		case ok && !same:
			listed++
			any := func(want, got string) bool { return want == "*" && strings.HasPrefix(got, `"`) || want == got }
			if !any(d.c, o) || !any(d.rust, c) {
				t.Errorf("%s: %s (%s): oracle %s, candidate %s; listed %s and %s", where, key, d.closes, o, c, d.c,
					d.rust)
			}
			if d.c != "*" && o == c {
				t.Errorf("%s: %s: the candidate now says %s as C does: remove it from buildinfoDiffs (%s)", where,
					key, c, d.closes)
			}
		case o != c:
			t.Errorf("%s: %s: oracle %s, candidate %s", where, key, o, c)
		}
	}
	if !same && listed != len(buildinfoDiffs) {
		t.Errorf("%s: %d of the %d listed slots exist", where, listed, len(buildinfoDiffs))
	}
}

// TestCLIBuildInfo compares -W buildinfo, -W buildinfojson and -W cmakecache (check `cli.buildinfo`): stdout, stderr
// and the exit code. Both binaries must be built with the same install paths. The slots agree but for the available
// memory (a decimal string on both sides) and `buildinfoDiffs` (none when the oracle is its own candidate); the
// text's lines pair with the JSON's slots in order.
func TestCLIBuildInfo(t *testing.T) {
	oracle, candidate := os.Getenv("PARITY_ORACLE"), os.Getenv("PARITY_CANDIDATE")
	if oracle == "" || candidate == "" {
		t.Fatal("parity: set PARITY_ORACLE and PARITY_CANDIDATE")
	}
	type run struct {
		out, err string
		code     int
	}
	get := func(args ...string) [2]run {
		var got [2]run
		for i, bin := range []string{oracle, candidate} {
			got[i].out, got[i].err, got[i].code = runPrint(t, bin, args...)
		}
		if got[0].err != got[1].err || got[0].code != got[1].code {
			t.Errorf("%v: stderr and exit code differ\noracle (%d):    %q\ncandidate (%d): %q", args, got[0].code,
				got[0].err, got[1].code, got[1].err)
		}
		return got
	}
	same := sameBinary(t)
	json := get("-W", "buildinfojson")
	keys, oValues := buildinfoSlots(t, Oracle, json[0].out)
	cKeys, cValues := buildinfoSlots(t, Candidate, json[1].out)
	compareBuildinfoSlots(t, "-W buildinfojson", keys, cKeys, oValues, cValues)
	// the bytes: C's layout, the listed slots' and the available memory's values masked
	var masked [2]string
	for i := range json {
		section := ""
		var out []string
		for _, l := range strings.Split(json[i].out, "\n") {
			if m := buildinfoSectionRe.FindStringSubmatch(l); m != nil {
				section = m[1]
			} else if m := buildinfoMemberRe.FindStringSubmatch(l); m != nil {
				key := section + "." + m[1]
				if _, ok := buildinfoDiffs[key]; ok && !same || key == "runtime.mem-available" {
					l = `        "` + m[1] + `":<masked>` + m[3]
				}
			}
			out = append(out, l)
		}
		masked[i] = strings.Join(out, "\n")
	}
	if masked[0] != masked[1] {
		t.Errorf("-W buildinfojson bytes differ\n%s", firstDifference([]byte(masked[0]), []byte(masked[1])))
	}

	text := get("-W", "buildinfo")
	var lines [2][]string
	for i := range text {
		lines[i] = strings.Split(strings.TrimSuffix(text[i].out, "\n"), "\n")
	}
	if len(lines[0]) != len(lines[1]) || len(lines[0]) != len(keys)+13 {
		t.Fatalf("text lines: oracle %d, candidate %d, slots %d", len(lines[0]), len(lines[1]), len(keys))
	}
	slot := 0
	for i, o := range lines[0] {
		c := lines[1][i]
		if !strings.HasPrefix(o, "    ") {
			if o != c {
				t.Errorf("title %d: oracle %q, candidate %q", i, o, c)
			}
			continue
		}
		key := keys[slot]
		slot++
		oLabel, oValue, _ := strings.Cut(o, " : ")
		cLabel, cValue, _ := strings.Cut(c, " : ")
		d, ok := buildinfoDiffs[key]
		switch {
		case oLabel != cLabel:
			t.Errorf("%s: labels %q and %q", key, oLabel, cLabel)
		case key == "runtime.mem-available":
			if !decimalRe.MatchString(fmt.Sprintf("%q", oValue)) || !decimalRe.MatchString(fmt.Sprintf("%q", cValue)) {
				t.Errorf("%s: oracle %q, candidate %q", key, oValue, cValue)
			}
		case ok && !same:
			if d.text != "*" && cValue != d.text {
				t.Errorf("%s (%s): the candidate's text %q, listed %q", key, d.closes, cValue, d.text)
			}
		case oValue != cValue:
			t.Errorf("%s: oracle %q, candidate %q", key, oValue, cValue)
		}
	}

	cache := get("-W", "cmakecache")
	if cache[0].out != cache[1].out {
		t.Errorf("-W cmakecache differs\n%s", firstDifference([]byte(cache[0].out), []byte(cache[1].out)))
	}
}
