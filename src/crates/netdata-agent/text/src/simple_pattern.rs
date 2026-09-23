//! Simple patterns, mirroring `libnetdata/simple_pattern/simple_pattern.c`.
//!
//! A pattern list is split on separators into words; `!word` is a negative
//! match; `*` is a wildcard anywhere in a word; `\` escapes the next byte
//! (only separators and `\` itself need it: an escaped `*` is still a
//! wildcard, as in C). Words are tried left to right and the first match
//! decides.
//!
//! Faithfully kept C behaviour: only the first segment of a word honours
//! `case_sensitive`; the segments after a `*` always compare
//! case-insensitively, because the C parser only sets the flag on the root
//! node of each word.

use crate::c::{self, eq_ignore_case, find, find_ignore_case};

/// The separators of HTTP API parameters (`SIMPLE_PATTERN_DEFAULT_WEB_SEPARATORS`).
pub const DEFAULT_WEB_SEPARATORS: &[u8] = b",|\t\r\n\x0c\x0b";

/// How a word without wildcards matches (`SIMPLE_PREFIX_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimplePatternMode {
    Exact,
    Prefix,
    Suffix,
    Substring,
}

/// The outcome of a match (`SIMPLE_PATTERN_RESULT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimplePatternResult {
    NotMatched,
    MatchedNegative,
    MatchedPositive,
}

/// The word separators of a pattern list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Separators<'a> {
    /// Whitespace (`" \t\r\n\f\v"`); what C uses for `NULL` or `""`.
    Whitespace,
    /// No separators at all (`SIMPLE_PATTERN_NO_SEPARATORS`).
    None,
    /// These bytes (an empty slice means [`Separators::Whitespace`]).
    Bytes(&'a [u8]),
}

impl Separators<'_> {
    fn table(self) -> [bool; 256] {
        let mut table = [false; 256];
        match self {
            Separators::None => {}
            Separators::Bytes(bytes) if !c::c_str(bytes).is_empty() => {
                for &b in c::c_str(bytes) {
                    table[usize::from(b)] = true;
                }
            }
            Separators::Whitespace | Separators::Bytes(_) => {
                for b in [b' ', b'\t', b'\r', b'\n', 0x0c, 0x0b] {
                    table[usize::from(b)] = true;
                }
            }
        }
        table
    }
}

/// One `*`-delimited segment of a word (a node of the C `child` chain).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Segment {
    /// `None` for an empty segment (C `match == NULL`), which matches anything.
    text: Option<Vec<u8>>,
    mode: SimplePatternMode,
}

impl Segment {
    fn len(&self) -> usize {
        self.text.as_ref().map_or(0, Vec::len)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Word {
    /// The root segment followed by its children.
    segments: Vec<Segment>,
    negative: bool,
    case_sensitive: bool,
}

/// A compiled pattern list (`SIMPLE_PATTERN`). An empty list (C `NULL`)
/// matches nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimplePattern {
    words: Vec<Word>,
}

/// `parse_pattern()`: splits a word at its inner asterisks into segments.
/// Nesting stops at 1000 levels; the rest of the word is then dropped.
fn parse_word(word: &[u8], default_mode: SimplePatternMode, count: usize, out: &mut Vec<Segment>) {
    if count >= 1000 {
        return;
    }

    // skip leading asterisks, then find the next one
    let mut c = word.iter().take_while(|&&b| b == b'*').count();
    while c < word.len() && word[c] != b'*' {
        c += 1;
    }

    let mut child = Vec::new();
    let mut s = word;
    if c < word.len() && c + 1 < word.len() {
        // an asterisk in the middle: the child starts at it, this segment ends with it
        parse_word(&word[c..], default_mode, count + 1, &mut child);
        s = &word[..=c];
    }

    let len = s.len();
    let (text, mode) = if len >= 2 && s[0] == b'*' && s[len - 1] == b'*' {
        (&s[1..len - 1], SimplePatternMode::Substring)
    } else if len >= 1 && s[0] == b'*' {
        (&s[1..], SimplePatternMode::Suffix)
    } else if len >= 1 && s[len - 1] == b'*' {
        (&s[..len - 1], SimplePatternMode::Prefix)
    } else {
        (s, default_mode)
    };

    out.push(if text.is_empty() {
        Segment {
            text: None,
            mode: SimplePatternMode::Substring,
        }
    } else {
        Segment {
            text: Some(text.to_vec()),
            mode,
        }
    });
    out.append(&mut child);
}

/// The `wildcarded` output buffer of the C matcher: `size` bytes including
/// the terminator; pieces are appended while they fit.
struct Wildcarded {
    out: Vec<u8>,
    remaining: usize,
}

impl Wildcarded {
    /// `add_wildcarded()`.
    fn add(&mut self, matched: &[u8]) {
        if self.remaining == 0 || matched.is_empty() {
            return;
        }
        let len = matched.len().min(self.remaining - 1);
        if len > 0 {
            self.out.extend_from_slice(&matched[..len]);
            self.remaining -= len;
        }
    }
}

/// `match_pattern()` over the rest of the C string `str`.
fn match_word(word: &Word, s: &[u8], wildcarded: &mut Wildcarded) -> bool {
    let mut str = s;
    for (index, m) in word.segments.iter().enumerate() {
        // only the root node carries the case-sensitivity flag in C
        let case_sensitive = index == 0 && word.case_sensitive;
        let text = m.text.as_deref().unwrap_or_default();
        let len = str.len();
        if m.len() > len {
            return false;
        }
        let has_child = index + 1 < word.segments.len();

        match m.mode {
            SimplePatternMode::Exact => {
                let equal = if case_sensitive {
                    str == text
                } else {
                    eq_ignore_case(str, text)
                };
                return equal && !has_child;
            }
            SimplePatternMode::Substring => {
                if m.len() == 0 {
                    return true;
                }
                let found = if case_sensitive {
                    find(str, text)
                } else {
                    find_ignore_case(str, text)
                };
                let Some(pos) = found else {
                    return false;
                };
                wildcarded.add(&str[..pos]);
                if !has_child {
                    wildcarded.add(&str[pos + m.len()..]);
                    return true;
                }
                str = &str[pos + m.len()..];
            }
            SimplePatternMode::Prefix => {
                let head = &str[..m.len()];
                let equal = if case_sensitive {
                    head == text
                } else {
                    eq_ignore_case(head, text)
                };
                if !equal {
                    return false;
                }
                if !has_child {
                    wildcarded.add(&str[m.len()..]);
                    return true;
                }
                str = &str[m.len()..];
            }
            SimplePatternMode::Suffix => {
                let tail = &str[len - m.len()..];
                let equal = if case_sensitive {
                    tail == text
                } else {
                    eq_ignore_case(tail, text)
                };
                if !equal {
                    return false;
                }
                wildcarded.add(&str[..len - m.len()]);
                return !has_child;
            }
        }
    }
    false
}

impl SimplePattern {
    /// `simple_pattern_create()`.
    pub fn new(
        list: &[u8],
        separators: Separators,
        default_mode: SimplePatternMode,
        case_sensitive: bool,
    ) -> Self {
        let list = c::c_str(list);
        let is_separator = separators.table();
        let mut words = Vec::new();
        let mut i = 0;

        while i < list.len() {
            while i < list.len() && is_separator[usize::from(list[i])] {
                i += 1;
            }
            let mut negative = false;
            if i < list.len() && list[i] == b'!' {
                negative = true;
                i += 1;
            }
            if i >= list.len() {
                break;
            }

            let mut word = Vec::new();
            let mut escape = false;
            while i < list.len() {
                let ch = list[i];
                i += 1;
                if ch == b'\\' && !escape {
                    escape = true;
                    continue;
                }
                if is_separator[usize::from(ch)] && !escape {
                    break;
                }
                word.push(ch);
                escape = false;
            }
            if word.is_empty() {
                continue;
            }

            let mut segments = Vec::new();
            parse_word(&word, default_mode, 0, &mut segments);
            if default_mode == SimplePatternMode::Substring {
                segments[0].mode = SimplePatternMode::Substring;
                if let Some(last) = segments.last_mut() {
                    last.mode = SimplePatternMode::Substring;
                }
            }
            words.push(Word {
                segments,
                negative,
                case_sensitive,
            });
        }

        SimplePattern { words }
    }

    /// `string_to_simple_pattern()`: `None` for empty text or a lone `*`.
    pub fn from_web(text: &[u8]) -> Option<Self> {
        is_valid_sp(text).then(|| {
            Self::new(
                text,
                Separators::Bytes(DEFAULT_WEB_SEPARATORS),
                SimplePatternMode::Exact,
                true,
            )
        })
    }

    /// `string_to_simple_pattern_nocase()`.
    pub fn from_web_nocase(text: &[u8]) -> Option<Self> {
        is_valid_sp(text).then(|| {
            Self::new(
                text,
                Separators::Bytes(DEFAULT_WEB_SEPARATORS),
                SimplePatternMode::Exact,
                false,
            )
        })
    }

    /// `string_to_simple_pattern_nocase_substring()`.
    pub fn from_web_nocase_substring(text: &[u8]) -> Option<Self> {
        is_valid_sp(text).then(|| {
            Self::new(
                text,
                Separators::Bytes(DEFAULT_WEB_SEPARATORS),
                SimplePatternMode::Substring,
                false,
            )
        })
    }

    /// `true` when the list has no words (C returns `NULL`).
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// `simple_pattern_matches()`: a positive match.
    pub fn matches(&self, s: &[u8]) -> bool {
        self.matches_extract(s, 0).0 == SimplePatternResult::MatchedPositive
    }

    /// `simple_pattern_matches_extract()`: the result, plus the bytes matched
    /// by the wildcards of the last word tried (as left in a C buffer of
    /// `wildcarded_size` bytes, i.e. at most `wildcarded_size - 1` bytes).
    ///
    /// C leaves the caller's buffer untouched for an empty `s` or an empty
    /// list; this returns an empty extraction then.
    pub fn matches_extract(
        &self,
        s: &[u8],
        wildcarded_size: usize,
    ) -> (SimplePatternResult, Vec<u8>) {
        let s = c::c_str(s);
        let mut wildcarded = Wildcarded {
            out: Vec::new(),
            remaining: 0,
        };
        if s.is_empty() {
            return (SimplePatternResult::NotMatched, wildcarded.out);
        }
        for word in &self.words {
            wildcarded = Wildcarded {
                out: Vec::new(),
                remaining: wildcarded_size,
            };
            if match_word(word, s, &mut wildcarded) {
                let result = if word.negative {
                    SimplePatternResult::MatchedNegative
                } else {
                    SimplePatternResult::MatchedPositive
                };
                return (result, wildcarded.out);
            }
        }
        (SimplePatternResult::NotMatched, wildcarded.out)
    }

    /// `simple_pattern_is_potential_name()`: whether the list may match a DNS
    /// name, as opposed to only IPv4/IPv6 addresses.
    pub fn is_potential_name(&self) -> bool {
        let (mut alpha, mut colon, mut wildcards) = (false, false, false);
        for word in &self.words {
            if word.segments[0].text.is_some() {
                for segment in &word.segments {
                    let Some(text) = &segment.text else {
                        continue;
                    };
                    if segment.mode == SimplePatternMode::Exact && text.as_slice() == b"localhost" {
                        continue;
                    }
                    alpha |= text.iter().any(u8::is_ascii_alphabetic);
                    colon |= text.contains(&b':');
                    wildcards |= segment.mode != SimplePatternMode::Exact;
                }
            }
            wildcards |= word.segments[0].mode != SimplePatternMode::Exact;
        }
        (alpha || wildcards) && !colon
    }

    /// `simple_pattern_iterate()`: the first segment of every word (`None`
    /// for an empty segment).
    pub fn words(&self) -> impl Iterator<Item = Option<&[u8]>> {
        self.words.iter().map(|w| w.segments[0].text.as_deref())
    }
}

/// `is_valid_sp()`: non-empty and not a lone `*`.
fn is_valid_sp(text: &[u8]) -> bool {
    let text = c::c_str(text);
    !text.is_empty() && text != b"*"
}

/// `simple_pattern_contains_wildcards()`: a leading `!`, or an unescaped
/// separator or `*`.
pub fn contains_wildcards(text: &[u8], separators: Separators) -> bool {
    let text = c::c_str(text);
    if text.is_empty() {
        return false;
    }
    if text[0] == b'!' {
        return true;
    }
    let is_separator = separators.table();
    let mut escape = false;
    for &ch in text {
        if ch == b'\\' && !escape {
            escape = true;
            continue;
        }
        if !escape && (is_separator[usize::from(ch)] || ch == b'*') {
            return true;
        }
        escape = false;
    }
    false
}
