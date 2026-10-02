// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"maps"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/plugin"
)

// The vnode the fake plugins define, and a GUID no host has.
const (
	vnodeGUID   = "5a1e0000-0000-4000-8000-0000000000d1"
	vnodeName   = "parity-vnode"
	unknownGUID = "5a1e0000-0000-4000-8000-0000000000d9"
)

// vnodeDefine is the HOST_DEFINE block of a vnode: its labels (name, value pairs), then HOST_DEFINE_END.
func vnodeDefine(guid, hostname string, labels ...string) string {
	var b strings.Builder
	fmt.Fprintf(&b, "HOST_DEFINE %s '%s'\n", guid, hostname)
	for i := 0; i+1 < len(labels); i += 2 {
		fmt.Fprintf(&b, "HOST_LABEL '%s' '%s'\n", labels[i], labels[i+1])
	}
	b.WriteString("HOST_DEFINE_END\n")
	return b.String()
}

// vnodeLabels are the labels the vnode cases define it with: its system info, a label of its own, and the stale
// period C zeroes before use (D145.1).
var vnodeLabels = []string{"_os_name", "Linux", "_kernel_version", "6.1.0", "_architecture", "x86_64",
	"role", "web", "_node_stale_after_seconds", "2"}

// compareVnodeViews compares what each side shows of the vnode under its first name (compareVnodeViewsAs).
func compareVnodeViews(t *testing.T, p *Pair, stage string) {
	t.Helper()
	compareVnodeViewsAs(t, p, stage, vnodeName)
}

// vnodeRefusal is each side's answer to a raw STREAM request for the vnode's GUID.
func vnodeRefusal(t *testing.T, p *Pair) [2]string {
	t.Helper()
	return streamAnswers(t, p, vnodeName, vnodeGUID)
}

// badVnodeCase is a plugin whose first line is a refused vnode keyword: C's disable record, then the plugin is not
// started again (it reports success without data: C's backoff, cut short by the stop).
func badVnodeCase(line, reason string) pluginCase {
	return pluginCase{
		sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{{Emit: line}, {Hang: true}}}}},
		play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
			waitPlugin(t, p, ls, "the plugin did not end", startEnded(1))
			time.Sleep(2 * time.Second)
		},
		guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
			if len(starts) != 1 {
				t.Errorf("oracle: %d starts", len(starts))
			}
			hasRecord(t, classes, "collector.log thread=PD[difftest]", reason)
		},
	}
}

// TestPluginsVnodes (check `plugins.vnodes`, M8 commit 4, D145): the fake plugin defines a vnode and collects into
// it; what both agents show of it, their records, its end with the run, and C's refusals.
func TestPluginsVnodes(t *testing.T) {
	hang := plugin.Step{Hang: true}
	define := plugin.Step{Emit: vnodeDefine(vnodeGUID, vnodeName, vnodeLabels...)}
	cases := map[string]pluginCase{
		// a vnode defined with labels, collected, then localhost again: both agents' views of it while the plugin
		// holds; at the stop the run's end lets it go
		"define": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				define,
				{Collect: &plugin.Collect{Chart: "difftest.vn", Dims: []string{"x"}, N: 3}},
				{Emit: "HOST localhost\n"},
				{Collect: &plugin.Collect{Chart: "difftest.lo", Dims: []string{"x"}, N: 2}},
				{WaitFile: "release-1"},
				hang,
			}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not collect and wait", func(s [][]plugin.Record) bool {
					return len(s) >= 1 && plugin.Has(s[0], "waiting", "release-1")
				})
				time.Sleep(1500 * time.Millisecond)
				compareVnodeViews(t, p, "while defined")
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				class := "daemon.log thread=PD[difftest]"
				hasRecord(t, classes, class, fmt.Sprintf(`VNODE: Configuring node stale after 0 seconds for host \"%s\"`, vnodeName))
				hasRecord(t, classes, class, "PLUGINSD: Checking virtual status for "+vnodeName)
				hasRecord(t, classes, class, "PLUGINSD: Reseting virtual host status for "+vnodeName)
			},
		},
		// a run that ends lets its vnode go (archived, still listed); the next start defines it again
		"exit": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{define, {Collect: &plugin.Collect{Chart: "difftest.vx", Dims: []string{"x"}, N: 1}},
					{WaitFile: "release-1"}, {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{define, {WaitFile: "release-2"}, hang}},
			}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not collect", func(s [][]plugin.Record) bool {
					return len(s) >= 1 && plugin.Has(s[0], "waiting", "release-1")
				})
				compareVnodeViews(t, p, "while defined")
				for i := range ls {
					if err := ls[i].Release("release-1"); err != nil {
						t.Fatal(err)
					}
				}
				waitPlugin(t, p, ls, "the plugin did not end", startEnded(1))
				time.Sleep(500 * time.Millisecond)
				compareVnodeViews(t, p, "after the run's end")
				waitPlugin(t, p, ls, "no second start", func(s [][]plugin.Record) bool {
					return len(s) >= 2 && plugin.Has(s[1], "waiting", "release-2")
				})
				time.Sleep(500 * time.Millisecond)
				compareVnodeViews(t, p, "defined again")
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				if len(starts) != 2 {
					t.Errorf("oracle: %d starts", len(starts))
				}
			},
		},
		// a STREAM for the vnode's GUID is refused while a plugin collects it
		"refusal": {
			sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{define, {WaitFile: "release-1"}, hang}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				waitPlugin(t, p, ls, "the plugin did not define", func(s [][]plugin.Record) bool {
					return len(s) >= 1 && plugin.Has(s[0], "waiting", "release-1")
				})
				time.Sleep(500 * time.Millisecond)
				answers := vnodeRefusal(t, p)
				if !strings.Contains(answers[0], "you are trying to stream my vnode back") {
					t.Errorf("oracle's answer: %q", answers[0])
				}
				if answers[0] != answers[1] {
					t.Errorf("the answers differ\noracle:    %q\ncandidate: %q", answers[0], answers[1])
				}
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {},
		},
		// two plugins define the same GUID: when the first one's run ends, the second one's HOST takes it again
		"shared-guid": {
			sc: plugin.Scenario{Starts: []plugin.Start{
				{Steps: []plugin.Step{define, {WaitFile: "release-1"}, {Exit: plugin.ExitCode(0)}}},
				{Steps: []plugin.Step{hang}},
			}},
			enable: []string{"difftesttwo"},
			more: []morePlugin{{dir: "plugins.d", file: "difftesttwo.plugin", sc: plugin.Scenario{Starts: []plugin.Start{{Steps: []plugin.Step{
				{WaitFile: "release-0"}, define, {WaitFile: "release-2"}, {Emit: "HOST " + vnodeGUID + "\n"},
				{Collect: &plugin.Collect{Chart: "difftest.vs", Dims: []string{"x"}, N: 1}}, hang,
			}}}}}},
			play: func(t *testing.T, p *Pair, ls [2]plugin.Layout) {
				two := moreLayouts(p, "plugins.d", "difftesttwo.plugin")
				// the first plugin's define creates the host (its records on that thread), the second one's updates it:
				// defined together, the two plugin threads race to HOST_DEFINE_END's find-or-create
				// (pluginsd_parser.c:264, rrdhost.c:892-940)
				waitPlugin(t, p, ls, "the first plugin did not define", waiting(1, "release-1"))
				waitIngest(t, p, "virtual", 10*time.Second)
				releaseAll(t, two, "release-0")
				waitPlugin(t, p, two, "the second plugin did not define", waiting(1, "release-2"))
				time.Sleep(500 * time.Millisecond)
				for i := range ls {
					if err := ls[i].Release("release-1"); err != nil {
						t.Fatal(err)
					}
				}
				waitPlugin(t, p, ls, "the first plugin did not end", startEnded(1))
				time.Sleep(500 * time.Millisecond)
				for i := range two {
					if err := two[i].Release("release-2"); err != nil {
						t.Fatal(err)
					}
				}
				waitPlugin(t, p, two, "the second plugin did not collect", startHangs(1))
				time.Sleep(500 * time.Millisecond)
				compareVnodeViews(t, p, "taken again")
			},
			guard: func(t *testing.T, starts [][]plugin.Record, classes map[string][]string) {
				hasRecord(t, classes, "daemon.log thread=PD[difftest]", fmt.Sprintf("Host '%s' (at registry as '%s') with guid '%s' initialized", vnodeName, vnodeName, vnodeGUID))
				hasRecord(t, classes, "daemon.log thread=PD[difftesttwo]", fmt.Sprintf(`VNODE: Re-enabling virtual host \"%s\"`, vnodeName))
			},
			// the stop finds the first plugin's thread sleeping ten update everies after a run without data
			// (plugins_d.c:67-73) and the second's hanging
			mask: maskStopWalk,
		},
		"bad-guid":      badVnodeCase("HOST_DEFINE zz "+vnodeName+"\n", "PLUGINSD: keyword HOST_DEFINE: cannot parse MACHINE_GUID - is it a valid UUID?"),
		"unknown-host":  badVnodeCase("HOST "+unknownGUID+"\n", "PLUGINSD: keyword HOST: cannot find a host with this machine guid - have you created it?"),
		"label-first":   badVnodeCase("HOST_LABEL role web\n", "PLUGINSD: keyword HOST_LABEL: host is not defined, send HOST_DEFINE before this"),
		"end-first":     badVnodeCase("HOST_DEFINE_END\n", "PLUGINSD: keyword HOST_DEFINE_END: missing initialization, send HOST_DEFINE before this"),
		"nested-define": badVnodeCase(fmt.Sprintf("HOST_DEFINE %s v\nHOST_DEFINE %s v\n", vnodeGUID, vnodeGUID), "PLUGINSD: keyword HOST_DEFINE: another host definition is already open - did you send HOST_DEFINE_END?"),
	}
	maps.Copy(cases, moreVnodeCases())
	runPluginCases(t, cases)
}
