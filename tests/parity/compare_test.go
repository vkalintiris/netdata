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
