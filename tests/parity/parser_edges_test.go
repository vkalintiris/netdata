package parity

import (
	"bytes"
	"encoding/json"
	"fmt"
	"maps"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/netdata/netdata/tests/query-corpus/daemon"
	"github.com/netdata/netdata/tests/query-corpus/stream"
)

// stream.parser-edges (D128; design evidence/2026-10-01-design-parser-edges.md): scripted children send what a
// well-behaved child never does, one host per case, to a C and a Rust parent; each parent's records written while
// the case ran are compared in file order, the limiters' drops included. The cases run one after another, at least
// 1.5 s apart, so no limiter's second spans two of them.

// edgeMarkers are the records a case compares: the parser's, the receiver's and the decompressor's.
var edgeMarkers = []string{`msg="PLUGINSD`, `msg="STREAM RCV[x]`, `msg="STREAM_DECOMPRESS`, `receiver disconnected`,
	`msg="REPLAY`, `msg="STREAM SND REPLAY ERROR`}

// parentClockRe is the parent's own clock in a record, which the sides read a second or so apart.
var parentClockRe = regexp.MustCompile(`now is \d+ \[parent wall clock\]`)

// edgeHost is the n-th case's child.
func edgeHost(n int) stream.HostInfo {
	return stream.HostInfo{Hostname: fmt.Sprintf("edge-%d", n), MachineGUID: fmt.Sprintf("5a1e0000-0000-4000-8000-%012d", 300+n)}
}

// edgeRecords are the records of a stream thread, from the daemon and collector logs, written after `from` lines of
// each, normalized.
func edgeRecords(t *testing.T, d *daemon.Daemon, from [2]int) []string {
	t.Helper()
	var out []string
	for i, name := range []string{"daemon.log", "collector.log"} {
		lines := logLines(t, d.Opts.RunDir, name)
		if from[i] > len(lines) {
			continue
		}
		for _, l := range lines[from[i]:] {
			if !strings.Contains(l, "thread=STREAM[") {
				continue
			}
			keep := false
			for _, m := range edgeMarkers {
				keep = keep || strings.Contains(l, m)
			}
			if !keep {
				continue
			}
			n := normalizeLog(l, d.Opts.RunDir, "")
			n = anyLocalPortRe.ReplaceAllString(n, "127.0.0.1${1}P")
			n = threadNRe.ReplaceAllString(n, "${1}[n]")
			n = receivedRe.ReplaceAllString(n, " ${1}=N")
			n = parentClockRe.ReplaceAllString(n, "now is T [parent wall clock]")
			out = append(out, name+": "+n)
		}
	}
	return out
}

// edgeMark is where each log of the daemon ends now.
func edgeMark(t *testing.T, d *daemon.Daemon) [2]int {
	t.Helper()
	return [2]int{len(logLines(t, d.Opts.RunDir, "daemon.log")), len(logLines(t, d.Opts.RunDir, "collector.log"))}
}

// edgeCase runs one case on each side in turn: a child connects with `caps`, `write` sends its lines, and the case
// waits until the parent closes the connection (or 5 s); the sides' records are compared and returned, and so are
// the REPLAY_CHART lines the parent sent down meanwhile (those `write` read itself included, through `down`).
func edgeCase(t *testing.T, p *Pair, n int, caps uint32, write func(c *stream.Conn) error) [2][]string {
	t.Helper()
	got, _ := edgeCaseDown(t, p, n, caps, true, func(c *stream.Conn, _ *[]string) error { return write(c) })
	return got
}

// replayLine is a parent's REPLAY_CHART line.
func replayLine(l string) bool { return strings.HasPrefix(l, "REPLAY_CHART ") }

// readReplay reads the parent's lines until `want` REPLAY_CHART lines arrived (or 5 s), appending them to `down`.
func readReplay(c *stream.Conn, down *[]string, want int) {
	deadline := time.Now().Add(5 * time.Second)
	for got := 0; got < want && time.Now().Before(deadline); {
		l, err := c.ReadLine(deadline)
		if err != nil {
			return
		}
		if replayLine(l) {
			*down = append(*down, l)
			got++
		}
	}
}

// edgeCaseDown is edgeCase with the REPLAY_CHART lines; without `closes` the parent keeps the connection, and the
// case reads its lines for 300 ms after `write`.
func edgeCaseDown(t *testing.T, p *Pair, n int, caps uint32, closes bool,
	write func(c *stream.Conn, down *[]string) error) ([2][]string, [2][]string) {
	t.Helper()
	var got, downs [2][]string
	for i, side := range p.Each() {
		from := edgeMark(t, side.Daemon)
		c, err := stream.Connect(side.Daemon.Addr, parentIdentity.StreamKey, edgeHost(n), caps)
		if err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		var down []string
		if err := write(c, &down); err != nil {
			t.Fatalf("%s: %v", side.Role, err)
		}
		_ = c.Flush()
		wait := 5 * time.Second
		if !closes {
			wait = 300 * time.Millisecond
		}
		deadline := time.Now().Add(wait)
		for time.Now().Before(deadline) {
			l, err := c.ReadLine(deadline)
			if err != nil {
				break
			}
			if replayLine(l) {
				down = append(down, l)
			}
		}
		downs[i] = down
		_ = c.Close()
		// the records of the removal follow the close
		time.Sleep(300 * time.Millisecond)
		got[i] = edgeRecords(t, side.Daemon, from)
	}
	t.Logf("oracle:\n%s\n%s", strings.Join(got[0], "\n"), strings.Join(downs[0], "\n"))
	diffLines(t, "records", got[0], got[1])
	diffLines(t, "replication requests", downs[0], downs[1])
	time.Sleep(1500 * time.Millisecond)
	return got, downs
}

// lines writes each line as it is.
func lines(c *stream.Conn, ls ...string) error {
	for _, l := range ls {
		c.Linef("%s", l)
	}
	return c.Flush()
}

func TestStreamParserEdges(t *testing.T) {
	// dbengine with a 60 s replication step, as a parent's requests are cut; host 900 replicates nothing
	noReplication := edgeHost(900).MachineGUID
	p := StartPair(t, daemon.Options{StorageTiers: 1, ReplicationStepSeconds: 60,
		StreamExtra: fmt.Sprintf("\n[%s]\n    enabled = yes\n    enable replication = no\n", noReplication)}, parentIdentity)
	n := 0
	next := func() int { n++; return n }

	// U: a line the parser refuses ends the connection with C's records; the line after it is never read
	t.Run("unknown-disconnects", func(t *testing.T) {
		chart := []string{"CHART 'e.u' '' 't' 'u' 'f' 'e.u' line 1 1 '' p m", "DIMENSION 'x' '' absolute 1 1 ''", ""}
		slotted := []string{"CHART SLOT:1 'e.u' '' 't' 'u' 'f' 'e.u' line 1 1 '' p m",
			"DIMENSION SLOT:1 'x' '' absolute 1 1 ''", ""}
		guid := func(k int) string { return edgeHost(k).MachineGUID }
		cases := []struct {
			name  string
			lines func(k int) []string
		}{
			{"host-define", func(int) []string { return append(chart, "HOST_DEFINE g") }},
			{"host", func(int) []string { return append(chart, "HOST g") }},
			{"host-define-end", func(int) []string { return append(chart, "HOST_DEFINE_END") }},
			{"host-label", func(int) []string { return append(chart, "HOST_LABEL a b") }},
			{"config", func(int) []string { return append(chart, "CONFIG x") }},
			{"trust-durations", func(int) []string { return append(chart, "TRUST_DURATIONS") }},
			{"flush", func(int) []string { return append(chart, "FLUSH") }},
			{"disable", func(int) []string { return append(chart, "DISABLE") }},
			{"exit", func(int) []string { return append(chart, "EXIT") }},
			{"plugin-keepalive", func(int) []string { return append(chart, "PLUGIN_KEEPALIVE") }},
			{"bogus", func(int) []string { return append(chart, "BOGUS") }},
			{"lowercase", func(int) []string { return append(chart, "begin2 'e.u' 1 5 #") }},
			{"empty-quoted", func(int) []string { return append(chart, "''") }},
			{"begin2-short", func(int) []string { return append(chart, "BEGIN2 'e.u' 1 5") }},
			{"rend-short", func(int) []string { return append(chart, "REND 1 2 3") }},
			{"claimed-id-one", func(k int) []string { return append(chart, "CLAIMED_ID '"+guid(k)+"'") }},
			{"claimed-id-bad-guid", func(int) []string {
				return append(chart, "CLAIMED_ID 'not-a-guid' 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'")
			}},
			{"claimed-id-bad-claim", func(k int) []string { return append(chart, "CLAIMED_ID '"+guid(k)+"' 'nope'") }},
			{"variable-no-name", func(int) []string { return append(chart, "VARIABLE HOST") }},
			{"variable-local-without-chart", func(int) []string { return []string{"VARIABLE LOCAL x = 1"} }},
			{"set2-slot-zero", func(int) []string {
				return append(slotted, "BEGIN2 'e.u' 1 5 #", "SET2 SLOT:65536 'x' 1 1 A")
			}},
		}
		oracleRecords := 0
		for _, tc := range cases {
			k := next()
			t.Run(tc.name, func(t *testing.T) {
				got := edgeCase(t, p, k, stream.CapsLive, func(c *stream.Conn) error {
					return lines(c, append(tc.lines(k), "BOGUS2")...)
				})
				for _, r := range got[0] {
					if strings.Contains(r, "BOGUS2") {
						t.Errorf("the line after the refused one was read: %s", r)
					}
				}
				oracleRecords += len(got[0])
			})
		}
		if oracleRecords < len(cases) {
			t.Errorf("the oracle wrote %d records for %d cases", oracleRecords, len(cases))
		}
	})

	// K: the streaming repertoire's lines that do nothing, or nothing a parent reports, keep the connection
	t.Run("repertoire", func(t *testing.T) {
		k := next()
		got := edgeCase(t, p, k, stream.CapsLive, func(c *stream.Conn) error {
			return lines(c,
				"CHART 'e.k' '' 't' 'u' 'f' 'e.k' line 1 1 '' p m",
				"DIMENSION 'k1' '' absolute 1 1 ''",
				"CLABEL 'k' 'v' 1",
				"CLABEL_COMMIT",
				`LABEL "edge" = 1 "x"`,
				"OVERWRITE labels",
				`FUNCTION GLOBAL "edge-fn" 10 "h" "top" "0x0" 100 3`,
				`FUNCTION_DEL GLOBAL "edge-fn"`,
				"FUNCTION_RESULT_BEGIN 'none' 200 'text/plain' 0",
				"BOGUS_BODY",
				"FUNCTION_RESULT_END",
				"FUNCTION_PROGRESS 'none' 1 2",
				"JSON EDGE_X",
				"BOGUS_JSON",
				"JSON_PAYLOAD_END",
				"BEGIN 'e.k' 0",
				"SET 'k1' = 3",
				"END",
				"",
				"   ",
				"=",
			)
		})
		_ = got
	})

	// R: a CHART_DEFINITION_END with odd retention asks for C's replication request, with C's NOTICE where C writes one
	// (one limiter for the process: one record a second)
	t.Run("replication-requests", func(t *testing.T) {
		b := time.Now().Unix()/60*60 - 3600
		at := func(d int64) string { return strconv.FormatInt(b+d, 10) }
		warm := func(c *stream.Conn, id string) {
			c.Linef("CHART '%s' '' 't' 'u' 'f' '%s' line 1 1 '' p m", id, id)
			c.Linef("DIMENSION 'x' '' absolute 1 1 ''")
			c.Linef("BEGIN2 '%s' 1 %s #", id, at(-3000))
			c.Linef("SET2 'x' 1 1 A")
			c.Linef("END2")
		}
		cases := []struct {
			name   string
			charts []string
			ends   []string // each chart's CHART_DEFINITION_END words, in order
			empty  bool     // the chart has no dimension and no data
		}{
			{"clamp", []string{"e.r1"}, []string{at(-10) + " " + at(5000) + " " + at(40)}, false},
			{"negative-reads-zero", []string{"e.r2a"}, []string{"-60 " + at(0) + " " + at(10)}, false},
			{"negative-first", []string{"e.r2b"}, []string{"18446744073709551556 " + at(0) + " " + at(10)}, false},
			{"first-after-wall", []string{"e.r3"}, []string{at(200) + " " + at(50) + " " + at(100)}, false},
			{"first-after-last", []string{"e.r4"}, []string{at(50) + " " + at(20) + " " + at(100)}, false},
			{"after-past-before", []string{"e.r5"}, []string{at(7200) + " " + at(7300) + " " + at(7400)}, false},
			{"local-newer", []string{"e.r6"}, []string{at(-4000) + " " + at(-3000) + " " + at(0)}, false},
			{"no-dimensions", []string{"e.r7"}, []string{at(-60) + " " + at(0) + " " + at(10)}, true},
			{"no-retention", []string{"e.r8"}, []string{"0 0 " + at(0)}, false},
			{"two-in-a-second", []string{"e.r10a", "e.r10b"},
				[]string{at(200) + " " + at(50) + " " + at(100), at(50) + " " + at(20) + " " + at(100)}, false},
			{"clamp-hides-the-rest", []string{"e.r11"}, []string{at(200) + " " + at(5000) + " " + at(100)}, false},
		}
		t.Run("replication-off", func(t *testing.T) {
			edgeCaseDown(t, p, 900, stream.CapsReplication, false, func(c *stream.Conn, down *[]string) error {
				warm(c, "e.r9")
				c.Linef("CHART 'e.r9' '' 't' 'u' 'f' 'e.r9' line 1 1 '' p m")
				c.Linef("CHART_DEFINITION_END %s %s %s", at(-60), at(0), at(10))
				if err := c.Flush(); err != nil {
					return err
				}
				readReplay(c, down, 1)
				return nil
			})
		})
		notices := 0
		for _, tc := range cases {
			k := next()
			t.Run(tc.name, func(t *testing.T) {
				got, _ := edgeCaseDown(t, p, k, stream.CapsReplication, false, func(c *stream.Conn, down *[]string) error {
					for _, id := range tc.charts {
						if tc.empty {
							c.Linef("CHART '%s' '' 't' 'u' 'f' '%s' line 1 1 '' p m", id, id)
						} else {
							warm(c, id)
						}
					}
					for i, id := range tc.charts {
						c.Linef("CHART '%s' '' 't' 'u' 'f' '%s' line 1 1 '' p m", id, id)
						c.Linef("CHART_DEFINITION_END %s", tc.ends[i])
					}
					if err := c.Flush(); err != nil {
						return err
					}
					readReplay(c, down, len(tc.charts))
					return nil
				})
				notices += strings.Count(strings.Join(got[0], "\n"), "STREAM SND REPLAY ERROR")
			})
		}
		if notices < 6 {
			t.Errorf("the oracle wrote %d replication NOTICEs, fewer than 6", notices)
		}
	})

	// P: a chart's replication answered by hand: RSSTATE moves the last update forward only and only while RSET is
	// enabled, a definition starts a new round only when none runs, an RBEGIN with invalid times disables RSET
	t.Run("replication-answers", func(t *testing.T) {
		b := time.Now().Unix()/60*60 - 3600
		at := func(d int64) string { return strconv.FormatInt(b+d, 10) }
		us := func(d int64) string { return strconv.FormatInt((b+d)*1_000_000, 10) }
		k := next()
		edgeCaseDown(t, p, k, stream.CapsReplication, false, func(c *stream.Conn, down *[]string) error {
			define := func() {
				c.Linef("CHART 'e.p' '' 't' 'u' 'f' 'e.p' line 1 1 '' p m")
				c.Linef("DIMENSION 'x' '' absolute 1 1 ''")
			}
			step := func(ls ...string) error {
				for _, l := range ls {
					c.Linef("%s", l)
				}
				if err := c.Flush(); err != nil {
					return err
				}
				readReplay(c, down, 1)
				return nil
			}
			define()
			c.Linef("BEGIN2 'e.p' 1 %s #", at(-3000))
			c.Linef("SET2 'x' 1 1 A")
			c.Linef("END2")
			// P1: a step, then the last update moved to b-30 and a quarter second
			if err := step("CHART_DEFINITION_END " + at(-130) + " " + at(-70) + " " + at(0)); err != nil {
				return err
			}
			c.Linef("RBEGIN 'e.p'")
			c.Linef("RBEGIN '' %s %s %s", at(-71), at(-70), at(0))
			c.Linef("RSET 'x' 5 A")
			c.Linef("RSSTATE %s %s", strconv.FormatInt((b-30)*1_000_000+250_000, 10), us(-30))
			c.Linef("REND 1 %s %s true %s %s %s", at(-130), at(-70), at(-130), at(-70), at(0))
			// P2: a new round from the last update RSSTATE set; RSSTATE backwards is ignored
			define()
			if err := step("CHART_DEFINITION_END " + at(-130) + " " + at(-10) + " " + at(1)); err != nil {
				return err
			}
			c.Linef("RBEGIN 'e.p'")
			c.Linef("RBEGIN '' %s %s %s", at(-11), at(-10), at(1))
			c.Linef("RSET 'x' 6 A")
			c.Linef("RSSTATE %s %s", us(-500), us(-500))
			c.Linef("REND 1 %s %s true %s %s %s", at(-130), at(-10), at(-30), at(-10), at(1))
			// P3: an RBEGIN without times disables RSET: RSSTATE is ignored
			define()
			if err := step("CHART_DEFINITION_END " + at(-130) + " " + at(-5) + " " + at(2)); err != nil {
				return err
			}
			c.Linef("RBEGIN 'e.p'")
			c.Linef("RSSTATE %s %s", us(100), us(100))
			c.Linef("REND 1 %s %s true %s %s %s", at(-130), at(-5), at(-10), at(-5), at(2))
			// P4: invalid times with the child's clock 0: the parent's clock judges them; RSET refused
			define()
			if err := step("CHART_DEFINITION_END " + at(-130) + " " + at(50) + " " + at(60)); err != nil {
				return err
			}
			c.Linef("RBEGIN 'e.p'")
			c.Linef("RBEGIN '' 100 50 0")
			c.Linef("RSET 'x' 1 A")
			c.Linef("REND 1 %s %s true %s %s %s", at(-130), at(50), at(-10), at(50), at(60))
			// P5: the same definition twice in one burst: one request
			define()
			c.Linef("CHART_DEFINITION_END %s %s %s", at(-130), at(55), at(65))
			define()
			if err := step("CHART_DEFINITION_END " + at(-130) + " " + at(55) + " " + at(65)); err != nil {
				return err
			}
			c.Linef("RBEGIN 'e.p'")
			c.Linef("REND 1 %s %s true %s %s %s", at(-130), at(55), at(50), at(55), at(65))
			// P6: nothing newer than what the parent has
			define()
			return step("CHART_DEFINITION_END " + at(-130) + " " + at(-20) + " " + at(60))
		})
	})

	// F: the first `after` a connection requests sets the completion's start; an empty request resets it
	t.Run("replication-first-time", func(t *testing.T) {
		k := next()
		// one clock for both sides: the requests' times follow the child's
		fn := time.Now().Unix()
		at := func(d int64) string { return strconv.FormatInt(fn+d, 10) }
		edgeCaseDown(t, p, k, stream.CapsReplication, true, func(c *stream.Conn, down *[]string) error {
			for _, id := range []string{"e.fa", "e.fb", "e.fc"} {
				c.Linef("CHART '%s' '' 't' 'u' 'f' '%s' line 1 1 '' p m", id, id)
				c.Linef("DIMENSION 'x' '' absolute 1 1 ''")
				c.Linef("BEGIN2 '%s' 1 %s #", id, at(-5000))
				c.Linef("SET2 'x' 1 1 A")
				c.Linef("END2")
			}
			for _, tc := range []struct{ id, end string }{
				{"e.fa", at(-1000) + " " + at(-100) + " " + at(0)},
				{"e.fb", "0 0 " + at(0)},
				{"e.fc", at(-900) + " " + at(-100) + " " + at(0)},
			} {
				c.Linef("CHART '%s' '' 't' 'u' 'f' '%s' line 1 1 '' p m", tc.id, tc.id)
				c.Linef("CHART_DEFINITION_END %s", tc.end)
			}
			if err := c.Flush(); err != nil {
				return err
			}
			readReplay(c, down, 3)
			c.Linef("RBEGIN 'e.fa'")
			c.Linef("RBEGIN '' %s %s %s", at(-801), at(-800), at(0))
			c.Linef("RSET 'x' 1 A")
			c.Linef("REND 1 %s %s false %s %s %s", at(-1000), at(-100), at(-1000), at(-940), at(0))
			if err := c.Flush(); err != nil {
				return err
			}
			readReplay(c, down, 1)
			// the removal's record carries the completion
			c.Linef("BOGUS")
			return c.Flush()
		})
	})

	// Z: a compressed message that decompresses to more than 16384 bytes ends the connection (C's
	// stream_decompress()); one of exactly 16384 is parsed
	t.Run("decompressed-cap", func(t *testing.T) {
		bogus := []byte("BOGUS\n")
		for _, tc := range []struct {
			name string
			caps uint32
			msg  func(n int) []byte
			text []byte
		}{
			{"zstd", stream.CapsLive | stream.CapZSTD, func(n int) []byte { return stream.ZstdRLE(n, '\n') },
				stream.ZstdRaw(bogus)},
			{"lz4", stream.CapsLive | stream.CapLZ4, func(n int) []byte { return stream.LZ4Run(n, '\n') },
				stream.LZ4Literals(bogus)},
		} {
			// 16384 empty lines, then a refused one: the parser fails on line 16385
			t.Run(tc.name+"/fits", func(t *testing.T) {
				k := next()
				got := edgeCase(t, p, k, tc.caps, func(c *stream.Conn) error {
					if err := c.WriteMessage(tc.msg(16384)); err != nil {
						return err
					}
					return c.WriteMessage(tc.text)
				})
				if !strings.Contains(strings.Join(got[0], "\n"), "failed on line 16385") {
					t.Errorf("the oracle did not parse the 16384 lines:\n%s", strings.Join(got[0], "\n"))
				}
			})
			t.Run(tc.name+"/over", func(t *testing.T) {
				k := next()
				got := edgeCase(t, p, k, tc.caps, func(c *stream.Conn) error { return c.WriteMessage(tc.msg(16385)) })
				if !strings.Contains(strings.Join(got[0], "\n"), "decompressed data is 16385 bytes") {
					t.Errorf("the oracle did not refuse 16385 bytes:\n%s", strings.Join(got[0], "\n"))
				}
			})
		}
	})

	// U-stale (D128.2, D131.3): the receive slot cache is the host's and outlives a connection. The first connection
	// grows it with u.a at slot 1; the second defines u.b and u.c again (the option word clears the accept's
	// obsolete mark) but not u.a, which the accept's obsolete-all unslots. Slot 5 caches u.b and then takes u.c's
	// block (no id check); slot 1, freed, finds u.c by id.
	t.Run("slot-cache-reconnect", func(t *testing.T) {
		k := next()
		at := time.Now().Unix() - 30
		chart := func(slot, id string) []string {
			return []string{fmt.Sprintf("CHART %s'%s' '' 't' 'u' 'f' '%s' line 1 1 'x' p m", slot, id, id),
				"DIMENSION 'd' '' absolute 1 1 ''"}
		}
		block := func(slot int, id string, t, v int64) []string {
			return []string{fmt.Sprintf("BEGIN2 SLOT:%d '%s' 1 %d #", slot, id, t), fmt.Sprintf("SET2 'd' %d %d A", v, v),
				"END2"}
		}
		first := append(append(chart("SLOT:1 ", "u.a"), chart("", "u.b")...), chart("", "u.c")...)
		edgeCaseDown(t, p, k, stream.CapsLive, false, func(c *stream.Conn, _ *[]string) error { return lines(c, first...) })
		second := append(chart("", "u.b"), chart("", "u.c")...)
		second = append(second, block(5, "u.b", at, 20)...)
		second = append(second, block(5, "u.c", at+1, 30)...)
		second = append(second, block(1, "u.c", at+2, 40)...)
		edgeCaseDown(t, p, k, stream.CapsLive, false, func(c *stream.Conn, _ *[]string) error { return lines(c, second...) })
		// each chart's points over the blocks' seconds (the data API, which both agents serve)
		values := map[string][]string{}
		for _, id := range []string{"u.a", "u.b", "u.c"} {
			path := fmt.Sprintf("/host/%s/api/v1/data?chart=%s&after=%d&before=%d&format=json&options=unaligned",
				edgeHost(k).Hostname, id, at-1, at+2)
			var bodies [2][]byte
			for i, side := range p.Each() {
				b, err := rawExchange(side.Daemon.Addr, []byte("GET "+path+" HTTP/1.1\r\n\r\n"), 5*time.Second)
				if err != nil {
					t.Fatal(err)
				}
				_, bodies[i], _ = bytes.Cut(b, []byte("\r\n\r\n"))
			}
			// u.a, obsolete, is no query's chart: C answers with a text, compared as bytes
			var data struct{ Data [][]any }
			if json.Unmarshal(bodies[0], &data) != nil {
				if !bytes.Equal(bodies[0], bodies[1]) {
					t.Errorf("%s:\noracle:    %q\ncandidate: %q", path, bodies[0], bodies[1])
				}
				continue
			}
			diffs, err := p.CompareJSON(path, nil, Rules{})
			if err != nil {
				t.Fatal(err)
			}
			for _, d := range diffs {
				t.Errorf("%s: %s", path, d)
			}
			for _, row := range data.Data {
				if len(row) == 2 && row[1] != nil {
					values[id] = append(values[id], fmt.Sprintf("%v@%v", row[1], int64(row[0].(float64))-at))
				}
			}
			slices.Sort(values[id])
		}
		// C: slot 5 cached u.b, which then took u.c's block; slot 1, freed with u.a's obsolete mark, found u.c
		want := map[string][]string{"u.b": {"20@0", "30@1"}, "u.c": {"40@2"}}
		if !maps.EqualFunc(values, want, slices.Equal) {
			t.Errorf("the oracle's points are %v, not C's %v", values, want)
		}
	})
}

// edgeProxySide is a parent proxying child M to a recording stub that refuses IEEE754 and compression (the proxy
// re-encodes its blocks for the stub), with M connected.
type edgeProxySide struct {
	stub  *stream.Parent
	proxy *daemon.Daemon
	child *stream.Conn
}

func startEdgeProxy(t *testing.T, role Role, bin string, m stream.HostInfo) *edgeProxySide {
	t.Helper()
	stub, err := stream.StartParent(func(r stream.Request) stream.Answer {
		return stream.Answer{Reply: stream.VCaps(r.Caps() &^ (stream.CapsCompression | stream.CapIEEE754)),
			StartStreaming: true}
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { stub.Close() })
	id := proxyIdentity
	d, err := daemon.Start(daemon.Options{Binary: bin, RunDir: runDir(t, role), Identity: &id, StorageTiers: 1,
		StreamSection: "    reconnect delay = 5\n",
		StreamExtra: fmt.Sprintf("\n[%s]\n    proxy enabled = yes\n    proxy destination = %s\n    proxy api key = %s\n",
			m.MachineGUID, stub.Addr(), proxyUpKey)})
	if err != nil {
		t.Fatalf("start %s: %v", role, err)
	}
	t.Cleanup(func() { _ = d.Stop() })
	c, err := stream.Connect(d.Addr, d.StreamKey, m, proxyCLikeCaps|stream.CapClaim)
	if err != nil {
		t.Fatalf("%s: %v", role, err)
	}
	t.Cleanup(func() { _ = c.Close() })
	return &edgeProxySide{stub: stub, proxy: d, child: c}
}

// upstreamLines are the stub's lines from byte `from` that the proxied groups compare.
func upstreamLines(data []byte, from int) []string {
	var out []string
	for _, l := range strings.Split(string(data[min(from, len(data)):]), "\n") {
		for _, p := range []string{"VARIABLE ", "CLAIMED_ID ", "BEGIN2 ", "SET2 ", "END2"} {
			if strings.HasPrefix(l, p) {
				out = append(out, l)
				break
			}
		}
	}
	return out
}

// The proxied groups (W, V, C, S): what a proxy makes of a child's odd lines, in its records and upstream.
func TestStreamParserEdgesProxied(t *testing.T) {
	bins := binaries(t)
	m := stream.HostInfo{Hostname: "parity-edges", MachineGUID: "5a1e0000-0000-4000-8000-0000000003e7"}
	var sides [2]*edgeProxySide
	for i, role := range []Role{"edges-proxy-oracle", "edges-proxy-candidate"} {
		sides[i] = startEdgeProxy(t, role, bins[i], m)
	}
	b := time.Now().Unix()/60*60 - 3600
	at := func(d int64) string { return strconv.FormatInt(b+d, 10) }
	hex := func(v int64) string { return stream.EncodeU64(stream.EncHex, uint64(v)) }
	b64 := func(v int64) string { return stream.EncodeU64(stream.EncBase64, uint64(v)) }
	warm := func(c *stream.Conn, id, slot string) {
		c.Linef("CHART %s'%s' '' 't' 'u' 'f' '%s' line 1 1 '' p m", slot, id, id)
		c.Linef("DIMENSION 'x' '' absolute 1 1 ''")
		c.Linef("BEGIN2 '%s' 1 %s #", id, at(-3000))
		c.Linef("SET2 'x' 1 1 A")
		c.Linef("END2")
	}
	flushes := 0
	// poke collects each chart once more, at b+k: a proxy defines a child's charts upstream at their next collection
	// once its sender is ready
	poke := func(c *stream.Conn, k int64) {
		for _, id := range []string{"e.w", "e.v"} {
			c.Linef("BEGIN2 '%s' 1 %s #", id, at(-2999+k))
			c.Linef("SET2 'x' 1 1 A")
			c.Linef("END2")
		}
		c.Linef("BEGIN2 'e.s1' 1 %s #", at(-2999+k))
		c.Linef("SET2 'a' 1 1 A")
		c.Linef("SET2 'b' 1 1 A")
		c.Linef("END2")
	}
	// group writes a group's lines on each side, then a new chart collected once: its definition sends what the
	// proxy's batch holds ahead of it (with `rends`, it first pokes the charts until that many of them finished
	// their upstream replication)
	group := func(t *testing.T, rends int, blocks bool, write func(c *stream.Conn)) {
		t.Helper()
		var records, up [2][]string
		flushes++
		flush := fmt.Sprintf("e.flush%d", flushes)
		flushDef := regexp.MustCompile(`(?m)^CHART (?:SLOT:\S+ )?"` + regexp.QuoteMeta(flush) + `"`)
		for i, s := range sides {
			from := edgeMark(t, s.proxy)
			// the sender connects after the host's first collection
			start := 0
			if ss := s.stub.Sessions(); len(ss) > 0 {
				start = len(ss[0].Data())
			}
			write(s.child)
			if err := s.child.Flush(); err != nil {
				t.Fatalf("side %d: %v", i, err)
			}
			sess := s.stub.WaitSession(1, 30*time.Second)
			if sess == nil {
				t.Fatalf("side %d: no upstream session", i)
			}
			for k := int64(1); rends > 0 && rendTrueCount(sess.Data()) < rends; k++ {
				if k > 30 {
					t.Fatalf("side %d: %d charts' upstream replication did not end:\n%s", i, rends, sess.Data())
				}
				poke(s.child, k)
				if err := s.child.Flush(); err != nil {
					t.Fatalf("side %d: %v", i, err)
				}
				time.Sleep(time.Second)
			}
			s.child.Linef("CHART '%s' '' 't' 'u' 'f' 'e.flush' line 1 1 '' p m", flush)
			s.child.Linef("DIMENSION 'x' '' absolute 1 1 ''")
			s.child.Linef("BEGIN2 '%s' 1 %s #", flush, at(int64(100+flushes)))
			s.child.Linef("SET2 'x' 1 1 A")
			s.child.Linef("END2")
			if err := s.child.Flush(); err != nil {
				t.Fatalf("side %d: %v", i, err)
			}
			if !sess.WaitData(flushDef.Match, 30*time.Second) {
				t.Fatalf("side %d: the flush chart was not defined upstream", i)
			}
			time.Sleep(300 * time.Millisecond)
			records[i] = edgeRecords(t, s.proxy, from)
			up[i] = upstreamLines(sess.Data(), start)
		}
		t.Logf("oracle:\n%s\nupstream:\n%s", strings.Join(records[0], "\n"), strings.Join(up[0], "\n"))
		diffLines(t, "records", records[0], records[1])
		diffLines(t, "upstream", up[0], up[1])
		if blocks && !slices.ContainsFunc(up[0], func(l string) bool { return strings.HasPrefix(l, "BEGIN2 ") }) {
			t.Errorf("no block reached the oracle's stub")
		}
	}
	// the charts, e.s1's definition with slots past the caps (S1: one WARNING, the others the limiter's)
	t.Run("warm", func(t *testing.T) {
		group(t, 3, false, func(c *stream.Conn) {
			c.Linef("VARIABLE edge_vh = 9")
			for _, id := range []string{"e.w", "e.v"} {
				warm(c, id, "")
			}
			c.Linef("CHART SLOT:1000001 'e.s1' '' 't' 'u' 'f' 'e.s1' line 1 1 '' p m")
			c.Linef("DIMENSION SLOT:65536 'a' '' absolute 1 1 ''")
			c.Linef("DIMENSION 'b' '' absolute 1 1 ''")
			c.Linef("BEGIN2 'e.s1' 1 %s #", at(-3000))
			c.Linef("SET2 'a' 1 1 A")
			c.Linef("SET2 'b' 1 1 A")
			c.Linef("END2")
		})
	})
	// W: BEGIN2's wall clock reaches the parent re-encoded; C reads a `#` wall clock's value as the end time's
	t.Run("begin2-wall-clock", func(t *testing.T) {
		group(t, 0, true, func(c *stream.Conn) {
			for k, wall := range []string{"#", at(9), "#" + b64(b + 9)[1:], hex(b + 9)} {
				c.Linef("BEGIN2 'e.w' 1 %s %s", at(int64(1+k)), wall)
				c.Linef("SET2 'x' %d %d A", k, k)
				c.Linef("END2")
			}
		})
	})
	// V: chart and host variables, their parse errors and their resend rules
	t.Run("variables", func(t *testing.T) {
		group(t, 0, true, func(c *stream.Conn) {
			c.Linef("BEGIN2 'e.v' 1 %s #", at(1))
			c.Linef("SET2 'x' 1 1 A")
			c.Linef("END2")
			c.Linef("VARIABLE edge_v0 = 1")
			c.Linef("VARIABLE HOST edge_v3")
			c.Linef("VARIABLE HOST edge_v4 = 12abc")
			c.Linef("VARIABLE GLOBAL edge_v5 = abc")
			c.Linef("VARIABLE HOST edge_v6 = null")
			c.Linef("VARIABLE LOCAL edge_v7 = 7")
			c.Linef("VARIABLE HOST edge_v4 = 12")
			c.Linef("BEGIN2 'e.v' 1 %s #", at(2))
			c.Linef("SET2 'x' 2 2 A")
			c.Linef("END2")
		})
	})
	// C: claim ids in their accepted forms, and one for another host
	t.Run("claimed-ids", func(t *testing.T) {
		group(t, 0, false, func(c *stream.Conn) {
			claim := "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
			c.Linef("CLAIMED_ID '%s' '%s'", m.MachineGUID, strings.ToUpper(claim))
			c.Linef("CLAIMED_ID '%s' '%s'", m.MachineGUID, strings.ReplaceAll(claim, "-", ""))
			c.Linef("CLAIMED_ID '%s' '%szz'", m.MachineGUID, claim)
			c.Linef("CLAIMED_ID '%s' '%s'", strings.ToUpper(m.MachineGUID), claim)
			c.Linef("CLAIMED_ID '%s' 'NULL'", m.MachineGUID)
		})
	})
	// S: slots past the caps, a slotted and an unslotted dimension, the replication keywords' own limiter
	t.Run("slots", func(t *testing.T) {
		group(t, 0, true, func(c *stream.Conn) {
			c.Linef("BEGIN2 SLOT:#%s 'e.s1' 1 %s #", b64(1000001)[1:], at(1))
			c.Linef("SET2 SLOT:9 'a' 3 3 A")
			c.Linef("SET2 'b' 4 4 A")
			c.Linef("END2")
		})
	})
}
