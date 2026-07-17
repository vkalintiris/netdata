//! netdata-support-bundle — one-command sanitized support bundles.
//!
//! Collects a sanitized diagnostic bundle (tarball on POSIX systems, zip on
//! Windows) that users attach to support tickets. See
//! packaging/installer/SUPPORT-BUNDLE.md for the bundle contract.

mod collect;
mod consts;
mod http;
mod item;
mod manifest;
mod netprobe;
mod platform_api;
mod publish;
mod run;
mod runtime;
mod sanitize;
mod selftest;
mod summary;
#[cfg(unix)]
mod unix;
mod util;
#[cfg(windows)]
mod windows;

use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;
use util::info;

#[derive(Parser)]
#[command(
    name = "netdata-support-bundle",
    version = consts::TOOL_VERSION,
    disable_version_flag = true,
    about = "Collect a sanitized Netdata diagnostic bundle"
)]
struct Options {
    /// directory for the final bundle (default: system temp)
    #[arg(short, long, value_name = "DIR")]
    output: Option<PathBuf>,

    /// log collection window in hours
    #[arg(long, value_name = "HOURS", default_value_t = 24,
          value_parser = clap::value_parser!(u64).range(1..))]
    since: u64,

    /// per-command timeout in seconds
    #[arg(long, value_name = "SECS", default_value_t = 10,
          value_parser = clap::value_parser!(u64).range(1..))]
    timeout: u64,

    /// disable PII pseudonymization (secrets are ALWAYS redacted)
    #[arg(long)]
    no_obfuscate: bool,

    /// keep the staging directory for inspection
    #[arg(long)]
    keep_staging: bool,

    /// print the resolved collection plan and exit (runs the same local
    /// discovery as a real run, incl. the local agent API probe - may wait
    /// a few seconds when no agent is running; collects nothing, writes
    /// nothing)
    #[arg(long, visible_alias = "dry-run")]
    list: bool,

    /// run the sanitizer regression suite and exit
    #[arg(long)]
    selftest: bool,

    /// print version and exit
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: (),
}

fn default_output() -> PathBuf {
    if cfg!(windows) {
        std::env::temp_dir()
    } else {
        PathBuf::from("/tmp")
    }
}

fn plan_opts(opts: &Options) -> item::PlanOpts {
    item::PlanOpts {
        since_hours: opts.since,
        obfuscate: !opts.no_obfuscate,
    }
}

/// --list / --dry-run: run tier-1 discovery, print the resolved plan, exit.
/// Discovery includes detect_env's local agent API probe (127.0.0.1 only,
/// ~3s timeout per endpoint when the agent is down); it never seeds
/// hostnames and never creates staging.
fn print_plan(opts: &Options) {
    use std::io::Write;
    let env = platform::detect_env();
    let plan = platform::build_items(&env, &plan_opts(opts));
    let out = std::io::stdout();
    let mut out = out.lock();
    // a closed pipe (`--list | head`) ends the listing, it is not an error
    let mut w = move || -> std::io::Result<()> {
        writeln!(
            out,
            "# netdata-support-bundle {} collection plan (resolved for THIS host; nothing was collected)",
            consts::TOOL_VERSION
        )?;
        writeln!(
            out,
            "# NOTE: this listing shows real local paths and command lines UNsanitized - review before sharing it"
        )?;
        for it in &plan {
            if let Some(a) = &it.announce {
                writeln!(out, "\n== {a} ==")?;
            }
            writeln!(
                out,
                "{:4} {:44} {}",
                it.kind().as_str(),
                it.rel,
                it.describe_source()
            )?;
        }
        writeln!(out, "\n{} items", plan.len())
    };
    let _ = w();
}

#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

fn main() -> ExitCode {
    // self-demotion FIRST: never compete with real workloads
    platform::demote_priority();
    platform::install_signal_handlers();

    let opts = Options::parse();
    if opts.selftest {
        std::process::exit(selftest::run());
    }
    if opts.list {
        print_plan(&opts);
        return ExitCode::SUCCESS;
    }

    match collect_bundle(&opts) {
        Ok(()) => {
            if util::interrupted() {
                ExitCode::from(130)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("netdata-support-bundle: {e}");
            if util::interrupted() {
                ExitCode::from(130)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn collect_bundle(opts: &Options) -> Result<(), String> {
    let staging = publish::Staging::create(opts.keep_staging)
        .map_err(|e| format!("cannot create staging dir: {e}"))?;
    let bundle_name = format!(
        "netdata-support-bundle-{}-{}",
        util::utc_now_compact(),
        std::process::id()
    );
    let work = staging.dir.join(&bundle_name);
    std::fs::create_dir_all(&work).map_err(|e| format!("cannot create work dir: {e}"))?;

    let identity = platform::detect_identity();
    let obfuscate = !opts.no_obfuscate;
    let output = opts.output.clone().unwrap_or_else(default_output);
    let sanitizer = sanitize::Sanitizer::new(obfuscate, identity);
    let mut ctx = collect::Ctx::new(work.clone(), sanitizer, opts.timeout);

    // tier 1: discovery, then the pre-step that must precede every capture
    // (seeding child hostnames so pseudonyms are stable across all files)
    let env = platform::detect_env();
    platform::startup_info(&env);
    if env.api_ok {
        runtime::seed_child_hostnames(&mut ctx);
    }

    // tier 2: declare the plan, then run it
    let plan = platform::build_items(&env, &plan_opts(opts));
    ctx.execute(plan);
    let facts = platform::bundle_facts(&env);

    info("writing summary and manifest");
    summary::write_summary(&mut ctx, &facts.summary);
    summary::write_readme(&mut ctx);
    let meta = manifest::ManifestMeta {
        generated_utc: util::utc_now_iso(),
        runtime_seconds: ctx.runtime_seconds(),
        pii_obfuscated: ctx.obfuscate(),
        agent_running: facts.agent_running,
        agent_api_reachable: facts.api_ok,
        is_container: facts.is_container,
    };
    // emitted LAST so every file (incl. summary.txt and README.md) is
    // indexed. origins/titles embed discovered paths (a config dir under a
    // user's home, command lines); the manifest gets the same in-memory PII
    // pass as the files - sanitized bytes only, never raw-then-rewritten. A
    // sanitize failure degrades to a withheld marker that stays valid JSON
    // (downstream ticket tooling parses this file) and must not abort an
    // otherwise complete bundle.
    let manifest_path = work.join("MANIFEST.json");
    let raw_manifest = ctx.emit_manifest(&meta);
    let (manifest_bytes, withheld) = ctx.sanitize_external_bytes(raw_manifest.as_bytes());
    let manifest_bytes = match withheld {
        None => manifest_bytes,
        Some(reason) => {
            info(&format!(
                "sanitize failed for MANIFEST.json: {reason} - content withheld"
            ));
            format!(
                "{{\"schema\":\"{}\",\"error\":\"manifest content withheld: sanitization failed\",\"files\":[]}}\n",
                consts::SCHEMA
            )
            .into_bytes()
        }
    };
    std::fs::write(&manifest_path, manifest_bytes)
        .map_err(|e| format!("cannot write MANIFEST.json: {e}"))?;

    if util::interrupted() {
        return Err("interrupted - no bundle published".to_string());
    }

    let archive = publish::build_archive(&staging.dir, &work, &bundle_name)
        .map_err(|e| format!("failed to create archive: {e}"))?;
    let published = publish::publish_archive(&archive, &output, &bundle_name)?;

    let map_out = if obfuscate {
        let m = publish::publish_map(ctx.pseudonym_rows(), &output, &bundle_name);
        if m.is_none() && !ctx.pseudonym_rows().is_empty() {
            info(
                "WARNING: could not write the pseudonym map next to the bundle - it was DISCARDED; rerun with --keep-staging if you need it",
            );
        }
        m
    } else {
        None
    };

    let size = std::fs::metadata(&published).map(|m| m.len()).unwrap_or(0);
    eprintln!();
    info(&format!("done in {}s", ctx.runtime_seconds()));
    info(&format!(
        "bundle:  {} ({} bytes)",
        published.display(),
        size
    ));
    if let Some(m) = map_out {
        info(&format!(
            "pseudonym map (KEEP PRIVATE, do not send): {}",
            m.display()
        ));
    }
    if cfg!(windows) {
        info(&format!(
            "review it:  expand the zip and inspect: {}",
            published.display()
        ));
    } else {
        info(&format!(
            "review it:  tar --zstd -tf {0}   (or: zstd -dc {0} | tar -tf -)",
            published.display()
        ));
    }
    if facts.docker_logs_needed {
        info(
            "IMPORTANT: this agent logs to the container's stdout - its log history is NOT in this bundle.",
        );
        info(
            "on the docker HOST also run:  docker logs --since 24h <netdata-container> > netdata-docker.log 2>&1",
        );
        info("and attach netdata-docker.log to the ticket as well.");
    }
    if opts.keep_staging {
        info(&format!("staging kept: {}", staging.dir.display()));
    }
    info("attach the bundle to your support ticket.");
    Ok(())
}
