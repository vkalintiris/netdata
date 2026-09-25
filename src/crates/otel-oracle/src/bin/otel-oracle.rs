//! The live tier-2 comparison: `otel_oracle::run` against the lab agent, the
//! `otel-tee` capture in front of it and the agent's traces store.
//!
//! `compare` asks the agent's `otel-traces` Function the tier-2 questions
//! while the tee is frozen and judges the answers; `ingest` only checks that
//! the store holds what the capture says was sent. The sanitised report goes
//! to `--evidence-dir`; the raw answers, the requests and the alias map stay in
//! `--run-dir`, which must be private.
//!
//! The freeze file is removed when the run ends, when it fails, on a panic
//! and on SIGINT or SIGTERM; a SIGKILL leaves it, and the tee then refuses
//! exports until it is removed by hand.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::{Value, json};

use otel_oracle::ingest::IngestWindow;
use otel_oracle::membership::DEFAULT_CHUNK_ENTRIES;
use otel_oracle::run::{self, AskError, Config, Lab, Outcome};
use otel_oracle::tier2::{Plan, Request};

#[derive(Parser)]
#[command(name = "otel-oracle")]
#[command(about = "Compare the lab agent's traces answers with the reference calculator")]
struct Args {
    #[command(subcommand)]
    mode: Mode,
    /// The otel-tee capture file.
    #[arg(long, global = true)]
    capture: Option<PathBuf>,
    /// The agent's traces store (`<run>/lib/otel/traces`).
    #[arg(long, global = true)]
    store: Option<PathBuf>,
    /// The tee's freeze file.
    #[arg(long, global = true)]
    freeze_file: Option<PathBuf>,
    /// Private directory for the raw answers, the requests and the alias map.
    #[arg(long, global = true)]
    run_dir: Option<PathBuf>,
    /// Where the sanitised report.md and summary.json go.
    #[arg(long, global = true)]
    evidence_dir: Option<PathBuf>,
    /// The chunk minimum the agent runs with.
    #[arg(long, global = true, default_value_t = DEFAULT_CHUNK_ENTRIES)]
    chunk_entries: u32,
    /// The longest the tee may stay frozen, in seconds.
    #[arg(long, global = true, default_value_t = 150)]
    longest_freeze_s: u64,
    /// A commit to name in the report, as `name=hash`; repeatable.
    #[arg(long = "commit", global = true)]
    commits: Vec<String>,
}

#[derive(Subcommand)]
enum Mode {
    /// Ask the tier-2 questions and judge the answers.
    Compare {
        /// The agent's web address.
        #[arg(long, default_value = "http://127.0.0.1:29999")]
        agent: String,
        /// A file holding the agent's bearer token; it must not be readable by
        /// group or others.
        #[arg(long)]
        bearer_file: PathBuf,
        /// Per-request timeout, in seconds.
        #[arg(long, default_value_t = 60)]
        timeout_s: u64,
    },
    /// Check only that the store holds what the capture says was sent.
    Ingest,
    /// Count what the capture holds (records, spans, fields per span); prints
    /// counts only.
    Stats,
}

struct Agent {
    client: reqwest::Client,
    url: String,
    bearer: String,
}

struct LiveLab {
    runtime: tokio::runtime::Handle,
    agent: Option<Agent>,
    freeze_file: PathBuf,
    started: Instant,
}

impl Lab for LiveLab {
    fn now_ns(&self) -> u64 {
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
    }

    fn monotonic(&self) -> Duration {
        self.started.elapsed()
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn set_frozen(&mut self, frozen: bool) -> std::io::Result<()> {
        if frozen {
            std::fs::File::create(&self.freeze_file).map(drop)
        } else {
            unfreeze(&self.freeze_file)
        }
    }

    fn ask(&mut self, _plan: &Plan, request: &Request) -> Result<Value, AskError> {
        let Some(agent) = &self.agent else {
            return Err(AskError::Refused("no agent to ask".to_string()));
        };
        let body = serde_json::to_vec(&request.body)
            .map_err(|e| AskError::Failed(format!("request body: {e}")))?;
        let sent = agent
            .client
            .post(&agent.url)
            .header(AUTHORIZATION, format!("Bearer {}", agent.bearer))
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        let answer = self.runtime.block_on(async {
            let response = sent.send().await?;
            let status = response.status();
            let bytes = response.bytes().await?;
            Ok::<_, reqwest::Error>((status, bytes))
        });
        let (status, bytes) = match answer {
            Ok(answer) => answer,
            Err(e) if e.is_connect() => {
                return Err(AskError::Refused(format!("agent unreachable: {e}")));
            }
            Err(e) => return Err(AskError::Failed(e.to_string())),
        };
        if matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::PRECONDITION_FAILED
        ) {
            return Err(AskError::Refused(format!("HTTP {status}")));
        }
        if !status.is_success() {
            let text = String::from_utf8_lossy(&bytes);
            let head: String = text.chars().take(300).collect();
            return Err(AskError::Failed(format!("HTTP {status}: {head}")));
        }
        serde_json::from_slice(&bytes).map_err(|e| AskError::Failed(format!("answer: {e}")))
    }
}

fn unfreeze(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Removes the freeze file when dropped, a panic's unwinding included.
struct FreezeGuard(PathBuf);

impl Drop for FreezeGuard {
    fn drop(&mut self) {
        if let Err(e) = unfreeze(&self.0) {
            eprintln!(
                "otel-oracle: could not remove {}: {e}; the tee refuses exports until it is removed",
                self.0.display()
            );
        }
    }
}

fn read_bearer(path: &Path) -> Result<String, String> {
    let mode = std::fs::metadata(path)
        .map_err(|e| format!("bearer file: {e}"))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(format!(
            "the bearer file is readable beyond its owner (mode {:o}); chmod 600 it",
            mode & 0o777
        ));
    }
    let bearer = std::fs::read_to_string(path).map_err(|e| format!("bearer file: {e}"))?;
    let bearer = bearer.trim().to_string();
    if bearer.is_empty() {
        return Err("the bearer file is empty".to_string());
    }
    Ok(bearer)
}

fn required(value: &Option<PathBuf>, flag: &str) -> Result<PathBuf, String> {
    value.clone().ok_or_else(|| format!("--{flag} is required"))
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The private record of a run: every request and answer, the unanswered
/// requests and why, and the alias map.
fn write_private(dir: &Path, outcome: &Outcome, aliases: &Value) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    for window in &outcome.windows {
        let Some(plan) = &window.plan else {
            continue;
        };
        let mut asked = serde_json::Map::new();
        for request in &plan.requests {
            asked.insert(
                request.id.clone(),
                json!({
                    "body": request.body,
                    "answer": window.answers.get(&request.id),
                }),
            );
        }
        let file = dir.join(format!("asked-{}.json", window.window.name));
        write_json(&file, &Value::Object(asked))?;
    }
    let failed: Vec<Value> = outcome
        .failed
        .iter()
        .map(|(id, why)| json!({"request": id, "why": why}))
        .collect();
    write_json(&dir.join("unanswered.json"), &json!(failed))?;
    write_json(&dir.join("aliases.json"), aliases)
}

fn clean(outcome: &Outcome) -> bool {
    outcome.gave_up.is_none()
        && outcome.failed.is_empty()
        && !outcome.windows.is_empty()
        && outcome
            .windows
            .iter()
            .all(|w| w.check.judged() && w.findings.is_empty())
}

/// Streams the capture and prints its shape: no stored value is printed.
fn stats(args: &Args) -> Result<(), String> {
    let path = required(&args.capture, "capture")?;
    let file = std::fs::File::open(&path).map_err(|e| format!("capture: {e}"))?;
    let mut reader = otel_oracle::capture::Reader::new(std::io::BufReader::new(file), None)
        .map_err(|e| format!("capture: {e}"))?;
    let mut records: BTreeMap<(&str, bool), u64> = BTreeMap::new();
    let (mut spans, mut fields, mut values, mut widest) = (0u64, 0u64, 0u64, 0usize);
    let mut names = std::collections::BTreeSet::new();
    let mut received: Option<(i64, i64)> = None;
    while let Some(record) = reader.next_record().map_err(|e| format!("capture: {e}"))? {
        let at = record.received_unix_ns;
        received = Some(received.map_or((at, at), |(first, last)| (first.min(at), last.max(at))));
        let signal = match record.signal {
            otel_oracle::capture::Signal::Traces => "traces",
            otel_oracle::capture::Signal::Logs => "logs",
        };
        *records.entry((signal, record.acknowledged())).or_default() += 1;
        if signal != "traces" || !record.acknowledged() {
            continue;
        }
        let Ok(request) = record.traces() else {
            continue;
        };
        for span in otel_oracle::model::spans_of_request(&request, 0) {
            spans += 1;
            fields += span.fields.len() as u64;
            widest = widest.max(span.fields.len());
            for (name, set) in &span.fields {
                values += set.len() as u64;
                if !names.contains(name.as_str()) {
                    names.insert(name.clone());
                }
            }
        }
    }
    for ((signal, ok), count) in &records {
        let answer = if *ok { "ok" } else { "not ok" };
        println!("records {signal} {answer}: {count}");
    }
    let minutes = received.map_or(0.0, |(first, last)| (last - first) as f64 / 60e9);
    println!("minutes: {minutes:.1}");
    println!("spans in ok traces records: {spans}");
    if spans > 0 && minutes > 0.0 {
        println!("spans per minute: {:.0}", spans as f64 / minutes);
        println!(
            "fields per span: {:.1} (widest {widest})",
            fields as f64 / spans as f64
        );
        println!("values per span: {:.1}", values as f64 / spans as f64);
    }
    println!("distinct field names: {}", names.len());
    if let Some(cut) = reader.cut() {
        println!("a last record being written: {cut} bytes");
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = Args::parse();
    if matches!(args.mode, Mode::Stats) {
        return match stats(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(why) => {
                eprintln!("otel-oracle: {why}");
                ExitCode::from(2)
            }
        };
    }
    match live(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(why) => {
            eprintln!("otel-oracle: {why}");
            ExitCode::from(2)
        }
    }
}

/// Runs and writes the report; `Ok(true)` when every window was judged with
/// no finding.
fn live(args: &Args) -> Result<bool, String> {
    let capture = required(&args.capture, "capture")?;
    let store = required(&args.store, "store")?;
    let freeze_file = required(&args.freeze_file, "freeze-file")?;
    let run_dir = required(&args.run_dir, "run-dir")?;
    let evidence_dir = required(&args.evidence_dir, "evidence-dir")?;
    let mut commits = BTreeMap::new();
    for commit in &args.commits {
        let (name, hash) = commit
            .split_once('=')
            .ok_or_else(|| format!("--commit {commit:?} is not name=hash"))?;
        commits.insert(name.to_string(), hash.to_string());
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    let on_signal = freeze_file.clone();
    runtime.spawn(async move {
        let Ok(mut term) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
        if let Err(e) = unfreeze(&on_signal) {
            eprintln!("otel-oracle: could not remove {}: {e}", on_signal.display());
        }
        eprintln!("otel-oracle: stopped by a signal; the freeze file is removed");
        std::process::exit(130);
    });

    let (agent, ask) = match &args.mode {
        Mode::Compare {
            agent,
            bearer_file,
            timeout_s,
        } => {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(*timeout_s))
                .build()
                .map_err(|e| format!("http client: {e}"))?;
            let agent = Agent {
                client,
                url: format!(
                    "{}/api/v3/function?function=otel-traces",
                    agent.trim_end_matches('/')
                ),
                bearer: read_bearer(bearer_file)?,
            };
            (Some(agent), true)
        }
        Mode::Ingest | Mode::Stats => (None, false),
    };
    let config = Config {
        capture,
        store,
        chunk_entries: args.chunk_entries,
        ingest: IngestWindow::LAB,
        longest_freeze: Duration::from_secs(args.longest_freeze_s),
        ask,
    };
    let mut lab = LiveLab {
        runtime: runtime.handle().clone(),
        agent,
        freeze_file: freeze_file.clone(),
        started: Instant::now(),
    };

    let outcome = {
        let _guard = FreezeGuard(freeze_file);
        run::run(&mut lab, &config).map_err(|e| e.to_string())?
    };
    let report = run::report(&outcome, commits, 20);
    let aliases = report
        .as_ref()
        .map_or(Value::Null, |report| report.aliases.clone());
    write_private(&run_dir, &outcome, &aliases)?;
    let report = report.map_err(|leaked| {
        format!(
            "the report would show {} stored value(s); nothing written to the evidence directory",
            leaked.len()
        )
    })?;
    std::fs::create_dir_all(&evidence_dir)
        .map_err(|e| format!("{}: {e}", evidence_dir.display()))?;
    std::fs::write(evidence_dir.join("report.md"), &report.markdown)
        .map_err(|e| format!("report.md: {e}"))?;
    write_json(&evidence_dir.join("summary.json"), &report.summary)?;
    eprintln!(
        "otel-oracle: {} attempt(s), frozen {} ms; report in {}",
        outcome.attempts,
        outcome.frozen.as_millis(),
        evidence_dir.display()
    );
    Ok(clean(&outcome))
}
