// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"reflect"
	"testing"
)

func TestParseJSONKeepsOrderDuplicatesAndNumberText(t *testing.T) {
	v, err := ParseJSON([]byte(`{"b":1.50,"a":[true,null,"x"],"b":-0}`))
	if err != nil {
		t.Fatal(err)
	}
	if got, want := v.String(), `{"b":1.50,"a":[true,null,"x"],"b":-0}`; got != want {
		t.Fatalf("round trip %s, want %s", got, want)
	}
	if _, err := ParseJSON([]byte(`{} {}`)); err == nil {
		t.Fatal("trailing document accepted")
	}
}

func TestCompare(t *testing.T) {
	cases := map[string]struct {
		oracle, candidate string
		masks             []Mask
		unordered         []string
		want              []Difference
	}{
		"identical": {
			oracle: `{"a":1,"b":[1,2]}`, candidate: `{"a":1,"b":[1,2]}`,
		},
		"number text differs even when numerically equal": {
			oracle: `{"a":1.0}`, candidate: `{"a":1}`,
			want: []Difference{{Path: "$.a", Oracle: "1.0", Candidate: "1"}},
		},
		"member order is compared": {
			oracle: `{"a":1,"b":2}`, candidate: `{"b":2,"a":1}`,
			want: []Difference{{Path: "$.<members>", Oracle: "[a b]", Candidate: "[b a]"}},
		},
		"array length and kind": {
			oracle: `[1,"x"]`, candidate: `[1,2,3]`,
			want: []Difference{
				{Path: "$[1]", Oracle: `"x"`, Candidate: "2"},
				{Path: "$.length", Oracle: "2", Candidate: "3"},
			},
		},
		"masks hide values on both sides": {
			oracle: `{"now":1,"view":{"t":[5,6]}}`, candidate: `{"now":2,"view":{"t":[7,6]}}`,
			masks: []Mask{{Pattern: "now", Reason: "clock"}, {Pattern: "**.t.[]", Reason: "clock"}},
		},
		"unordered objects compare as sets": {
			oracle: `{"labels":{"a":"1","b":"2"}}`, candidate: `{"labels":{"b":"2","a":"1"}}`,
			unordered: []string{"labels"},
		},
		"unordered still compares values": {
			oracle: `{"labels":{"a":"1","b":"2"}}`, candidate: `{"labels":{"b":"3","a":"1"}}`,
			unordered: []string{"labels"},
			want:      []Difference{{Path: "$.labels.b", Oracle: `"2"`, Candidate: `"3"`}},
		},
		"unordered arrays compare as multisets": {
			oracle:    `{"summary":{"labels":[{"id":"k","vl":[1]},{"id":"_p","vl":[2]}]},"result":{"labels":["time","a"]}}`,
			candidate: `{"summary":{"labels":[{"id":"_p","vl":[2]},{"id":"k","vl":[1]}]},"result":{"labels":["time","a"]}}`,
			unordered: []string{"summary.labels[]"},
		},
		"unordered arrays still compare items": {
			oracle:    `{"summary":{"labels":[{"id":"k","vl":[1]},{"id":"_p","vl":[2]}]}}`,
			candidate: `{"summary":{"labels":[{"id":"_p","vl":[3]},{"id":"k","vl":[1]}]}}`,
			unordered: []string{"summary.labels[]"},
			want:      []Difference{{Path: "$.summary.labels[0].vl[0]", Oracle: "2", Candidate: "3"}},
		},
		"other arrays keep their order": {
			oracle:    `{"summary":{"labels":[]},"result":{"labels":["time","a","b"]}}`,
			candidate: `{"summary":{"labels":[]},"result":{"labels":["time","b","a"]}}`,
			unordered: []string{"summary.labels[]"},
			want: []Difference{
				{Path: "$.result.labels[1]", Oracle: `"a"`, Candidate: `"b"`},
				{Path: "$.result.labels[2]", Oracle: `"b"`, Candidate: `"a"`},
			},
		},
		"a mask does not hide siblings": {
			oracle: `{"now":1,"x":1}`, candidate: `{"now":2,"x":2}`,
			masks: []Mask{{Pattern: "now", Reason: "clock"}},
			want:  []Difference{{Path: "$.x", Oracle: "1", Candidate: "2"}},
		},
	}
	for name, tc := range cases {
		t.Run(name, func(t *testing.T) {
			a, err := ParseJSON([]byte(tc.oracle))
			if err != nil {
				t.Fatal(err)
			}
			b, err := ParseJSON([]byte(tc.candidate))
			if err != nil {
				t.Fatal(err)
			}
			got := Compare(ApplyMasks(a, tc.masks), ApplyMasks(b, tc.masks), tc.unordered...)
			if !reflect.DeepEqual(got, tc.want) {
				t.Fatalf("differences %v, want %v", got, tc.want)
			}
		})
	}
}

func TestMatchPath(t *testing.T) {
	cases := map[string]struct {
		pattern, path []string
		want          bool
	}{
		"literal":             {[]string{"a", "b"}, []string{"a", "b"}, true},
		"star matches index":  {[]string{"a", "*"}, []string{"a", "[0]"}, true},
		"index only on index": {[]string{"[]"}, []string{"key"}, false},
		"double star empty":   {[]string{"**", "a"}, []string{"a"}, true},
		"double star deep":    {[]string{"**", "c"}, []string{"a", "[1]", "c"}, true},
		"prefix is not match": {[]string{"a"}, []string{"a", "b"}, false},
	}
	for name, tc := range cases {
		t.Run(name, func(t *testing.T) {
			if got := matchPath(tc.pattern, tc.path); got != tc.want {
				t.Fatalf("matchPath(%v, %v) = %v", tc.pattern, tc.path, got)
			}
		})
	}
}

// TestLabelOrderOnly: answers equal but for C's label order pass; a label value, a member order elsewhere or a header
// still fail.
func TestLabelOrderOnly(t *testing.T) {
	head := "HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n"
	body := `{"api":2,"summary":{"labels":[{"id":"k"},{"id":"_collect_plugin"}]},"instances":[{"labels":{"k":"v1","_p":"x"}}]}`
	cases := map[string]struct {
		candidate string
		want      bool
	}{
		"label keys swapped": {head + `{"api":2,"summary":{"labels":[{"id":"_collect_plugin"},{"id":"k"}]},` +
			`"instances":[{"labels":{"_p":"x","k":"v1"}}]}`, true},
		"a label value changed": {head + `{"api":2,"summary":{"labels":[{"id":"_collect_plugin"},{"id":"k"}]},` +
			`"instances":[{"labels":{"_p":"x","k":"v2"}}]}`, false},
		"members reordered": {head + `{"summary":{"labels":[{"id":"k"},{"id":"_collect_plugin"}]},"api":2,` +
			`"instances":[{"labels":{"k":"v1","_p":"x"}}]}`, false},
		"a header differs": {"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n" + body, false},
		"not JSON":         {head + "cb(" + body + ")", false},
	}
	for name, tc := range cases {
		t.Run(name, func(t *testing.T) {
			if got := labelOrderOnly([]byte(head+body), []byte(tc.candidate)); got != tc.want {
				t.Fatalf("labelOrderOnly %v, want %v", got, tc.want)
			}
		})
	}
}
