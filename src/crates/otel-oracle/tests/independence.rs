//! The calculator only judges the query engine while it shares none of its
//! code. This test fails when a dependency or an identifier from the judged
//! path appears in the crate's `src/` (tests and fixtures may build stores).

use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN_CRATES: [&str; 4] = ["sfsq", "otel-ledger", "ng-index", "file-lifecycle"];

/// The query side of `sfst`, the engine's own grouping and ingest code, and
/// the crate paths of the judged crates, matched as whole words: `_role` or
/// `_duration_band` inside the calculator's own names do not count.
const FORBIDDEN_IDENTIFIERS: [&str; 50] = [
    "sfsq::",
    "otel_ledger::",
    "ng_index::",
    "file_registry::",
    "file_lifecycle::",
    "prefix::",
    "compile_filter",
    "matched_count",
    ".facets(",
    ".timeline(",
    "trace_by_id",
    "trace_combine",
    "trace_plan",
    "TraceFileSession",
    "IndexReader",
    "chunk_boundaries",
    "tail_start",
    "scan_frame_boundaries",
    "normalize_trace_request",
    "prepare_trace_frame",
    "flatten_trace_request",
    "duration_band",
    "CHUNK_MIN_ENTRIES",
    "MAX_FACET_VALUES",
    "DEFAULT_CARDINALITY_THRESHOLD",
    "FieldTier",
    "TRACE_PINNED_FIELDS",
    "row_values",
    "GROUPS_CAP",
    "compile_duration",
    "compile_time_range",
    "count_without",
    "MIN_SUPPORT",
    "span_family",
    "derive_span_family",
    "GroupAcc",
    "GroupSides",
    "SideNumbers",
    "ScopeTraces",
    "UnsetRows",
    "UNSET_FACET_FIELDS",
    "count_absent",
    "facet_unset",
    "value_order",
    "build_forest",
    "canonical_bytes",
    "content_hash",
    "SpanRef",
    "SpanSource",
    "CombineOutcome",
];

/// Matched at the start of a word only: families of names.
const FORBIDDEN_PREFIXES: [&str; 2] = ["materialize_", "build_sfst"];

/// The format-layer reads membership may make, and nothing else of the store's
/// crates, anywhere.
const STORE_PATHS: [(&str, &str); 3] = [
    ("sfst::", "ChunkReader"),
    ("wal::", "Reader"),
    ("ng_flatten::", "decode_trace_frame"),
];
const MEMBERSHIP: &str = "membership.rs";

/// Reader methods membership must not call: everything beyond ids, times and
/// durations. The column manifest may be read for which columns exist (the
/// explorer's legacy rule), never the seal's derived values.
const MEMBERSHIP_DENIED: [&str; 13] = [
    ".metadata(",
    ".fields(",
    ".tree(",
    "_raw(",
    ".stream_batch",
    ".trace_id_index(",
    ".trace_rollup(",
    ".event_index(",
    ".link_index(",
    ".child_durations(",
    ".decode_counts(",
    ".open_range(",
    ".header(",
];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn sources() -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    rust_files(&crate_dir().join("src"), &mut files);
    assert!(!files.is_empty());
    files
        .into_iter()
        .map(|file| {
            let text = fs::read_to_string(&file).expect("source");
            (file, text)
        })
        .collect()
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Byte offsets where `needle` starts a word (and, unless `prefix`, also ends
/// one).
fn word_matches(text: &str, needle: &str, prefix: bool) -> Vec<usize> {
    let starts_ident = needle.starts_with(is_ident);
    let ends_ident = needle.ends_with(is_ident);
    let mut out = Vec::new();
    for (at, _) in text.match_indices(needle) {
        let before = text[..at].chars().next_back();
        let after = text[at + needle.len()..].chars().next();
        let starts_word = !starts_ident || !before.is_some_and(is_ident);
        let ends_word = prefix || !ends_ident || !after.is_some_and(is_ident);
        if starts_word && ends_word {
            out.push(at);
        }
    }
    out
}

fn forbidden_in(text: &str) -> Option<&'static str> {
    FORBIDDEN_IDENTIFIERS
        .into_iter()
        .find(|ident| !word_matches(text, ident, false).is_empty())
        .or_else(|| {
            FORBIDDEN_PREFIXES
                .into_iter()
                .find(|ident| !word_matches(text, ident, true).is_empty())
        })
}

/// Why `text` reaches the store's crates beyond what `file` may use.
fn store_access_in(file: &Path, text: &str) -> Option<String> {
    let is_membership = file.file_name().is_some_and(|n| n == MEMBERSHIP);
    for (path, allowed) in STORE_PATHS {
        for at in word_matches(text, path, true) {
            let rest = &text[at + path.len()..];
            if !is_membership {
                return Some(format!("`{path}` outside {MEMBERSHIP}"));
            }
            let named = rest.starts_with(allowed) && !rest[allowed.len()..].starts_with(is_ident);
            let line = rest.lines().next().unwrap_or("");
            if !named || line.contains(" as ") {
                return Some(format!("`{path}{}` is not `{path}{allowed}`", line.trim()));
            }
        }
    }
    if is_membership {
        if let Some(denied) = MEMBERSHIP_DENIED.into_iter().find(|d| text.contains(d)) {
            return Some(format!("reader call `{denied}`"));
        }
    }
    None
}

#[test]
fn manifest_has_no_judged_crate() {
    let manifest = fs::read_to_string(crate_dir().join("Cargo.toml")).expect("manifest");
    let mut section = String::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line.to_string();
            continue;
        }
        if !section.contains("dependencies") {
            continue;
        }
        let name = line.split(['=', ' ']).next().unwrap_or("");
        assert!(
            !FORBIDDEN_CRATES.contains(&name),
            "{section} depends on `{name}`, which the calculator judges"
        );
    }
}

#[test]
fn sources_use_no_judged_code() {
    for (file, text) in sources() {
        if let Some(ident) = forbidden_in(&text) {
            panic!(
                "{} uses `{ident}`, which belongs to the judged code",
                file.display()
            );
        }
    }
}

#[test]
fn only_membership_reads_the_store_and_only_ids_times_and_durations() {
    for (file, text) in sources() {
        if let Some(why) = store_access_in(&file, &text) {
            panic!("{}: {why}", file.display());
        }
    }
}

#[test]
fn the_guard_catches_what_it_is_for() {
    let membership = Path::new("src/membership.rs");
    let model = Path::new("src/model.rs");
    let cases: [(&str, &Path, &str, bool); 13] = [
        (
            "the engine's reader",
            membership,
            "use sfst::IndexReader;",
            true,
        ),
        (
            "a grouped import",
            membership,
            "use sfst::{ChunkReader, IndexReader};",
            true,
        ),
        ("a glob", membership, "use wal::*;", true),
        ("an alias", membership, "use wal::Reader as R;", true),
        (
            "the engine's fold",
            membership,
            "wal::prefix::chunk_boundaries(&f, 16)",
            true,
        ),
        ("a column beyond ids", membership, "reader.fields()", true),
        (
            "a store read outside membership",
            model,
            "sfst::ChunkReader::open(&d)",
            true,
        ),
        (
            "an allowed read",
            membership,
            "sfst::ChunkReader::open(&data)",
            false,
        ),
        (
            "the allowed WAL reader",
            membership,
            "wal::Reader::open(path)",
            false,
        ),
        (
            "the calculator's own field name",
            model,
            "\"_duration_band\"",
            false,
        ),
        (
            "a name containing a banned word",
            model,
            "fn a_mismatched_count() {}",
            false,
        ),
        (
            "the engine's band function",
            model,
            "duration_band(ns)",
            true,
        ),
        (
            "the engine's trace tree",
            Path::new("src/assembly.rs"),
            "let (roots, children) = build_forest(&spans);",
            true,
        ),
    ];
    for (name, file, text, caught) in cases {
        let found = forbidden_in(text).is_some() || store_access_in(file, text).is_some();
        assert_eq!(found, caught, "{name}");
    }
}
