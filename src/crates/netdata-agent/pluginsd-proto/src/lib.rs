//! The plugins.d / streaming text protocol codec (spec: `knowledge/plugins-d.md` §2 in the status repository;
//! C: `src/plugins.d/pluginsd_parser.c`, `pluginsd_internals.h`, `gperf-hashtable.h`,
//! `src/libnetdata/functions_evloop/functions_evloop.h`).
//!
//! A leaf crate: framing, word splitting, keyword lookup, the optional `SLOT:` word and the lines the agent writes.
//! What a keyword *does* (charts, samples, functions) belongs to the ingest code that owns the model (decisions D9).

#![forbid(unsafe_code)]

mod keyword;

use netdata_agent_text::c::c_str;
use netdata_agent_text::line_splitter::{PLUGINSD_MAX_WORDS, Separators, quoted_strings_splitter};
use netdata_agent_text::parse::str2ull_encoded;

pub use keyword::{Keyword, Repertoire};

/// `PLUGINSD_LINE_MAX` (`COMPRESSION_MAX_MSG_SIZE - 768`): the size of each read from a plugin or a child.
pub const LINE_MAX: usize = 15_487;
/// `PLUGINSD_MAX_DEFERRED_SIZE`: the largest deferred body (`FUNCTION_RESULT_BEGIN`, `JSON`).
pub const MAX_DEFERRED_SIZE: usize = 100 * 1024 * 1024;
/// `PLUGINSD_CHART_SLOT_MAX`.
pub const CHART_SLOT_MAX: u64 = 1_000_000;
/// `PLUGINSD_DIMENSION_SLOT_MAX`.
pub const DIMENSION_SLOT_MAX: u64 = 65_535;

/// The words of one line, as `quoted_strings_splitter_pluginsd()` leaves them (at most 30).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Words(Vec<Vec<u8>>);

impl Words {
    pub fn split(line: &[u8]) -> Self {
        Words(quoted_strings_splitter(
            line,
            PLUGINSD_MAX_WORDS,
            Separators::Pluginsd,
        ))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// `get_word()`: `None` past the last word, which handlers treat differently from an empty word.
    pub fn get(&self, index: usize) -> Option<&[u8]> {
        self.0.get(index).map(Vec::as_slice)
    }

    /// The keyword in word 0, if it is one.
    pub fn keyword(&self) -> Option<Keyword> {
        self.get(0).and_then(Keyword::lookup)
    }

    /// `pluginsd_parse_rrd_slot()`: `None` when word 1 is not `SLOT:<n>`; `Some(0)` when `n` exceeds `max_slot`
    /// (the word is present but unusable as a cache index); otherwise `Some(n)`.
    pub fn slot(&self, max_slot: u64) -> Option<u64> {
        let id = self.get(1)?;
        let value = id.strip_prefix(b"SLOT:")?;
        let parsed = str2ull_encoded(value);
        Some(if parsed > max_slot { 0 } else { parsed })
    }

    /// `line_splitter_reconstruct_line()`: every word in single quotes, joined by spaces (used in error logs).
    pub fn reconstruct(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, word) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(b' ');
            }
            out.push(b'\'');
            out.extend_from_slice(word);
            out.push(b'\'');
        }
        out
    }
}

/// Splits a byte stream into lines on `\n`, keeping the newline, with no length cap: partial lines accumulate.
#[derive(Debug, Default)]
pub struct LineReader {
    pending: Vec<u8>,
}

impl LineReader {
    /// Appends received bytes and returns every complete line, newline included.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.pending.extend_from_slice(bytes);
        let mut lines = Vec::new();
        let mut start = 0;
        while let Some(nl) = self.pending[start..].iter().position(|&c| c == b'\n') {
            lines.push(self.pending[start..start + nl + 1].to_vec());
            start += nl + 1;
        }
        self.pending.drain(..start);
        lines
    }

    /// Bytes received after the last newline.
    pub fn pending(&self) -> &[u8] {
        &self.pending
    }
}

/// What feeding one line to a deferred body did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deferred {
    /// The line was appended to the body.
    Continue,
    /// The end keyword arrived; the body is complete (the end line is not part of it).
    Done(Vec<u8>),
    /// The body passed `MAX_DEFERRED_SIZE`: the run ends.
    TooBig,
}

/// A deferred body (`FUNCTION_RESULT_BEGIN` ... `FUNCTION_RESULT_END`, streaming `JSON` ... `JSON_PAYLOAD_END`).
#[derive(Debug)]
pub struct DeferredBody {
    end_keyword: Vec<u8>,
    body: Vec<u8>,
}

impl DeferredBody {
    pub fn new(end_keyword: &str) -> Self {
        DeferredBody {
            end_keyword: end_keyword.as_bytes().to_vec(),
            body: Vec::new(),
        }
    }

    /// Feeds one raw line (newline included). The end line is recognized by its first word (same separators as the
    /// tokenizer), compared on its first 99 bytes.
    pub fn feed(&mut self, line: &[u8]) -> Deferred {
        let first = quoted_strings_splitter(line, 1, Separators::Pluginsd);
        if let Some(word) = first.first() {
            if word[..word.len().min(99)] == self.end_keyword[..] {
                return Deferred::Done(std::mem::take(&mut self.body));
            }
        }
        self.body.extend_from_slice(c_str(line));
        if self.body.len() > MAX_DEFERRED_SIZE {
            return Deferred::TooBig;
        }
        Deferred::Continue
    }
}

/// The lines the agent writes to a plugin or a child to drive a function call (`src/plugins.d/pluginsd_functions.c`).
pub mod emit {
    use std::fmt::Write as _;

    /// `FUNCTION <tx> <timeout> "<cmd>" "0x<access>" "<source>"\n`; strings go out raw.
    pub fn function(tx: &str, timeout_s: i32, command: &str, access: u32, source: &str) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "FUNCTION {tx} {timeout_s} \"{command}\" \"0x{access:x}\" \"{source}\""
        );
        s
    }

    /// `FUNCTION_PAYLOAD ...` + payload + `FUNCTION_PAYLOAD_END`, written in one piece.
    pub fn function_payload(
        tx: &str,
        timeout_s: i32,
        command: &str,
        access: u32,
        source: &str,
        content_type: &str,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut out = format!(
            "FUNCTION_PAYLOAD {tx} {timeout_s} \"{command}\" \"0x{access:x}\" \"{source}\" \"{content_type}\"\n"
        )
        .into_bytes();
        out.extend_from_slice(payload);
        out.extend_from_slice(b"\nFUNCTION_PAYLOAD_END\n");
        out
    }

    pub fn function_cancel(tx: &str) -> String {
        format!("FUNCTION_CANCEL {tx}\n")
    }

    pub fn function_progress(tx: &str) -> String {
        format!("FUNCTION_PROGRESS {tx}\n")
    }

    /// `QUIT`: four bytes, no newline.
    pub const QUIT: &[u8] = b"QUIT";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_slot_and_reconstruction() {
        let w = Words::split(b"SET SLOT:12 user = 5\n");
        assert_eq!(w.keyword(), Some(Keyword::Set));
        assert_eq!(w.slot(DIMENSION_SLOT_MAX), Some(12));
        assert_eq!(w.get(2), Some(&b"user"[..]));
        assert_eq!(w.get(3), Some(&b"5"[..]));
        assert_eq!(w.get(4), None);
        assert_eq!(w.reconstruct(), b"'SET' 'SLOT:12' 'user' '5'".to_vec());
        assert_eq!(
            Words::split(b"BEGIN SLOT:2000000 x\n").slot(CHART_SLOT_MAX),
            Some(0)
        );
        assert_eq!(Words::split(b"BEGIN slot:2 x\n").slot(CHART_SLOT_MAX), None);
        assert_eq!(Words::split(b"\n").keyword(), None);
    }

    #[test]
    fn line_reader_keeps_partial_lines() {
        let mut r = LineReader::default();
        assert!(r.push(b"BEGIN a").is_empty());
        assert_eq!(
            r.push(b"\nSET x 1\nEN"),
            vec![b"BEGIN a\n".to_vec(), b"SET x 1\n".to_vec()]
        );
        assert_eq!(r.pending(), b"EN");
    }

    #[test]
    fn deferred_bodies_end_on_the_first_word() {
        let mut d = DeferredBody::new("FUNCTION_RESULT_END");
        assert_eq!(d.feed(b"{\"a\":1}\n"), Deferred::Continue);
        assert_eq!(
            d.feed(b"  FUNCTION_RESULT_END extra\n"),
            Deferred::Done(b"{\"a\":1}\n".to_vec())
        );
    }

    #[test]
    fn emitters() {
        assert_eq!(
            emit::function("0123", 10, "top", 0x13, "method=api"),
            "FUNCTION 0123 10 \"top\" \"0x13\" \"method=api\"\n"
        );
        assert_eq!(emit::QUIT, b"QUIT");
    }
}
