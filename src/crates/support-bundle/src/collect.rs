//! The collection context and the collection primitives. They enforce
//! timeouts, size caps, line-aligned truncation, sanitization, and manifest
//! registration — collectors never write into the bundle directly, and the
//! private fields keep the compiler enforcing that rule.

use crate::consts::{API_CAP, ND_PORT, TOOL_VERSION};
use crate::manifest::{Manifest, ManifestKind, ManifestMeta};
use crate::run::{CaptureMode, CmdOutput, run_capped};
use crate::sanitize::{MapRow, Sanitizer};
use crate::util;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How many distinct skip reasons the tally keeps verbatim; beyond this only
/// the count grows (a hostile tree must not grow the summary without bound).
const SKIP_DETAIL_CAP: usize = 50;

pub struct Ctx {
    /// bundle root inside the staging dir (WORK)
    work: PathBuf,
    sanitizer: Sanitizer,
    manifest: Manifest,
    started: Instant,
    deadline: Duration,
    cmd_timeout: Duration,
    /// silently-dropped captures, surfaced in summary.txt so an incomplete
    /// bundle is explainable (graceful degradation is a contract, invisible
    /// degradation is not)
    skipped: Vec<String>,
    skipped_total: usize,
}

impl Ctx {
    pub fn new(work: PathBuf, sanitizer: Sanitizer, cmd_timeout_secs: u64) -> Self {
        Ctx {
            work,
            sanitizer,
            manifest: Manifest::default(),
            started: Instant::now(),
            deadline: Duration::from_secs(crate::consts::GLOBAL_DEADLINE_SECS),
            cmd_timeout: Duration::from_secs(cmd_timeout_secs),
            skipped: Vec::new(),
            skipped_total: 0,
        }
    }

    // --- read-only facts collectors may use --------------------------------

    pub fn work(&self) -> &Path {
        &self.work
    }

    pub fn cmd_timeout(&self) -> Duration {
        self.cmd_timeout
    }

    pub fn obfuscate(&self) -> bool {
        self.sanitizer.obfuscate()
    }

    pub fn runtime_seconds(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// Checked before each collector; one in-flight command may overrun by up
    /// to the command timeout, so the hard runtime bound is deadline + timeout.
    pub fn deadline_exceeded(&self) -> bool {
        util::interrupted() || self.started.elapsed() >= self.deadline
    }

    // --- the narrow sanitizer/manifest surface main.rs needs ----------------

    /// Pre-seed a child/mirrored hostname for stable pseudonymization.
    pub fn seed_fqdn(&mut self, host: &str) {
        self.sanitizer.seed_fqdn(host);
    }

    /// Sanitize bytes produced outside the collection primitives
    /// (MANIFEST.json is the only such content) so they too are written
    /// sanitized-only, never raw-then-rewritten.
    pub fn sanitize_external_bytes(&mut self, raw: &[u8]) -> (Vec<u8>, Option<&'static str>) {
        self.sanitizer.sanitize_bytes(raw)
    }

    pub fn pseudonym_rows(&self) -> &[MapRow] {
        self.sanitizer.map_rows()
    }

    pub fn emit_manifest(&self, meta: &ManifestMeta) -> String {
        self.manifest.emit(meta)
    }

    /// The tally of captures that were dropped without a bundle marker.
    pub fn skipped(&self) -> (&[String], usize) {
        (&self.skipped, self.skipped_total)
    }

    fn record_skip(&mut self, rel: &str, reason: &str) {
        self.skipped_total += 1;
        if self.skipped.len() < SKIP_DETAIL_CAP {
            self.skipped.push(format!("{rel}: {reason}"));
        }
    }

    // --- internals -----------------------------------------------------------

    fn out_path(&mut self, rel: &str) -> Option<PathBuf> {
        // rel paths are collector-constructed; refuse traversal outright as
        // defense in depth for any future dynamically-built path. has_root
        // matters on Windows, where "/foo" is rooted but not "absolute"
        if rel.split(['/', '\\']).any(|c| c == "..")
            || Path::new(rel).has_root()
            || Path::new(rel).is_absolute()
        {
            self.record_skip(rel, "refused: path traversal in bundle path");
            return None;
        }
        let out = self.work.join(rel);
        if let Some(parent) = out.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                self.record_skip(rel, &format!("cannot create bundle dir: {e}"));
                return None;
            }
        }
        Some(out)
    }

    /// The single write sink for bundle content: sanitize IN MEMORY, write
    /// once, register in the manifest. Raw (pre-sanitization) bytes never
    /// touch disk — a crash mid-run cannot leave unsanitized content in the
    /// staging tree.
    fn finish_write(
        &mut self,
        out: &Path,
        rel: &str,
        kind: ManifestKind,
        origin: &str,
        title: &str,
        body: impl AsRef<[u8]>,
    ) {
        let (sanitized, withheld) = self.sanitizer.sanitize_bytes(body.as_ref());
        if let Some(reason) = withheld {
            // the marker file explains itself, but the operator triaging an
            // incomplete capture needs the rel/reason breadcrumb on stderr
            util::info(&format!("content withheld for {rel}: {reason}"));
        }
        if let Err(e) = std::fs::write(out, &sanitized) {
            self.record_skip(rel, &format!("cannot write capture: {e}"));
            return;
        }
        let pii = self.obfuscate();
        self.manifest
            .add(rel, kind, origin, title, sanitized.len() as u64, pii);
    }

    // --- collection primitives ----------------------------------------------

    /// Command capture with a single-line provenance header and an
    /// exit/duration trailer; output is truncated at LINE boundaries so a
    /// secret can never straddle the cut and dodge the line-based sanitizer.
    fn collect_cmd_capped(&mut self, rel: &str, title: &str, argv: &[&str], cap: u64) {
        let cmdline = flatten_cmdline(&argv.join(" "));
        self.collect_body(rel, title, &cmdline, cap, |ctx| {
            run_capped(argv, ctx.cmd_timeout, (cap * 4) as usize, CaptureMode::Head)
        });
    }

    /// Command capture whose body is produced natively (fallback chains,
    /// filters). `origin` describes the pipeline for the header/manifest.
    fn collect_native_capped(
        &mut self,
        rel: &str,
        title: &str,
        origin: &str,
        cap: u64,
        f: impl FnOnce(&Ctx) -> String,
    ) {
        self.collect_body(rel, title, origin, cap, |ctx| {
            let started = std::time::Instant::now();
            // a panicking producer loses ITS capture, never the run - the
            // same degradation contract every other failure mode follows
            // (&Ctx is a shared borrow, so no state is torn mid-panic).
            // CAVEAT: under a panic="abort" build profile catch_unwind is a
            // no-op and a producer panic aborts the process - ship this tool
            // with the unwinding release profile
            let (output, exit_desc) =
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx))) {
                    Ok(text) => (text.into_bytes(), "0".to_string()),
                    Err(_) => {
                        util::info(&format!("producer failed for {rel} - content withheld"));
                        (
                            b"[content withheld: producer failed]\n".to_vec(),
                            "producer-panic".to_string(),
                        )
                    }
                };
            CmdOutput {
                output,
                exit_desc,
                duration_secs: started.elapsed().as_secs(),
                timed_out: false,
            }
        });
    }

    fn collect_body(
        &mut self,
        rel: &str,
        title: &str,
        origin: &str,
        cap: u64,
        f: impl FnOnce(&Ctx) -> CmdOutput,
    ) {
        let Some(out) = self.out_path(rel) else {
            return;
        };
        if self.deadline_exceeded() {
            // headered captures leave a visible marker; the file/raw/api
            // primitives instead tally into summary.txt (a marker would
            // masquerade as content there) - the asymmetry is deliberate
            let title = format!("{title} (skipped: deadline)");
            self.finish_write(
                &out,
                rel,
                ManifestKind::Cmd,
                "skipped",
                &title,
                "SKIPPED: global deadline reached\n",
            );
            return;
        }
        let r = f(self);
        let raw_size = r.output.len() as u64;
        let mut text = String::from_utf8_lossy(&r.output).into_owned();
        // a timeout can cut mid-line; drop the unterminated final segment so
        // the line-based sanitizer never sees a half record
        if r.timed_out && !text.ends_with('\n') {
            match text.rfind('\n') {
                Some(nl) => text.truncate(nl + 1),
                None => text.clear(),
            }
        }
        let mut body = String::with_capacity(text.len().min(cap as usize) + 256);
        body.push_str(&format!(
            "# netdata-support-bundle v{TOOL_VERSION} | command: {origin} | captured: {}\n",
            util::utc_now_iso()
        ));
        body.push_str(&truncate_line_aligned(&text, cap as usize));
        if raw_size > cap {
            body.push_str(&format!(
                "### TRUNCATED: output was {raw_size} bytes, first {cap} kept (line-aligned) ###\n"
            ));
        }
        body.push_str(&format!(
            "# exit: {} | duration: {}s\n",
            r.exit_desc, r.duration_secs
        ));
        self.finish_write(&out, rel, ManifestKind::Cmd, origin, title, body);
    }

    /// Copy a file with the default 1 MiB cap (or an explicit one). Oversized
    /// files keep their line-aligned tail; a symlinked LEAF is withheld so a
    /// swapped link cannot redirect collection (symlinked parent directories
    /// resolve normally).
    fn collect_file_capped(&mut self, rel: &str, title: &str, src: &Path, cap: u64) {
        use std::io::{Read, Seek, SeekFrom};
        if self.deadline_exceeded() {
            self.record_skip(rel, "deadline reached");
            return;
        }
        // pre-check keeps the "missing file is silent, symlink is loud"
        // distinction; on unix the open below re-verifies race-free
        let Ok(meta) = src.symlink_metadata() else {
            return; // a missing source is normal degradation, not a skip
        };
        if meta.file_type().is_symlink() {
            self.withhold_symlink(rel, title, src);
            return;
        }
        if !meta.is_file() {
            return;
        }
        // open ONCE and do everything through the descriptor: no path re-lookup
        // between check and read, and never more than ~cap bytes in memory
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            // open the reparse point itself (never its target); the handle
            // attributes are checked below, closing the check-to-open race
            use std::os::windows::fs::OpenOptionsExt;
            opts.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
        }
        let mut file = match opts.open(src) {
            Ok(f) => f,
            #[cfg(unix)]
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
                // the leaf became a symlink between lstat and open
                self.withhold_symlink(rel, title, src);
                return;
            }
            Err(e) => {
                self.record_skip(rel, &format!("cannot open source: {e}"));
                return;
            }
        };
        let Ok(fmeta) = file.metadata() else {
            self.record_skip(rel, "cannot stat source");
            return;
        };
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            if fmeta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                self.withhold_symlink(rel, title, src);
                return;
            }
        }
        if !fmeta.is_file() {
            return;
        }
        let size = fmeta.len();
        let Some(out) = self.out_path(rel) else {
            return;
        };
        let origin;
        let body: Vec<u8>;
        if size > cap {
            // cap at a LINE boundary (drop the first, possibly partial, line)
            if file.seek(SeekFrom::Start(size - cap)).is_err() {
                self.record_skip(rel, "cannot seek source");
                return;
            }
            let mut tail = Vec::with_capacity(cap as usize);
            if let Err(e) = file.take(cap).read_to_end(&mut tail) {
                self.record_skip(rel, &format!("cannot read source: {e}"));
                return;
            }
            match tail.iter().position(|&b| b == b'\n') {
                Some(nl) if nl + 1 < tail.len() => body = tail[nl + 1..].to_vec(),
                _ => {
                    // the whole tail is one giant line: withhold rather than
                    // risk a mid-token cut hiding a secret
                    body = b"[content withheld: file tail exceeds the cap without a line break]\n"
                        .to_vec();
                }
            }
            origin = format!(
                "{} (last ~{cap} of {size} bytes, line-aligned)",
                src.display()
            );
        } else {
            let mut buf = Vec::with_capacity(size as usize);
            // bounded even if the file grows between fstat and read
            if let Err(e) = file.take(cap + 1).read_to_end(&mut buf) {
                self.record_skip(rel, &format!("cannot read source: {e}"));
                return;
            }
            if buf.len() as u64 > cap {
                // grew past the cap mid-read: keep the line-aligned head
                let text = String::from_utf8_lossy(&buf).into_owned();
                buf = truncate_line_aligned(&text, cap as usize).into_bytes();
            }
            body = buf;
            origin = src.display().to_string();
        }
        self.finish_write(&out, rel, ManifestKind::File, &origin, title, body);
    }

    fn withhold_symlink(&mut self, rel: &str, title: &str, src: &Path) {
        let Some(out) = self.out_path(rel) else {
            return;
        };
        let origin = format!("{} (symlink, withheld)", src.display());
        self.finish_write(
            &out,
            rel,
            ManifestKind::File,
            &origin,
            title,
            "[content withheld: source is a symlink]\n",
        );
    }

    /// Like collect_cmd but with NO provenance header/trailer, for output that
    /// must stay parseable (JSON). Oversized output is withheld whole (a
    /// truncated JSON document is worse than none); removed when empty.
    fn collect_cmd_raw(&mut self, rel: &str, title: &str, argv: &[&str]) {
        if self.deadline_exceeded() {
            self.record_skip(rel, "deadline reached");
            return;
        }
        let cmdline = flatten_cmdline(&argv.join(" "));
        let r = run_capped(
            argv,
            self.cmd_timeout,
            (API_CAP + 1) as usize,
            CaptureMode::Head,
        );
        let body = if r.timed_out {
            // a killed command can stop mid-record: fail closed
            b"{\"error\":\"command timed out and its partial output was withheld\"}\n".to_vec()
        } else if r.output.len() as u64 > API_CAP {
            b"{\"error\":\"output exceeded the cap and was withheld\"}\n".to_vec()
        } else {
            r.output
        };
        if body.is_empty() {
            // empty output means the tool had nothing to say - expected
            // degradation, not a tallied skip
            return;
        }
        let Some(out) = self.out_path(rel) else {
            return;
        };
        self.finish_write(&out, rel, ManifestKind::Cmd, &cmdline, title, body);
    }

    /// GET from the local agent API; response is sanitized without a header
    /// (stays parseable), withheld whole on overflow, dropped when empty or
    /// on HTTP failure.
    fn collect_api(&mut self, rel: &str, title: &str, url_path: &str) {
        if self.deadline_exceeded() {
            self.record_skip(rel, "deadline reached");
            return;
        }
        let resp =
            crate::http::local_get(ND_PORT, url_path, self.cmd_timeout, (API_CAP + 1) as usize);
        let body = match resp {
            // the runtime section only runs when the API probe succeeded, so
            // a per-endpoint failure here is unexpected and worth tallying
            Err(e) => {
                self.record_skip(rel, &format!("API request failed: {e}"));
                return;
            }
            Ok(r) if !(200..300).contains(&r.status) => {
                self.record_skip(rel, &format!("API returned HTTP {}", r.status));
                return;
            }
            Ok(r) => {
                if r.capped || !r.complete || r.body.len() as u64 > API_CAP {
                    // capped or timeout-cut bodies may end mid-record: a
                    // truncated JSON document is worse than none
                    b"{\"error\":\"response exceeded the cap or was cut short and was withheld\"}\n"
                        .to_vec()
                } else {
                    r.body
                }
            }
        };
        if body.is_empty() {
            // an empty body (endpoint absent on this agent) is expected
            // degradation, not a tallied skip
            return;
        }
        let Some(out) = self.out_path(rel) else {
            return;
        };
        self.finish_write(&out, rel, ManifestKind::Api, url_path, title, body);
    }

    /// Write a generated marker/instruction file. The content is tool-made,
    /// but it may embed discovered paths (a config dir under a user's home),
    /// so it goes through the sanitizer like everything else.
    pub fn write_generated(&mut self, rel: &str, title: &str, content: &str) {
        let Some(out) = self.out_path(rel) else {
            return;
        };
        self.finish_write(&out, rel, ManifestKind::File, "generated", title, content);
    }

    // --- the executor ---------------------------------------------------------

    /// Run a declared plan in list order. Deadline handling lives in the
    /// per-kind bodies (headered captures leave a SKIPPED marker, the others
    /// tally; Generated markers are never deadline-skipped) — the loop adds
    /// no behavior of its own.
    pub fn execute(&mut self, items: Vec<crate::item::Item>) {
        for item in items {
            self.execute_item(item);
        }
    }

    fn execute_item(&mut self, item: crate::item::Item) {
        use crate::item::Source;
        if let Some(msg) = &item.announce {
            util::info(msg);
        }
        let rel = item.rel;
        let title = item.title;
        match item.source {
            Source::Cmd { argv, cap } => {
                let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
                self.collect_cmd_capped(&rel, &title, &argv, cap);
            }
            Source::File { src, cap } => self.collect_file_capped(&rel, &title, &src, cap),
            Source::Api { url_path } => self.collect_api(&rel, &title, &url_path),
            Source::CmdRaw { argv } => {
                let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
                self.collect_cmd_raw(&rel, &title, &argv);
            }
            Source::Generated { content } => self.write_generated(&rel, &title, &content),
            Source::Native {
                origin,
                cap,
                produce,
            } => self.collect_native_capped(&rel, &title, &origin, cap, produce),
        }
    }
}

/// Keep command headers single-line even for multi-line pipelines. On unix,
/// backslashes are shell line continuations and are removed; on Windows they
/// are path separators and survive.
fn flatten_cmdline(s: &str) -> String {
    util::flatten_single_line(s, cfg!(unix))
}

/// Accumulate whole lines until the byte budget is hit (the awk
/// `b += length($0) + 1; if (b > cap) exit` logic).
fn truncate_line_aligned(text: &str, cap: usize) -> String {
    let mut out = String::new();
    let mut budget = 0usize;
    for line in text.split_inclusive('\n') {
        let stripped = line.strip_suffix('\n').unwrap_or(line);
        budget += stripped.len() + 1;
        if budget > cap {
            break;
        }
        out.push_str(stripped);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx(work: PathBuf) -> Ctx {
        let id = crate::sanitize::Identity::gated("testhost99", "", "testuser9");
        let sanitizer = crate::sanitize::Sanitizer::new(true, id);
        Ctx::new(work, sanitizer, 5)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("support-bundle-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn oversized_file_keeps_line_aligned_tail() {
        let dir = scratch("tail");
        let src = dir.join("big.log");
        // ~200 KiB of numbered lines; only the tail may survive a 1000-byte cap
        let mut content = String::new();
        for i in 0..10_000 {
            content.push_str(&format!("line number {i} with some padding\n"));
        }
        std::fs::write(&src, &content).unwrap();
        let mut ctx = test_ctx(dir.join("work"));
        ctx.collect_file_capped("05-logs/big.log", "t", &src, 1000);
        let out = std::fs::read_to_string(dir.join("work/05-logs/big.log")).unwrap();
        assert!(out.len() <= 1001, "tail exceeds cap: {}", out.len());
        assert!(
            out.starts_with("line number "),
            "first line is partial: {out:?}"
        );
        assert!(out.ends_with("line number 9999 with some padding\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_leaf_is_withheld() {
        let dir = scratch("symlink");
        let target = dir.join("target.txt");
        std::fs::write(&target, "secret-target-content\n").unwrap();
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut ctx = test_ctx(dir.join("work"));
        ctx.collect_file_capped("04-config/link.txt", "t", &link, crate::consts::FILE_CAP);
        let out = std::fs::read_to_string(dir.join("work/04-config/link.txt")).unwrap();
        assert!(out.contains("content withheld"), "{out:?}");
        assert!(!out.contains("secret-target-content"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn giant_single_line_tail_is_withheld() {
        let dir = scratch("giantline");
        let src = dir.join("one-line.log");
        std::fs::write(&src, "x".repeat(100_000)).unwrap();
        let mut ctx = test_ctx(dir.join("work"));
        ctx.collect_file_capped("05-logs/one-line.log", "t", &src, 1000);
        let out = std::fs::read_to_string(dir.join("work/05-logs/one-line.log")).unwrap();
        assert!(out.contains("content withheld"), "{out:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn oversized_raw_output_is_withheld_whole() {
        let dir = scratch("rawcap");
        let mut ctx = test_ctx(dir.join("work"));
        // > API_CAP bytes of output must be replaced by the error document
        ctx.collect_cmd_raw(
            "07-runtime/big.json",
            "t",
            &["sh", "-c", "head -c 3000000 /dev/zero | tr '\\0' 'a'"],
        );
        let out = std::fs::read_to_string(dir.join("work/07-runtime/big.json")).unwrap();
        assert!(out.contains("exceeded the cap"), "{out:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deadline_skip_leaves_marker_and_manifest_row() {
        let dir = scratch("deadline");
        let mut ctx = test_ctx(dir.join("work"));
        ctx.deadline = Duration::from_secs(0);
        ctx.collect_cmd_capped("01-system/late.txt", "too late", &["true"], API_CAP);
        let out = std::fs::read_to_string(dir.join("work/01-system/late.txt")).unwrap();
        assert!(out.contains("SKIPPED: global deadline reached"), "{out:?}");
        let meta = ManifestMeta {
            generated_utc: String::new(),
            runtime_seconds: 0,
            pii_obfuscated: true,
            agent_running: false,
            agent_api_reachable: false,
            is_container: false,
        };
        let doc: serde_json::Value = serde_json::from_str(&ctx.emit_manifest(&meta)).unwrap();
        let row = &doc["files"][0];
        assert_eq!(row["origin"], "skipped");
        assert!(row["title"].as_str().unwrap().contains("skipped: deadline"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn traversal_and_absolute_paths_are_refused_and_tallied() {
        let dir = scratch("traversal");
        let mut ctx = test_ctx(dir.join("work"));
        ctx.write_generated("../escape.txt", "t", "x");
        ctx.write_generated("/abs/escape.txt", "t", "x");
        assert!(!dir.join("escape.txt").exists());
        let (details, total) = ctx.skipped();
        assert_eq!(total, 2);
        assert!(details[0].contains("path traversal"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn line_aligned_truncation() {
        let text = "aaaa\nbbbb\ncccc\n";
        assert_eq!(truncate_line_aligned(text, 10), "aaaa\nbbbb\n");
        assert_eq!(truncate_line_aligned(text, 4), "");
        assert_eq!(truncate_line_aligned(text, 100), text);
    }

    #[test]
    fn cmdline_flattening() {
        let expected = if cfg!(unix) { "a b cd" } else { "a b c\\d" };
        assert_eq!(flatten_cmdline("a  b\n\tc\\d"), expected);
    }

    // --- executor tests (declared items through Ctx::execute) ---------------

    #[test]
    fn executor_runs_each_source_kind_and_registers_manifest_rows() {
        let dir = scratch("exec-kinds");
        let src = dir.join("input.conf");
        std::fs::write(&src, "plain line\n").unwrap();
        let mut ctx = test_ctx(dir.join("work"));
        let items = vec![
            crate::item::Item::cmd("01-system/echo.txt", "t-cmd", &["echo", "hi"]),
            crate::item::Item::file("04-config/input.conf", "t-file", &src),
            crate::item::Item::generated("05-logs/note.txt", "t-gen", "a note\n"),
            crate::item::Item::native("01-system/native.txt", "t-nat", "test pipeline", |_| {
                "native body\n".to_string()
            }),
            // raw command with empty output: dropped silently, no file
            crate::item::Item::cmd_raw("07-runtime/empty.json", "t-raw", &["true"]),
            // API endpoint that cannot succeed: tallied skip, no file
            crate::item::Item::api("07-runtime/probe.json", "t-api", "/api/v3/nonexistent"),
        ];
        ctx.execute(items);
        for rel in [
            "01-system/echo.txt",
            "04-config/input.conf",
            "05-logs/note.txt",
            "01-system/native.txt",
        ] {
            assert!(dir.join("work").join(rel).is_file(), "missing {rel}");
        }
        let manifest = ctx.emit_manifest(&ManifestMeta {
            generated_utc: "t".into(),
            runtime_seconds: 0,
            pii_obfuscated: true,
            agent_running: false,
            agent_api_reachable: false,
            is_container: false,
        });
        for rel in ["01-system/echo.txt", "04-config/input.conf"] {
            assert!(manifest.contains(rel), "manifest lacks {rel}");
        }
        // CmdRaw with empty output is dropped without a file or a tally;
        // the unreachable/404 API endpoint tallies a skip without a file
        assert!(!dir.join("work/07-runtime/empty.json").exists());
        assert!(!dir.join("work/07-runtime/probe.json").exists());
        let (_, total) = ctx.skipped();
        assert_eq!(total, 1, "only the API probe should tally");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn executor_deadline_asymmetry_marker_vs_tally() {
        let dir = scratch("exec-deadline");
        let src = dir.join("late.conf");
        std::fs::write(&src, "x\n").unwrap();
        let mut ctx = test_ctx(dir.join("work"));
        ctx.deadline = Duration::from_secs(0);
        let items = vec![
            crate::item::Item::cmd("01-system/late-cmd.txt", "late cmd", &["true"]),
            crate::item::Item::file("04-config/late.conf", "late file", &src),
            crate::item::Item::generated("05-logs/marker.txt", "gen survives", "still here\n"),
        ];
        ctx.execute(items);
        // headered capture leaves a visible marker file
        let cmd_out = std::fs::read_to_string(dir.join("work/01-system/late-cmd.txt")).unwrap();
        assert!(cmd_out.contains("SKIPPED: global deadline reached"));
        // file capture tallies without creating output
        assert!(!dir.join("work/04-config/late.conf").exists());
        let (details, total) = ctx.skipped();
        assert_eq!(total, 1, "exactly the file item tallies: {details:?}");
        assert!(details[0].contains("late.conf"));
        // Generated markers are never deadline-skipped
        let note = std::fs::read_to_string(dir.join("work/05-logs/marker.txt")).unwrap();
        assert!(note.contains("still here"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn executor_sink_writes_only_sanitized_bytes() {
        // the security invariant the refactor added: content is sanitized in
        // memory and ONLY sanitized bytes ever reach the staging tree
        let dir = scratch("exec-sanitize");
        let mut ctx = test_ctx(dir.join("work"));
        let items = vec![
            crate::item::Item::generated(
                "06-state/gen.txt",
                "t",
                "api_token = super-secret-value-123\n",
            ),
            crate::item::Item::native("06-state/nat.txt", "t", "test", |_| {
                "password=hunter2-abc\nclean line\n".to_string()
            }),
        ];
        ctx.execute(items);
        let redacted = std::fs::read_to_string(dir.join("work/06-state/gen.txt")).unwrap();
        assert!(!redacted.contains("super-secret-value-123"), "{redacted:?}");
        assert!(redacted.contains("[REDACTED]"), "{redacted:?}");
        let nat = std::fs::read_to_string(dir.join("work/06-state/nat.txt")).unwrap();
        assert!(!nat.contains("hunter2-abc"), "{nat:?}");
        assert!(nat.contains("clean line"), "{nat:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn executor_survives_panicking_producer_and_withholds() {
        let dir = scratch("exec-panic");
        let mut ctx = test_ctx(dir.join("work"));
        ctx.execute(vec![
            crate::item::Item::native("06-state/boom.txt", "t", "test", |_| {
                panic!("producer exploded")
            }),
            crate::item::Item::generated("06-state/after.txt", "t", "still running\n"),
        ]);
        // the panicking item is withheld fail-closed, the run continues
        let boom = std::fs::read_to_string(dir.join("work/06-state/boom.txt")).unwrap();
        assert!(boom.contains("content withheld"), "{boom:?}");
        let after = std::fs::read_to_string(dir.join("work/06-state/after.txt")).unwrap();
        assert!(after.contains("still running"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
