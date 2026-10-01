//! The plugins.d / streaming text protocol codec (spec: `knowledge/plugins-d.md` §2 in the status repository;
//! C: `src/plugins.d/pluginsd_parser.c`, `pluginsd_internals.h`, `gperf-hashtable.h`,
//! `src/libnetdata/functions_evloop/functions_evloop.h`).
//!
//! A leaf crate: framing, word splitting, keyword lookup, the optional `SLOT:` word and the lines the agent writes.
//! What a keyword *does* (charts, samples, functions) belongs to the ingest code that owns the model (decisions D9).

#![forbid(unsafe_code)]

pub mod caps;
mod keyword;

use netdata_agent_text::c::c_str;
use netdata_agent_text::line_splitter::{PLUGINSD_MAX_WORDS, Separators, quoted_strings_splitter};
use netdata_agent_text::parse::str2ull_encoded;

pub use keyword::{Keyword, Repertoire};

/// `COMPRESSION_MAX_CHUNK`.
pub const COMPRESSION_MAX_CHUNK: usize = 0x4000;
/// `COMPRESSION_MAX_MSG_SIZE`: the largest piece of a stream commit, and the largest compressed message a receiver
/// accepts.
pub const COMPRESSION_MAX_MSG_SIZE: usize = COMPRESSION_MAX_CHUNK - 128 - 1;
/// `PLUGINSD_LINE_MAX` (`COMPRESSION_MAX_MSG_SIZE - 768`): the size of each read from a plugin or a child.
pub const LINE_MAX: usize = COMPRESSION_MAX_MSG_SIZE - 768;
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

    /// `quoted_strings_splitter_whitespace()`: the words of a line a stream sender receives.
    pub fn split_whitespace(line: &[u8]) -> Self {
        Words(quoted_strings_splitter(
            line,
            PLUGINSD_MAX_WORDS,
            Separators::Whitespace,
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
        self.slot_checked(max_slot).map(|s| s.unwrap_or(0))
    }

    /// `pluginsd_parse_rrd_slot()` in one parse: `None` without a `SLOT:` word, `Some(Ok(n))` within the cap, and
    /// `Some(Err(text))` over it, with the text after `SLOT:` that C's warning prints.
    pub fn slot_checked(&self, max_slot: u64) -> Option<Result<u64, &[u8]>> {
        let value = self.get(1)?.strip_prefix(b"SLOT:")?;
        let parsed = str2ull_encoded(value);
        Some(if parsed > max_slot { Err(value) } else { Ok(parsed) })
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
    /// Appends received bytes and returns every complete line, newline included. Only the new bytes are searched: what
    /// was pending holds no newline.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut searched = self.pending.len();
        self.pending.extend_from_slice(bytes);
        let mut lines = Vec::new();
        let mut start = 0;
        while let Some(nl) = self.pending[searched..].iter().position(|&c| c == b'\n') {
            let end = searched + nl + 1;
            lines.push(self.pending[start..end].to_vec());
            start = end;
            searched = end;
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
    /// The body passed `MAX_DEFERRED_SIZE` (its size): the run ends.
    TooBig(usize),
}

/// A deferred body (`FUNCTION_RESULT_BEGIN` ... `FUNCTION_RESULT_END`, streaming `JSON` ... `JSON_PAYLOAD_END`).
#[derive(Debug)]
pub struct DeferredBody {
    end_keyword: Vec<u8>,
    body: Vec<u8>,
    /// Whether the body is kept (`parser->defer.response` set); a discarded body has no size limit.
    keep: bool,
}

impl DeferredBody {
    pub fn new(end_keyword: &str) -> Self {
        DeferredBody {
            end_keyword: end_keyword.as_bytes().to_vec(),
            body: Vec::new(),
            keep: true,
        }
    }

    /// A body nobody waits for (a result for an unknown transaction): skipped up to the end keyword.
    pub fn discarding(end_keyword: &str) -> Self {
        DeferredBody {
            keep: false,
            ..DeferredBody::new(end_keyword)
        }
    }

    /// Feeds one raw line (newline included). The end line is recognized by its first keyword as
    /// `find_first_keyword()` takes it: after leading separators, the bytes up to the next one (quotes kept), at most
    /// 99.
    pub fn feed(&mut self, line: &[u8]) -> Deferred {
        let line_c = c_str(line);
        let start = line_c.iter().position(|&c| !Separators::Pluginsd.contains(c)).unwrap_or(line_c.len());
        let rest = &line_c[start..];
        let len = rest.iter().position(|&c| Separators::Pluginsd.contains(c)).unwrap_or(rest.len()).min(99);
        if len > 0 && rest[..len] == self.end_keyword[..] {
            return Deferred::Done(std::mem::take(&mut self.body));
        }
        if !self.keep {
            return Deferred::Continue;
        }
        self.body.extend_from_slice(c_str(line));
        if self.body.len() > MAX_DEFERRED_SIZE {
            return Deferred::TooBig(self.body.len());
        }
        Deferred::Continue
    }
}

/// The lines the agent writes to a plugin or a child to drive a function call (`src/plugins.d/pluginsd_functions.c`),
/// and a streaming child's lines to its parent (`stream`).
pub mod emit {
    use std::fmt::Write as _;

    pub mod stream;

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

    /// A long line read in LINE_MAX chunks costs its length, not its length squared (R59-2): each push searches only
    /// the bytes it brought.
    #[test]
    fn a_long_line_is_read_in_linear_time() {
        let mut r = LineReader::default();
        let chunk = vec![b'x'; LINE_MAX];
        let started = std::time::Instant::now();
        for _ in 0..(32 << 20) / LINE_MAX {
            assert!(r.push(&chunk).is_empty());
        }
        let lines = r.push(b"\n");
        assert_eq!((lines.len(), lines[0].len()), (1, (32 << 20) / LINE_MAX * LINE_MAX + 1));
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    }

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
        assert_eq!(Words::split(b"BEGIN SLOT:2000000 x\n").slot_checked(CHART_SLOT_MAX), Some(Err(&b"2000000"[..])));
        assert_eq!(Words::split(b"BEGIN SLOT:0 x\n").slot_checked(CHART_SLOT_MAX), Some(Ok(0)));
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
        // A discarded body never grows too big.
        let mut d = DeferredBody::discarding("FUNCTION_RESULT_END");
        let line = vec![b'x'; MAX_DEFERRED_SIZE + 1];
        assert_eq!(d.feed(&line), Deferred::Continue);
        assert_eq!(d.feed(b"FUNCTION_RESULT_END\n"), Deferred::Done(Vec::new()));
    }

    /// `find_first_keyword()` ends a deferred body: the bytes up to the first separator, quotes kept, so a quoted end
    /// keyword is body; `=` separates as in every pluginsd word.
    #[test]
    fn a_quoted_end_keyword_stays_in_the_body_as_c() {
        let mut d = DeferredBody::new("JSON_PAYLOAD_END");
        assert_eq!(d.feed(b"'JSON_PAYLOAD_END'\n"), Deferred::Continue);
        assert_eq!(d.feed(b"\"JSON_PAYLOAD_END\" x\n"), Deferred::Continue);
        assert_eq!(
            d.feed(b"JSON_PAYLOAD_END=x\n"),
            Deferred::Done(b"'JSON_PAYLOAD_END'\n\"JSON_PAYLOAD_END\" x\n".to_vec())
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
