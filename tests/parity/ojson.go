// SPDX-License-Identifier: GPL-3.0-or-later

package parity

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"
)

// Value is a decoded JSON value that keeps what a byte-identical
// implementation must reproduce and encoding/json discards: object member
// order, duplicate keys and the exact text of numbers.
//
// Exactly one of the fields is meaningful, selected by Kind.
type Value struct {
	Kind    Kind
	Text    string   // KindNumber: literal text; KindString: decoded string
	Bool    bool     // KindBool
	Members []Member // KindObject, in document order
	Items   []Value  // KindArray
}

// Member is one object member.
type Member struct {
	Key   string
	Value Value
}

// Kind is the JSON type of a Value.
type Kind int

const (
	KindNull Kind = iota
	KindBool
	KindNumber
	KindString
	KindArray
	KindObject
	// KindMasked replaces a value a check declared nondeterministic; Text
	// names the mask so reports show what was hidden.
	KindMasked
)

// ParseJSON decodes one JSON document.
func ParseJSON(data []byte) (Value, error) {
	dec := json.NewDecoder(bytes.NewReader(data))
	dec.UseNumber()
	v, err := decodeValue(dec)
	if err != nil {
		return Value{}, err
	}
	if _, err := dec.Token(); !errors.Is(err, io.EOF) {
		return Value{}, fmt.Errorf("parity: trailing data after the JSON document")
	}
	return v, nil
}

func decodeValue(dec *json.Decoder) (Value, error) {
	tok, err := dec.Token()
	if err != nil {
		return Value{}, err
	}
	switch t := tok.(type) {
	case nil:
		return Value{Kind: KindNull}, nil
	case bool:
		return Value{Kind: KindBool, Bool: t}, nil
	case json.Number:
		return Value{Kind: KindNumber, Text: t.String()}, nil
	case string:
		return Value{Kind: KindString, Text: t}, nil
	case json.Delim:
		switch t {
		case '{':
			v := Value{Kind: KindObject}
			for dec.More() {
				keyTok, err := dec.Token()
				if err != nil {
					return Value{}, err
				}
				key, ok := keyTok.(string)
				if !ok {
					return Value{}, fmt.Errorf("parity: object key is %T", keyTok)
				}
				member, err := decodeValue(dec)
				if err != nil {
					return Value{}, err
				}
				v.Members = append(v.Members, Member{Key: key, Value: member})
			}
			_, err := dec.Token() // '}'
			return v, err
		case '[':
			v := Value{Kind: KindArray}
			for dec.More() {
				item, err := decodeValue(dec)
				if err != nil {
					return Value{}, err
				}
				v.Items = append(v.Items, item)
			}
			_, err := dec.Token() // ']'
			return v, err
		}
	}
	return Value{}, fmt.Errorf("parity: unexpected JSON token %v", tok)
}

// String renders a Value compactly for reports.
func (v Value) String() string {
	var b strings.Builder
	v.write(&b)
	return b.String()
}

func (v Value) write(b *strings.Builder) {
	switch v.Kind {
	case KindNull:
		b.WriteString("null")
	case KindBool:
		fmt.Fprintf(b, "%t", v.Bool)
	case KindNumber:
		b.WriteString(v.Text)
	case KindString:
		enc, _ := json.Marshal(v.Text)
		b.Write(enc)
	case KindMasked:
		fmt.Fprintf(b, "<masked:%s>", v.Text)
	case KindArray:
		b.WriteByte('[')
		for i, item := range v.Items {
			if i > 0 {
				b.WriteByte(',')
			}
			item.write(b)
		}
		b.WriteByte(']')
	case KindObject:
		b.WriteByte('{')
		for i, m := range v.Members {
			if i > 0 {
				b.WriteByte(',')
			}
			enc, _ := json.Marshal(m.Key)
			b.Write(enc)
			b.WriteByte(':')
			m.Value.write(b)
		}
		b.WriteByte('}')
	}
}
