//! The calculator only judges the query engine while it shares none of its
//! code. This test fails when a dependency or an identifier from the judged
//! path appears in the crate.

use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN_CRATES: [&str; 3] = ["sfsq", "otel-ledger", "ng-index"];

/// The query side of `sfst` and the crate paths of the judged crates. The raw
/// per-row id readers of `sfst` stay allowed: they only tell which file a span
/// landed in.
const FORBIDDEN_IDENTIFIERS: [&str; 13] = [
    "sfsq::",
    "otel_ledger::",
    "ng_index::",
    "compile_filter",
    "matched_count",
    ".facets(",
    ".timeline(",
    "materialize_",
    "trace_by_id",
    "trace_combine",
    "trace_plan",
    "TraceFileSession",
    "IndexReader",
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
    let mut files = Vec::new();
    rust_files(&crate_dir().join("src"), &mut files);
    assert!(!files.is_empty());
    for file in files {
        let text = fs::read_to_string(&file).expect("source");
        for ident in FORBIDDEN_IDENTIFIERS {
            assert!(
                !text.contains(ident),
                "{} uses `{ident}`, which belongs to the judged query path",
                file.display()
            );
        }
    }
}
