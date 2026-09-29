// SPDX-License-Identifier: GPL-3.0-or-later

package stream

import (
	"fmt"
	"math"
	"strconv"
	"strings"
	"time"
)

// The fake child's richer lines, for a proxy's checks: the number encodings, slots, the host's metadata, and a
// background reader that answers the proxy's replication requests while the caller writes.

// Encoding is how a line's numbers are written.
type Encoding int

const (
	EncDecimal Encoding = iota
	EncHex              // `0x` and uppercase digits; doubles `%` and the hex of their bits
	EncBase64           // `#` and base64 digits; doubles `@` and the base64 of their bits
)

// base64Digits is the digit alphabet of C's `print_uint64_base64_reversed()`, most significant digit first.
const base64Digits = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"

// EncodingFor is the integer and double encodings a C sender picks for `caps`: base64 for both with IEEE754, else
// hex integers and decimal doubles (command-begin-set-end-v2.c:11-13).
func EncodingFor(caps uint32) (ints, doubles Encoding) {
	if caps&CapIEEE754 != 0 {
		return EncBase64, EncBase64
	}
	return EncHex, EncDecimal
}

func digits(v uint64, base uint64, alphabet string) string {
	var b []byte
	for {
		b = append(b, alphabet[v%base])
		v /= base
		if v == 0 {
			break
		}
	}
	for i, j := 0, len(b)-1; i < j; i, j = i+1, j-1 {
		b[i], b[j] = b[j], b[i]
	}
	return string(b)
}

// EncodeU64 writes v as C's `buffer_print_uint64_encoded()`.
func EncodeU64(e Encoding, v uint64) string {
	switch e {
	case EncHex:
		return "0x" + digits(v, 16, "0123456789ABCDEF")
	case EncBase64:
		return "#" + digits(v, 64, base64Digits)
	}
	return strconv.FormatUint(v, 10)
}

// EncodeI64 writes v as C's `buffer_print_int64_encoded()`: a negative one is `-` and its magnitude's encoding.
func EncodeI64(e Encoding, v int64) string {
	if v < 0 && e != EncDecimal {
		return "-" + EncodeU64(e, uint64(0)-uint64(v))
	}
	if e == EncDecimal {
		return strconv.FormatInt(v, 10)
	}
	return EncodeU64(e, uint64(v))
}

// EncodeF64 writes v as C's `buffer_print_netdata_double_encoded()`: its bits in hex or base64, or its decimal
// text (`NAN` for a NaN, which the parser reads).
func EncodeF64(e Encoding, v float64) string {
	switch e {
	case EncHex:
		return "%" + digits(math.Float64bits(v), 16, "0123456789ABCDEF")
	case EncBase64:
		return "@" + digits(math.Float64bits(v), 64, base64Digits)
	}
	if math.IsNaN(v) {
		return "NAN"
	}
	return strconv.FormatFloat(v, 'f', -1, 64)
}

// slotted is a line's `SLOT:<slot> ` prefix, none without a slot.
func slotted(slot string) string {
	if slot == "" {
		return ""
	}
	return "SLOT:" + slot + " "
}

// DimensionWith buffers a DIMENSION line with a slot (none when empty) and its options (`type=float`, `hidden`, ...).
func (c *Conn) DimensionWith(slot, id, name, algorithm string, mul, div int, options string) {
	if algorithm == "" {
		algorithm = "absolute"
	}
	if mul == 0 {
		mul = 1
	}
	if div == 0 {
		div = 1
	}
	c.Linef("DIMENSION %s%s %s %s %d %d %s", slotted(slot), qw(id), qw(name), algorithm, mul, div, qw(options))
}

// Begin2Raw buffers a BEGIN2 line whose words are written as given (encoded numbers, `#`).
func (c *Conn) Begin2Raw(slot, chartID, updateEvery, endTime, wallClock string) {
	c.Linef("BEGIN2 %s%s %s %s %s", slotted(slot), qw(chartID), updateEvery, endTime, wallClock)
}

// Set2Raw buffers a SET2 line whose words are written as given (`#`, `NAN`, `@…`, flags such as `RA` or an empty
// quoted word).
func (c *Conn) Set2Raw(slot, dimID, collected, value, flags string) {
	c.Linef("SET2 %s%s %s %s %s", slotted(slot), qw(dimID), collected, value, flags)
}

// HostLabel buffers one host label as C's sender writes it (`LABEL "name" = <source> "value"`).
func (c *Conn) HostLabel(name, value string, source int) {
	c.Linef(`LABEL "%s" = %d "%s"`, name, source, value)
}

// OverwriteLabels applies the buffered host labels.
func (c *Conn) OverwriteLabels() {
	c.Linef("OVERWRITE labels")
}

// ClaimedID buffers the host's claim id line (`NULL` for none).
func (c *Conn) ClaimedID(machineGUID, claimID string) {
	c.Linef("CLAIMED_ID '%s' '%s'", machineGUID, claimID)
}

// Variable buffers a VARIABLE line: `scope` is HOST or CHART (the current chart).
func (c *Conn) Variable(scope, name, value string) {
	c.Linef("VARIABLE %s %s = %s", scope, name, value)
}

// StreamPath buffers a JSON STREAM_PATH payload.
func (c *Conn) StreamPath(json string) {
	c.Linef("JSON STREAM_PATH\n%s\nJSON_PAYLOAD_END", json)
}

// FunctionGlobal buffers a host function's registration (C's parser order: name, timeout, help, tags, access,
// priority, version; pluginsd_functions.c:502-518).
func (c *Conn) FunctionGlobal(name, help string) {
	c.Linef(`FUNCTION GLOBAL "%s" 10 "%s" "top" "0x0" 100 3`, name, help)
}

// DownLine is one line the parent sent down, and when it came.
type DownLine struct {
	At   time.Time
	Line string
}

// Serve reads the parent's lines in the background until the connection ends: each is kept for Downstream, and a
// REPLAY_CHART for a chart of `charts` is answered from `handler` as ServeReplication does (a chart it does not know
// gets no answer). From here on the caller writes only inside Burst, and neither ReadLine nor ServeReplication runs.
func (c *Conn) Serve(charts map[string]ReplayChart, childNow int64, handler ReplayHandler) {
	c.mu.Lock()
	c.granted = map[string]bool{}
	c.serving.Store(true)
	c.mu.Unlock()
	go func() {
		for {
			line, err := c.r.ReadString('\n')
			if err != nil {
				return
			}
			line = strings.TrimRight(line, "\r\n")
			c.downMu.Lock()
			c.down = append(c.down, DownLine{At: time.Now(), Line: line})
			c.downMu.Unlock()
			// the parent quotes the chart id and the boolean with double quotes
			words := strings.Fields(strings.ReplaceAll(line, `"`, " "))
			if len(words) < 5 || words[0] != "REPLAY_CHART" {
				continue
			}
			ret, known := charts[words[1]]
			after, err1 := strconv.ParseInt(words[3], 10, 64)
			before, err2 := strconv.ParseInt(words[4], 10, 64)
			if !known || err1 != nil || err2 != nil {
				continue
			}
			wantStream := words[2] == "true"
			c.Burst(func() { c.writeReplayAnswer(words[1], ret, wantStream, after, before, childNow, handler) })
			if wantStream {
				c.mu.Lock()
				c.granted[words[1]] = true
				c.mu.Unlock()
			}
		}
	}()
}

// Burst writes `write`'s lines and flushes them as one write, never interleaved with a replication answer.
func (c *Conn) Burst(write func()) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.inBurst.Store(true)
	write()
	c.inBurst.Store(false)
	return c.Flush()
}

// WaitGranted waits until each chart was answered with streaming on.
func (c *Conn) WaitGranted(charts []string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	for {
		c.mu.Lock()
		missing := ""
		for _, ch := range charts {
			if !c.granted[ch] {
				missing = ch
				break
			}
		}
		c.mu.Unlock()
		if missing == "" {
			return nil
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("stream: %q not granted streaming within %v", missing, timeout)
		}
		time.Sleep(20 * time.Millisecond)
	}
}

// Downstream is every line the parent sent so far, in order, a JSON payload joined into one line (its lines
// separated by `\n`).
func (c *Conn) Downstream() []DownLine {
	c.downMu.Lock()
	defer c.downMu.Unlock()
	var out []DownLine
	var payload *DownLine
	for _, d := range c.down {
		switch {
		case payload != nil && d.Line == "JSON_PAYLOAD_END":
			out = append(out, *payload)
			payload = nil
		case payload != nil:
			payload.Line += "\n" + d.Line
		case strings.HasPrefix(d.Line, "JSON "):
			payload = &DownLine{At: d.At, Line: d.Line}
		default:
			out = append(out, d)
		}
	}
	return out
}
