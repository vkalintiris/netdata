// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"fmt"
	"slices"
	"strconv"
	"strings"
)

// Mask hides a value both implementations legitimately produce differently
// (wall-clock times, random identifiers, resource figures). Every mask is a
// recorded exception: reports list the masked paths, so a mask cannot silently
// hide a regression elsewhere.
//
// Pattern is a dotted path. A segment matches an object key literally, "*"
// matches any one key or array index, "[]" matches any array index and "**"
// matches any number of segments (including none).
type Mask struct {
	Pattern string
	Reason  string
}

// ApplyMasks returns v with every value matched by a mask replaced by a
// KindMasked placeholder naming the mask's reason.
func ApplyMasks(v Value, masks []Mask) Value {
	compiled := make([][]string, len(masks))
	for i, m := range masks {
		compiled[i] = strings.Split(m.Pattern, ".")
	}
	return applyMasks(v, nil, masks, compiled)
}

func applyMasks(v Value, path []string, masks []Mask, compiled [][]string) Value {
	for i, pattern := range compiled {
		if matchPath(pattern, path) {
			return Value{Kind: KindMasked, Text: masks[i].Reason}
		}
	}
	switch v.Kind {
	case KindObject:
		out := Value{Kind: KindObject, Members: make([]Member, len(v.Members))}
		for i, m := range v.Members {
			out.Members[i] = Member{Key: m.Key, Value: applyMasks(m.Value, append(path, m.Key), masks, compiled)}
		}
		return out
	case KindArray:
		out := Value{Kind: KindArray, Items: make([]Value, len(v.Items))}
		for i, item := range v.Items {
			out.Items[i] = applyMasks(item, append(path, "["+strconv.Itoa(i)+"]"), masks, compiled)
		}
		return out
	}
	return v
}

func matchPath(pattern, path []string) bool {
	if len(pattern) == 0 {
		return len(path) == 0
	}
	switch head := pattern[0]; head {
	case "**":
		for skip := 0; skip <= len(path); skip++ {
			if matchPath(pattern[1:], path[skip:]) {
				return true
			}
		}
		return false
	default:
		if len(path) == 0 {
			return false
		}
		seg := path[0]
		isIndex := strings.HasPrefix(seg, "[")
		ok := head == "*" || (head == "[]" && isIndex) || head == seg
		return ok && matchPath(pattern[1:], path[1:])
	}
}

// Difference is one divergence between the oracle and the candidate.
type Difference struct {
	Path      string
	Oracle    string
	Candidate string
}

func (d Difference) String() string {
	return fmt.Sprintf("%s: oracle %s, candidate %s", d.Path, d.Oracle, d.Candidate)
}

// Compare reports every divergence between two (already masked) values.
// Object member ORDER is compared, except for objects whose path matches one
// of the unordered patterns (same syntax as Mask patterns): the C agent emits
// most members in a fixed order that clients may depend on, but a few objects
// (e.g. host labels added by concurrent startup threads) have no stable order.
func Compare(oracle, candidate Value, unordered ...string) []Difference {
	c := comparer{}
	for _, u := range unordered {
		c.unordered = append(c.unordered, strings.Split(u, "."))
	}
	c.compareAt("$", nil, oracle, candidate)
	return c.out
}

type comparer struct {
	unordered [][]string
	out       []Difference
}

func (c *comparer) isUnordered(segs []string) bool {
	for _, u := range c.unordered {
		if matchPath(u, segs) {
			return true
		}
	}
	return false
}

func (c *comparer) compareAt(path string, segs []string, a, b Value) {
	out := &c.out
	if a.Kind != b.Kind {
		*out = append(*out, Difference{Path: path, Oracle: a.String(), Candidate: b.String()})
		return
	}
	switch a.Kind {
	case KindNull:
	case KindBool:
		if a.Bool != b.Bool {
			*out = append(*out, Difference{Path: path, Oracle: a.String(), Candidate: b.String()})
		}
	case KindNumber, KindString, KindMasked:
		if a.Text != b.Text {
			*out = append(*out, Difference{Path: path, Oracle: a.String(), Candidate: b.String()})
		}
	case KindArray:
		n := min(len(a.Items), len(b.Items))
		for i := range n {
			idx := "[" + strconv.Itoa(i) + "]"
			c.compareAt(path+idx, append(segs, idx), a.Items[i], b.Items[i])
		}
		if len(a.Items) != len(b.Items) {
			*out = append(*out, Difference{
				Path:      path + ".length",
				Oracle:    strconv.Itoa(len(a.Items)),
				Candidate: strconv.Itoa(len(b.Items)),
			})
		}
	case KindObject:
		ka, kb := memberKeys(a), memberKeys(b)
		if c.isUnordered(segs) {
			ka, kb = slices.Sorted(slices.Values(ka)), slices.Sorted(slices.Values(kb))
		}
		if strings.Join(ka, "\x00") != strings.Join(kb, "\x00") {
			*out = append(*out, Difference{
				Path:      path + ".<members>",
				Oracle:    "[" + strings.Join(ka, " ") + "]",
				Candidate: "[" + strings.Join(kb, " ") + "]",
			})
		}
		bByKey := make(map[string]Value, len(b.Members))
		for _, m := range b.Members {
			if _, seen := bByKey[m.Key]; !seen {
				bByKey[m.Key] = m.Value
			}
		}
		seen := make(map[string]bool, len(a.Members))
		for _, m := range a.Members {
			if seen[m.Key] {
				continue
			}
			seen[m.Key] = true
			if bv, ok := bByKey[m.Key]; ok {
				c.compareAt(path+"."+m.Key, append(segs, m.Key), m.Value, bv)
			}
		}
	}
}

func memberKeys(v Value) []string {
	keys := make([]string, len(v.Members))
	for i, m := range v.Members {
		keys[i] = m.Key
	}
	return keys
}
