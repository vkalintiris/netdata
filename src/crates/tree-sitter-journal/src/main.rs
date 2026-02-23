//! Stress-test tool for the journal log writer.
//!
//! Walks a source repository, parses files with tree-sitter, and writes the
//! extracted AST information as journal log entries.
//!
//! Usage:
//!     cargo run --release -p tree-sitter-journal -- [OPTIONS] <REPO_PATH>

use clap::{Parser, ValueEnum};
use journal_log_writer::{Config, Log, RetentionPolicy, RotationPolicy};
use journal_registry::Origin;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tree_sitter::Node;
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    about = "Parse source files with tree-sitter and write AST data as journal log entries"
)]
struct Args {
    /// Path to the source repository to parse.
    repo_path: PathBuf,

    /// Journal output directory.
    #[arg(short, long, default_value = "./journal-out")]
    output_dir: PathBuf,

    /// What to log from each parsed file.
    #[arg(short, long, default_value = "symbols")]
    granularity: Granularity,

    /// Maximum journal file size in bytes (e.g. 33554432 for 32 MB).
    #[arg(long, default_value_t = 32 * 1024 * 1024)]
    rotation_size: u64,

    /// Maximum number of entries per journal file.
    #[arg(long)]
    rotation_entries: Option<usize>,

    /// Maximum number of journal files to keep.
    #[arg(long)]
    retention_files: Option<usize>,

    /// Include source text snippets in entries.
    #[arg(long)]
    include_text: bool,

    /// Maximum length of included text snippets (bytes).
    #[arg(long, default_value_t = 200)]
    max_text_len: usize,

    /// Sync to disk every N entries (0 = only at the end).
    #[arg(long, default_value_t = 10_000)]
    sync_every: usize,
}

#[derive(Clone, Copy, ValueEnum)]
enum Granularity {
    /// One entry per file (lowest volume).
    FileSummary,
    /// One entry per symbol definition + file summary (medium volume).
    Symbols,
    /// One entry per named AST node (highest volume).
    AllNodes,
}

impl std::fmt::Display for Granularity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Granularity::FileSummary => write!(f, "file-summary"),
            Granularity::Symbols => write!(f, "symbols"),
            Granularity::AllNodes => write!(f, "all-nodes"),
        }
    }
}

// ---------------------------------------------------------------------------
// Language support
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Lang {
    Rust,
    C,
}

impl Lang {
    fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Lang::Rust),
            "c" | "h" => Some(Lang::C),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::C => "c",
        }
    }
}

// ---------------------------------------------------------------------------
// Counters per file
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FileCounts {
    nodes: usize,
    functions: usize,
    structs: usize,
    enums: usize,
    impls: usize,
    traits: usize,
    macros: usize,
    type_defs: usize,
}

// ---------------------------------------------------------------------------
// Entry builder
// ---------------------------------------------------------------------------

/// Collects KEY=VALUE pairs for one journal entry, then hands them off to the
/// writer.
struct EntryBuilder {
    items: Vec<Vec<u8>>,
}

impl EntryBuilder {
    fn new() -> Self {
        Self {
            items: Vec::with_capacity(16),
        }
    }

    fn field(&mut self, key: &str, value: &str) -> &mut Self {
        let mut buf = Vec::with_capacity(key.len() + 1 + value.len());
        buf.extend_from_slice(key.as_bytes());
        buf.push(b'=');
        buf.extend_from_slice(value.as_bytes());
        self.items.push(buf);
        self
    }

    fn field_u(&mut self, key: &str, value: usize) -> &mut Self {
        self.field(key, &value.to_string())
    }

    fn refs(&self) -> Vec<&[u8]> {
        self.items.iter().map(|v| v.as_slice()).collect()
    }

    fn clear(&mut self) {
        self.items.clear();
    }
}

// ---------------------------------------------------------------------------
// Walking helpers
// ---------------------------------------------------------------------------

fn is_symbol_node(lang: Lang, kind: &str) -> bool {
    match lang {
        Lang::Rust => matches!(
            kind,
            "function_item"
                | "struct_item"
                | "enum_item"
                | "trait_item"
                | "impl_item"
                | "type_item"
                | "const_item"
                | "static_item"
                | "macro_definition"
                | "mod_item"
        ),
        Lang::C => matches!(
            kind,
            "function_definition"
                | "struct_specifier"
                | "enum_specifier"
                | "type_definition"
                | "declaration"
        ),
    }
}

fn count_node(lang: Lang, kind: &str, counts: &mut FileCounts) {
    counts.nodes += 1;
    match lang {
        Lang::Rust => match kind {
            "function_item" => counts.functions += 1,
            "struct_item" => counts.structs += 1,
            "enum_item" => counts.enums += 1,
            "impl_item" => counts.impls += 1,
            "trait_item" => counts.traits += 1,
            "macro_definition" => counts.macros += 1,
            "type_item" => counts.type_defs += 1,
            _ => {}
        },
        Lang::C => match kind {
            "function_definition" => counts.functions += 1,
            "struct_specifier" => counts.structs += 1,
            "enum_specifier" => counts.enums += 1,
            "type_definition" => counts.type_defs += 1,
            _ => {}
        },
    }
}

/// Extract the "name" child from a node, if present.
fn extract_name<'a>(node: Node<'a>, source: &'a [u8]) -> Option<&'a str> {
    // Try common name-bearing child field names.
    for field in &["name", "type"] {
        if let Some(child) = node.child_by_field_name(field) {
            if let Ok(text) = child.utf8_text(source) {
                return Some(text);
            }
        }
    }
    None
}

/// Extract visibility (pub, pub(crate), etc.) from a Rust node.
fn extract_visibility<'a>(node: Node<'a>, source: &'a [u8]) -> Option<&'a str> {
    if let Some(vis) = node.child_by_field_name("visibility") {
        return vis.utf8_text(source).ok();
    }
    // Walk immediate children looking for visibility_modifier.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            return child.utf8_text(source).ok();
        }
    }
    None
}

/// Truncate a string to at most `max_len` bytes on a char boundary.
fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    let mut end = max_len;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ---------------------------------------------------------------------------
// Entry writers
// ---------------------------------------------------------------------------

fn write_file_summary(
    log: &mut Log,
    eb: &mut EntryBuilder,
    rel_path: &str,
    lang: Lang,
    source: &[u8],
    counts: &FileCounts,
) -> journal_log_writer::Result<()> {
    let line_count = bytecount(source, b'\n') + 1;

    eb.clear();
    eb.field(
        "MESSAGE",
        &format!(
            "Parsed {}: {} lines, {} nodes, {} functions, {} structs",
            rel_path, line_count, counts.nodes, counts.functions, counts.structs,
        ),
    );
    eb.field("PRIORITY", "6");
    eb.field("FILE", rel_path);
    eb.field("LANGUAGE", lang.name());
    eb.field_u("FILE_SIZE", source.len());
    eb.field_u("LINE_COUNT", line_count);
    eb.field_u("NODE_COUNT", counts.nodes);
    eb.field_u("FUNCTION_COUNT", counts.functions);
    eb.field_u("STRUCT_COUNT", counts.structs);
    eb.field_u("ENUM_COUNT", counts.enums);
    eb.field_u("IMPL_COUNT", counts.impls);
    eb.field_u("TRAIT_COUNT", counts.traits);
    eb.field_u("MACRO_COUNT", counts.macros);
    eb.field_u("TYPE_DEF_COUNT", counts.type_defs);

    log.write_entry(&eb.refs(), None)
}

fn write_symbol_entry(
    log: &mut Log,
    eb: &mut EntryBuilder,
    node: Node<'_>,
    source: &[u8],
    rel_path: &str,
    lang: Lang,
    depth: usize,
    include_text: bool,
    max_text_len: usize,
) -> journal_log_writer::Result<()> {
    let kind = node.kind();
    let name = extract_name(node, source).unwrap_or("<anonymous>");
    let start = node.start_position();
    let end = node.end_position();
    let line_count = end.row.saturating_sub(start.row) + 1;

    // Build a one-line signature for MESSAGE.
    let sig = if let Ok(text) = node.utf8_text(source) {
        // Take just the first line of the node text.
        let first_line = text.lines().next().unwrap_or(text);
        truncate(first_line, 120).to_string()
    } else {
        format!("{kind} {name}")
    };

    eb.clear();
    eb.field("MESSAGE", &sig);
    eb.field("PRIORITY", "6");
    eb.field("SYMBOL_TYPE", kind);
    eb.field("SYMBOL_NAME", name);
    eb.field("FILE", rel_path);
    eb.field("LANGUAGE", lang.name());
    eb.field_u("START_LINE", start.row + 1);
    eb.field_u("END_LINE", end.row + 1);
    eb.field_u("LINE_COUNT", line_count);
    eb.field_u("DEPTH", depth);

    if let Some(vis) = extract_visibility(node, source) {
        eb.field("VISIBILITY", vis);
    }

    if include_text {
        if let Ok(text) = node.utf8_text(source) {
            eb.field("TEXT", truncate(text, max_text_len));
        }
    }

    log.write_entry(&eb.refs(), None)
}

fn write_all_node_entry(
    log: &mut Log,
    eb: &mut EntryBuilder,
    node: Node<'_>,
    source: &[u8],
    rel_path: &str,
    lang: Lang,
    depth: usize,
    include_text: bool,
    max_text_len: usize,
) -> journal_log_writer::Result<()> {
    let kind = node.kind();
    let start = node.start_position();
    let end = node.end_position();

    // Brief message: node kind + snippet
    let snippet = if let Ok(text) = node.utf8_text(source) {
        let first_line = text.lines().next().unwrap_or(text);
        truncate(first_line, 60).to_string()
    } else {
        String::new()
    };

    eb.clear();
    eb.field("MESSAGE", &format!("{kind} \"{snippet}\""));
    eb.field("PRIORITY", "7");
    eb.field("NODE_TYPE", kind);
    eb.field("FILE", rel_path);
    eb.field("LANGUAGE", lang.name());
    eb.field_u("DEPTH", depth);
    eb.field_u("START_LINE", start.row + 1);
    eb.field_u("START_COL", start.column);
    eb.field_u("END_LINE", end.row + 1);
    eb.field_u("END_COL", end.column);
    eb.field_u("CHILD_COUNT", node.child_count());

    if let Some(parent) = node.parent() {
        eb.field("PARENT_TYPE", parent.kind());
    }

    if include_text {
        if let Ok(text) = node.utf8_text(source) {
            eb.field("TEXT", truncate(text, max_text_len));
        }
    }

    log.write_entry(&eb.refs(), None)
}

/// Count occurrences of a byte in a slice.
fn bytecount(data: &[u8], needle: u8) -> usize {
    data.iter().filter(|&&b| b == needle).count()
}

// ---------------------------------------------------------------------------
// Tree walking
// ---------------------------------------------------------------------------

struct WalkState<'a> {
    log: &'a mut Log,
    eb: EntryBuilder,
    granularity: Granularity,
    include_text: bool,
    max_text_len: usize,
    entries_written: usize,
    sync_every: usize,
}

impl WalkState<'_> {
    fn maybe_sync(&mut self) -> journal_log_writer::Result<()> {
        if self.sync_every > 0 && self.entries_written % self.sync_every == 0 {
            self.log.sync()?;
        }
        Ok(())
    }
}

fn walk_tree(
    state: &mut WalkState<'_>,
    tree: &tree_sitter::Tree,
    source: &[u8],
    rel_path: &str,
    lang: Lang,
) -> journal_log_writer::Result<FileCounts> {
    let mut counts = FileCounts::default();
    let root = tree.root_node();

    match state.granularity {
        Granularity::FileSummary => {
            // Just count everything, emit one summary.
            count_all_nodes(root, lang, &mut counts);
            write_file_summary(state.log, &mut state.eb, rel_path, lang, source, &counts)?;
            state.entries_written += 1;
            state.maybe_sync()?;
        }

        Granularity::Symbols => {
            // Walk tree: emit symbol entries, count everything.
            walk_symbols(state, root, source, rel_path, lang, 0, &mut counts)?;
            // Then emit file summary.
            write_file_summary(state.log, &mut state.eb, rel_path, lang, source, &counts)?;
            state.entries_written += 1;
            state.maybe_sync()?;
        }

        Granularity::AllNodes => {
            // Emit an entry for every named node.
            walk_all_named(state, root, source, rel_path, lang, 0, &mut counts)?;
            // Then emit file summary.
            write_file_summary(state.log, &mut state.eb, rel_path, lang, source, &counts)?;
            state.entries_written += 1;
            state.maybe_sync()?;
        }
    }

    Ok(counts)
}

fn count_all_nodes(node: Node<'_>, lang: Lang, counts: &mut FileCounts) {
    count_node(lang, node.kind(), counts);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        count_all_nodes(child, lang, counts);
    }
}

fn walk_symbols(
    state: &mut WalkState<'_>,
    node: Node<'_>,
    source: &[u8],
    rel_path: &str,
    lang: Lang,
    depth: usize,
    counts: &mut FileCounts,
) -> journal_log_writer::Result<()> {
    count_node(lang, node.kind(), counts);

    if is_symbol_node(lang, node.kind()) {
        write_symbol_entry(
            state.log,
            &mut state.eb,
            node,
            source,
            rel_path,
            lang,
            depth,
            state.include_text,
            state.max_text_len,
        )?;
        state.entries_written += 1;
        state.maybe_sync()?;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_symbols(state, child, source, rel_path, lang, depth + 1, counts)?;
    }
    Ok(())
}

fn walk_all_named(
    state: &mut WalkState<'_>,
    node: Node<'_>,
    source: &[u8],
    rel_path: &str,
    lang: Lang,
    depth: usize,
    counts: &mut FileCounts,
) -> journal_log_writer::Result<()> {
    count_node(lang, node.kind(), counts);

    if node.is_named() {
        write_all_node_entry(
            state.log,
            &mut state.eb,
            node,
            source,
            rel_path,
            lang,
            depth,
            state.include_text,
            state.max_text_len,
        )?;
        state.entries_written += 1;
        state.maybe_sync()?;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_all_named(state, child, source, rel_path, lang, depth + 1, counts)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Validate input path.
    if !args.repo_path.is_dir() {
        eprintln!("Error: {} is not a directory", args.repo_path.display());
        std::process::exit(1);
    }

    // Create output directory.
    std::fs::create_dir_all(&args.output_dir)?;

    // Set up parsers.
    let mut rust_parser = tree_sitter::Parser::new();
    rust_parser.set_language(&tree_sitter_rust::LANGUAGE.into())?;

    let mut c_parser = tree_sitter::Parser::new();
    c_parser.set_language(&tree_sitter_c::LANGUAGE.into())?;

    // Configure journal log writer.
    let mut rotation = RotationPolicy::default()
        .with_size_of_journal_file(args.rotation_size);

    if let Some(n) = args.rotation_entries {
        rotation = rotation.with_number_of_entries(n);
    }

    let mut retention = RetentionPolicy::default();
    if let Some(n) = args.retention_files {
        retention = retention.with_number_of_journal_files(n);
    }

    let origin = Origin {
        machine_id: None,
        namespace: None,
        source: journal_registry::Source::System,
    };

    let config = Config::new(origin, rotation, retention);
    let mut log = Log::new(&args.output_dir, config)?;

    println!("Repository:  {}", args.repo_path.display());
    println!("Output:      {}", args.output_dir.display());
    println!("Granularity: {}", args.granularity);
    println!("Rotation:    {} bytes per file", args.rotation_size);
    println!();

    // Discover source files.
    let start = Instant::now();
    let mut files_parsed = 0usize;
    let mut files_skipped = 0usize;
    let mut parse_errors = 0usize;
    let mut total_bytes = 0u64;

    let mut state = WalkState {
        log: &mut log,
        eb: EntryBuilder::new(),
        granularity: args.granularity,
        include_text: args.include_text,
        max_text_len: args.max_text_len,
        entries_written: 0,
        sync_every: args.sync_every,
    };

    for entry in WalkDir::new(&args.repo_path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_hidden(e))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(ext) => ext,
            None => continue,
        };

        let lang = match Lang::from_extension(ext) {
            Some(l) => l,
            None => {
                files_skipped += 1;
                continue;
            }
        };

        // Read source file.
        let source = match std::fs::read(path) {
            Ok(s) => s,
            Err(_) => {
                files_skipped += 1;
                continue;
            }
        };

        total_bytes += source.len() as u64;

        // Choose parser.
        let parser = match lang {
            Lang::Rust => &mut rust_parser,
            Lang::C => &mut c_parser,
        };

        // Parse.
        let tree = match parser.parse(&source, None) {
            Some(t) => t,
            None => {
                parse_errors += 1;
                continue;
            }
        };

        // Compute relative path for the FILE field.
        let rel_path = relative_path(&args.repo_path, path);

        // Walk tree and write entries.
        match walk_tree(&mut state, &tree, &source, &rel_path, lang) {
            Ok(_counts) => {
                files_parsed += 1;
            }
            Err(e) => {
                eprintln!("Error writing entries for {}: {e}", rel_path);
                parse_errors += 1;
            }
        }

        // Progress indicator every 100 files.
        if files_parsed % 100 == 0 && files_parsed > 0 {
            let secs = start.elapsed().as_secs_f64();
            let eps = if secs > 0.0 { state.entries_written as f64 / secs } else { 0.0 };
            eprint!(
                "\r  {} files, {} entries, {:.0} entries/sec...        ",
                files_parsed, state.entries_written, eps,
            );
        }
    }

    // Final sync.
    state.log.sync()?;
    let elapsed = start.elapsed();

    // Clear progress line.
    eprint!("\r");

    println!("=== Results ===");
    println!("  Files parsed:     {}", files_parsed);
    println!("  Files skipped:    {}", files_skipped);
    println!("  Parse errors:     {}", parse_errors);
    println!("  Source bytes:     {} ({:.1} MB)", total_bytes, total_bytes as f64 / (1024.0 * 1024.0));
    println!("  Entries written:  {}", state.entries_written);
    println!("  Elapsed:          {:.2?}", elapsed);

    // Count output files and size.
    let (journal_files, journal_bytes) = count_journal_output(&args.output_dir);

    if elapsed.as_secs_f64() > 0.0 {
        let secs = elapsed.as_secs_f64();
        let eps = state.entries_written as f64 / secs;
        let mbps = journal_bytes as f64 / (1024.0 * 1024.0) / secs;
        println!("  Entries/sec:      {:.0}", eps);
        println!("  Throughput:       {:.1} MB/sec", mbps);
    }

    println!("  Journal files:    {}", journal_files);
    println!(
        "  Journal size:     {} ({:.1} MB)",
        journal_bytes,
        journal_bytes as f64 / (1024.0 * 1024.0)
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .is_some_and(|s| s.starts_with('.'))
}

fn relative_path(base: &Path, full: &Path) -> String {
    full.strip_prefix(base)
        .unwrap_or(full)
        .to_string_lossy()
        .into_owned()
}

fn count_journal_output(dir: &Path) -> (usize, u64) {
    let mut files = 0;
    let mut bytes = 0u64;

    for entry in WalkDir::new(dir).into_iter().flatten() {
        if entry.file_type().is_file() {
            if let Some(ext) = entry.path().extension() {
                if ext == "journal" {
                    files += 1;
                    bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
        }
    }

    (files, bytes)
}
