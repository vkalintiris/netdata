//! The declared collection plan: one `Item` per capture, built by the
//! platform modules from tier-1 discovery facts, executed by `Ctx::execute`.
//!
//! Simple sources (`Cmd`, `File`, `Api`, `CmdRaw`, `Generated`) are plain
//! data so `--list` can print exactly what will run. `Native` is the single
//! escape hatch for composites (fallback chains, filters, multi-command
//! concatenation); its `origin` string describes the pipeline and is what
//! `--list` shows.

use crate::collect::Ctx;
use crate::consts::{API_CAP, FILE_CAP};
use crate::manifest::ManifestKind;
use std::path::PathBuf;

/// Facts the declaration builders need from the CLI options; a subset of
/// `Ctx` so `--list` can build the plan without a collection context.
pub struct PlanOpts {
    pub since_hours: u64,
    /// Windows declares no obfuscation-dependent items today; the field is
    /// read only by the POSIX builders.
    #[cfg_attr(windows, allow(dead_code))]
    pub obfuscate: bool,
}

/// One declared capture. `rel` is the bundle-relative output path; `title`
/// lands in the manifest (and the provenance header for headered kinds).
pub struct Item {
    pub rel: String,
    pub title: String,
    /// Progress line printed before this item runs (set on the first item
    /// of each section so per-section narration survives declare/execute).
    pub announce: Option<String>,
    pub source: Source,
}

/// Attach a progress line to the first item of a section's batch.
pub fn announce_first(items: &mut [Item], msg: &str) {
    if let Some(first) = items.first_mut() {
        first.announce = Some(msg.to_string());
    }
}

/// Attach a progress line to the first item a section pushed onto a shared
/// plan (`start` = the plan length before the section ran). A section that
/// declared nothing announces nothing (deliberate no-op when
/// `start == items.len()`).
pub fn announce_at(items: &mut [Item], start: usize, msg: &str) {
    if let Some(first) = items.get_mut(start) {
        first.announce = Some(msg.to_string());
    }
}

// Declaration sugar mirroring the old Ctx::collect_* signatures so the
// platform builders read like the plan they declare. The Item::* constructors
// are the canonical cross-platform API; a push_* helper is cfg-gated to the
// platforms that currently call it (remove the gate when the other platform
// grows a caller - an unconditional helper would be dead code there today).
#[cfg(unix)]
pub fn push_cmd(v: &mut Vec<Item>, rel: &str, title: &str, argv: &[&str]) {
    v.push(Item::cmd(rel, title, argv));
}

pub fn push_file(v: &mut Vec<Item>, rel: &str, title: &str, src: impl Into<PathBuf>) {
    v.push(Item::file(rel, title, src));
}

pub fn push_file_capped(
    v: &mut Vec<Item>,
    rel: &str,
    title: &str,
    src: impl Into<PathBuf>,
    cap: u64,
) {
    v.push(Item::file_capped(rel, title, src, cap));
}

pub fn push_api(v: &mut Vec<Item>, rel: &str, title: &str, url_path: &str) {
    v.push(Item::api(rel, title, url_path));
}

#[cfg(unix)]
pub fn push_generated(v: &mut Vec<Item>, rel: &str, title: &str, content: impl Into<String>) {
    v.push(Item::generated(rel, title, content));
}

pub fn push_native(
    v: &mut Vec<Item>,
    rel: &str,
    title: &str,
    origin: &str,
    produce: impl FnOnce(&Ctx) -> String + 'static,
) {
    v.push(Item::native(rel, title, origin, produce));
}

pub fn push_native_capped(
    v: &mut Vec<Item>,
    rel: &str,
    title: &str,
    origin: &str,
    cap: u64,
    produce: impl FnOnce(&Ctx) -> String + 'static,
) {
    v.push(Item::native_capped(rel, title, origin, cap, produce));
}

pub enum Source {
    /// Run argv; provenance header + exit/duration trailer; line-aligned
    /// head truncation at `cap`.
    Cmd { argv: Vec<String>, cap: u64 },
    /// Copy a file; line-aligned tail when over `cap`; symlinked leaf
    /// withheld.
    File { src: PathBuf, cap: u64 },
    /// GET from the local agent API; no header; withheld whole on overflow.
    Api { url_path: String },
    /// Run argv with NO header/trailer (output must stay parseable);
    /// withheld whole on overflow; dropped when empty.
    CmdRaw { argv: Vec<String> },
    /// Tool-generated marker/instruction text. Never deadline-skipped.
    Generated { content: String },
    /// Composite produced in-process. Prefer named producer functions over
    /// inline closures so the plan stays greppable and testable.
    Native {
        origin: String,
        cap: u64,
        produce: Box<dyn FnOnce(&Ctx) -> String>,
    },
}

impl Item {
    pub fn cmd(rel: &str, title: &str, argv: &[&str]) -> Item {
        Item::cmd_capped(rel, title, argv, API_CAP)
    }

    pub fn cmd_capped(rel: &str, title: &str, argv: &[&str], cap: u64) -> Item {
        debug_assert!(!argv.is_empty(), "Cmd item {rel} declared with empty argv");
        Item {
            rel: rel.to_string(),
            title: title.to_string(),
            announce: None,
            source: Source::Cmd {
                argv: argv.iter().map(|s| s.to_string()).collect(),
                cap,
            },
        }
    }

    pub fn file(rel: &str, title: &str, src: impl Into<PathBuf>) -> Item {
        Item::file_capped(rel, title, src, FILE_CAP)
    }

    pub fn file_capped(rel: &str, title: &str, src: impl Into<PathBuf>, cap: u64) -> Item {
        Item {
            rel: rel.to_string(),
            title: title.to_string(),
            announce: None,
            source: Source::File {
                src: src.into(),
                cap,
            },
        }
    }

    pub fn api(rel: &str, title: &str, url_path: &str) -> Item {
        Item {
            rel: rel.to_string(),
            title: title.to_string(),
            announce: None,
            source: Source::Api {
                url_path: url_path.to_string(),
            },
        }
    }

    pub fn cmd_raw(rel: &str, title: &str, argv: &[&str]) -> Item {
        debug_assert!(
            !argv.is_empty(),
            "CmdRaw item {rel} declared with empty argv"
        );
        Item {
            rel: rel.to_string(),
            title: title.to_string(),
            announce: None,
            source: Source::CmdRaw {
                argv: argv.iter().map(|s| s.to_string()).collect(),
            },
        }
    }

    pub fn generated(rel: &str, title: &str, content: impl Into<String>) -> Item {
        Item {
            rel: rel.to_string(),
            title: title.to_string(),
            announce: None,
            source: Source::Generated {
                content: content.into(),
            },
        }
    }

    pub fn native(
        rel: &str,
        title: &str,
        origin: &str,
        produce: impl FnOnce(&Ctx) -> String + 'static,
    ) -> Item {
        Item::native_capped(rel, title, origin, API_CAP, produce)
    }

    pub fn native_capped(
        rel: &str,
        title: &str,
        origin: &str,
        cap: u64,
        produce: impl FnOnce(&Ctx) -> String + 'static,
    ) -> Item {
        Item {
            rel: rel.to_string(),
            title: title.to_string(),
            announce: None,
            source: Source::Native {
                origin: origin.to_string(),
                cap,
                produce: Box::new(produce),
            },
        }
    }

    /// The manifest kind this item will register as (also shown by --list).
    pub fn kind(&self) -> ManifestKind {
        match &self.source {
            Source::Cmd { .. } | Source::CmdRaw { .. } | Source::Native { .. } => ManifestKind::Cmd,
            Source::File { .. } | Source::Generated { .. } => ManifestKind::File,
            Source::Api { .. } => ManifestKind::Api,
        }
    }

    /// What --list prints as the item's source.
    pub fn describe_source(&self) -> String {
        match &self.source {
            Source::Cmd { argv, .. } => argv.join(" "),
            Source::File { src, .. } => src.display().to_string(),
            Source::Api { url_path } => format!("GET localhost:19999{url_path}"),
            Source::CmdRaw { argv } => format!("{} (raw)", argv.join(" ")),
            Source::Generated { .. } => "generated".to_string(),
            Source::Native { origin, .. } => origin.clone(),
        }
    }
}
