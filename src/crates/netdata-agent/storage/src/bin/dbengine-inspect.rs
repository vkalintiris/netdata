//! `dbengine-inspect [--json|--totals|--rebuild-v2|--dump <uuid>] <cache-dir|tier-dir>...`: reads dbengine files
//! and reports what they hold (`netdata_agent_storage::dbengine::format::inspect`). The exit status is 1 when a check
//! fails (the extents' zero padding excepted) or a rebuilt v2 file differs, 2 on a usage or I/O error.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use netdata_agent_storage::dbengine::format::inspect;

const USAGE: &str =
    "usage: dbengine-inspect [--json|--totals|--rebuild-v2|--dump <uuid>] <cache-dir|tier-dir>...";

enum Mode {
    Summary,
    Json,
    Totals,
    RebuildV2,
    Dump([u8; 16]),
}

fn parse_uuid(s: &str) -> Option<[u8; 16]> {
    let hex: Vec<u8> = s.bytes().filter(|&b| b != b'-').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (o, pair) in out.iter_mut().zip(hex.chunks(2)) {
        *o = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

fn run(mode: &Mode, dir: &Path) -> std::io::Result<bool> {
    let mut ok = true;
    match mode {
        Mode::Json => {
            for (_, tier) in inspect::tier_dirs(dir) {
                let report = inspect::tier_report(&tier)?;
                let files = report["files"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                ok &= files.iter().all(|f| inspect::failed_checks(f).is_empty());
                println!("{report:#}");
            }
        }
        Mode::Totals => println!("{:#}", inspect::cache_totals(dir)?),
        Mode::RebuildV2 => {
            for (_, tier) in inspect::tier_dirs(dir) {
                for r in inspect::rebuild_v2(&tier)? {
                    ok &= r.identical();
                    println!("{}", r.line());
                }
            }
        }
        Mode::Dump(uuid) => {
            let mut out = Vec::new();
            for (_, tier) in inspect::tier_dirs(dir) {
                out.push(format!("{}:", tier.display()));
                inspect::dump(&tier, uuid, &mut out)?;
            }
            println!("{}", out.join("\n"));
        }
        Mode::Summary => {
            for (t, tier) in inspect::tier_dirs(dir) {
                let report = inspect::tier_report(&tier)?;
                let rebuilds = inspect::rebuild_v2(&tier)?;
                println!("{}", tier.display());
                for f in report["files"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                {
                    let njf = format!("journalfile-1-{}.njf", f["fileno"].as_str().unwrap_or(""));
                    let rebuild = rebuilds
                        .iter()
                        .find(|r| r.njf.file_name().is_some_and(|n| *n == *njf));
                    ok &= inspect::failed_checks(f).is_empty()
                        && rebuild.is_none_or(|r| r.identical());
                    println!("{}", inspect::file_line(f, rebuild));
                }
                println!("{}", inspect::tier_line(t, &inspect::tier_totals(&tier)?));
            }
        }
    }
    Ok(ok)
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut mode = Mode::Summary;
    let mut dirs = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => mode = Mode::Json,
            "--totals" => mode = Mode::Totals,
            "--rebuild-v2" => mode = Mode::RebuildV2,
            "--dump" => match args.next().as_deref().and_then(parse_uuid) {
                Some(uuid) => mode = Mode::Dump(uuid),
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if arg.starts_with('-') => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
            _ => dirs.push(PathBuf::from(arg)),
        }
    }
    if dirs.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let mut ok = true;
    for dir in &dirs {
        match run(&mode, dir) {
            Ok(dir_ok) => ok &= dir_ok,
            Err(e) => {
                eprintln!("dbengine-inspect: {}: {e}", dir.display());
                return ExitCode::from(2);
            }
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
