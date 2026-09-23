//! `quoted_strings_splitter()` from `libnetdata/line_splitter/line_splitter.h`: the word splitter behind the
//! plugins.d and streaming protocol, netdata.conf lists, query group-by labels and dyncfg ids.
//!
//! Ported literally, including what C does not do: a backslash does not escape (it stays in the word and only keeps
//! the next byte from being a quote or separator), a closing quote is overwritten with a space (a separator for the
//! whitespace, plugins.d and config maps, a literal space for the others), and words past `max_words` are dropped.

use crate::c::c_str;

/// The separator maps (`isspace_map_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Separators {
    /// `' '`, `\t`, `\r`, `\n`, `\f`, `\v`.
    Whitespace,
    /// Whitespace plus `=`.
    Pluginsd,
    /// Whitespace plus `,`.
    Config,
    /// `,` and `|`.
    GroupByLabel,
    /// `:`.
    DyncfgId,
}

impl Separators {
    fn contains(self, c: u8) -> bool {
        let whitespace = matches!(c, b' ' | b'\t' | b'\r' | b'\n' | 0x0c | 0x0b);
        match self {
            Separators::Whitespace => whitespace,
            Separators::Pluginsd => whitespace || c == b'=',
            Separators::Config => whitespace || c == b',',
            Separators::GroupByLabel => c == b',' || c == b'|',
            Separators::DyncfgId => c == b':',
        }
    }
}

/// `PLUGINSD_MAX_WORDS`.
pub const PLUGINSD_MAX_WORDS: usize = 30;

/// Splits the C string in `input` into at most `max_words` words.
pub fn quoted_strings_splitter(
    input: &[u8],
    max_words: usize,
    separators: Separators,
) -> Vec<Vec<u8>> {
    if max_words == 0 {
        return Vec::new();
    }
    // A NUL-terminated working copy that is modified in place, as in C.
    let mut s: Vec<u8> = c_str(input).to_vec();
    s.push(0);
    let is_sep = |c: u8| separators.contains(c);
    let is_quote = |c: u8| c == b'\'' || c == b'"';

    let mut i = 0;
    while is_sep(s[i]) {
        i += 1;
    }
    if s[i] == 0 {
        return Vec::new();
    }
    let mut quote = 0u8;
    if is_quote(s[i]) {
        quote = s[i];
        i += 1;
    }
    let mut starts = vec![i];
    while s[i] != 0 {
        if s[i] == b'\\' && s[i + 1] != 0 {
            i += 2;
        } else if s[i] == quote {
            quote = 0;
            s[i] = b' ';
        } else if quote == 0 && is_sep(s[i]) {
            s[i] = 0;
            i += 1;
            while is_sep(s[i]) {
                i += 1;
            }
            if is_quote(s[i]) {
                quote = s[i];
                i += 1;
            }
            if s[i] == 0 {
                break;
            }
            if starts.len() < max_words {
                starts.push(i);
            } else {
                break;
            }
        } else {
            i += 1;
        }
    }
    starts
        .into_iter()
        .map(|start| {
            let end = s[start..]
                .iter()
                .position(|&c| c == 0)
                .map_or(s.len(), |n| start + n);
            s[start..end].to_vec()
        })
        .collect()
}
