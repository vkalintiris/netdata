//! The external plugins, ported from `src/plugins.d/plugins_d.c` and `pluginsd_process()` (`pluginsd_parser.c`): the
//! `PLUGINSD` thread scans the plugins directories and starts a `PD[<name>]` thread per plugin it finds enabled, which
//! runs the plugin through the spawn server, feeds its output to a plugin parser, and starts it again, or gives up on
//! it, by C's policy. At the exit `PLUGINSD` cancels its plugin threads and joins them.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use netdata_agent_ingest::{Config as ParserConfig, Parser, PluginHosts};
use netdata_agent_inicfg::{Config, SECTION_PLUGINS};

use crate::server::Shared;
use netdata_agent_log::{Field, Priority, Source, Value, nd_log, netdata_log_error, netdata_log_info, push};
use netdata_agent_pluginsd_proto::{LINE_MAX, LineReader};
use netdata_agent_rrd::host::Host;
use netdata_agent_spawn::popen::Popen;
use nix::errno::Errno;
use nix::poll::PollFlags;

use crate::shutdown;

/// `SERIAL_FAILURES_THRESHOLD`: runs without data after which a plugin is given up on.
const SERIAL_FAILURES_THRESHOLD: u64 = 10;
/// `ND_CHECK_CANCELLABILITY_WHILE_WAITING_EVERY_MS`.
const CHECK_EVERY: Duration = Duration::from_millis(100);
/// `buffered_reader_read_timeout(…, 2 * 60 * MSEC_PER_SEC, …)`: how long a plugin may stay silent.
const READ_TIMEOUT_MS: i64 = 120_000;
/// `spawn_popen_kill(pi, 3 * MSEC_PER_SEC)`.
const KILL_TIMEOUT_MS: i32 = 3_000;
/// `ND_THREAD_TAG_MAX`.
const THREAD_TAG_MAX: usize = 15;
/// `char module[100]`: the log's module field of a plugin.
const MODULE_MAX: usize = 99;
/// `CONFIG_MAX_NAME`: a plugin's name is cut to it.
const CONFIG_MAX_NAME: usize = 1024;
/// `PLUGINSD_CMD_MAX` (`FILENAME_MAX * 2`): a command is cut to it.
const CMD_MAX: usize = 8192;
/// `FILENAME_MAX`: a plugin's path is cut to it.
const FILENAME_MAX: usize = 4096;
/// `NETDATA_THREAD_TAG_MAX`: the tag a failed thread creation names is cut to it.
const TAG_BUFFER_MAX: usize = 99;
/// The plugins this agent replaced (`is_obsolete_plugin()`).
const OBSOLETE: [&[u8]; 1] = [b"otel-signal-viewer"];

/// What the plugins thread starts with.
pub struct Settings {
    /// `plugin_directories[]`, as `[directories] plugins` listed them (at most `PLUGINSD_MAX_DIRECTORIES`).
    pub dirs: Vec<String>,
    /// `threads.thread_stack_size`, for every thread here.
    pub stack_size: usize,
    /// `localhost->rrd_update_every`: a plugin's default `update every`.
    pub update_every: i32,
    /// The plugins' parser settings but the update every, which is each plugin's.
    pub parser: ParserConfig,
    /// Localhost and the vnodes the plugins define.
    pub hosts: PluginHosts,
}

/// `cd->unsafe`: what a plugin's thread shares with the scanner and its cleanup.
#[derive(Debug, Default)]
struct State {
    enabled: AtomicBool,
    running: AtomicBool,
    /// `nd_thread_signal_cancel()` of the plugin's thread.
    cancelled: AtomicBool,
}

/// `struct plugind` as the scanner keeps it.
struct Plugind {
    id: String,
    filename: Vec<u8>,
    state: Arc<State>,
    thread: Option<JoinHandle<()>>,
}

/// A started `PLUGINSD` thread.
pub struct Pluginsd {
    thread: JoinHandle<()>,
    /// `service_wait_exit(SERVICE_COLLECTORS, …)`'s cancellation of every collector thread: the scanner and each plugin
    /// thread.
    collectors_cancelled: Arc<AtomicBool>,
}

impl Pluginsd {
    /// The exit's `service_wait_exit(SERVICE_COLLECTORS | …)`: every collector thread cancelled, then `PLUGINSD`, which
    /// stops and joins its plugin threads, waited for until `deadline`; still running, it is given back for the
    /// remaining threads' wait.
    pub fn stop_by(self, deadline: Instant) -> Option<Pluginsd> {
        self.collectors_cancelled.store(true, Ordering::Release);
        while Instant::now() < deadline && !self.thread.is_finished() {
            std::thread::sleep(Duration::from_millis(50));
        }
        if self.thread.is_finished() {
            let _ = self.thread.join();
            return None;
        }
        Some(self)
    }
}

/// Starts `PLUGINSD` (`pluginsd_main()`), which reads its settings from `netdata.conf` on the thread, as C does.
pub fn spawn(shared: Arc<Shared>, settings: Settings) -> std::io::Result<Pluginsd> {
    let collectors_cancelled = Arc::new(AtomicBool::new(false));
    let thread = {
        let collectors_cancelled = Arc::clone(&collectors_cancelled);
        std::thread::Builder::new()
            .name("PLUGINSD".into())
            .stack_size(settings.stack_size)
            .spawn(move || {
                netdata_agent_log::thread_created();
                let localhost = Arc::clone(settings.hosts.hosts.localhost());
                let mut scanner = Scanner { localhost, shared, settings, collectors_cancelled, plugins: Vec::new() };
                scanner.main();
                scanner.cleanup();
                netdata_agent_log::thread_finished();
            })
            .map_err(|err| netdata_agent_evloop::thread_create_failed("PLUGINSD", &err))?
    };
    Ok(Pluginsd { thread, collectors_cancelled })
}

/// `pluginsd_sleep()`: up to `seconds` in 100 ms steps, until `running()` turns false.
fn sleep(seconds: i64, running: &dyn Fn() -> bool) {
    let mut left_ms = seconds.saturating_mul(1000);
    while left_ms > 0 {
        if !running() {
            break;
        }
        std::thread::sleep(CHECK_EVERY);
        left_ms -= CHECK_EVERY.as_millis() as i64;
    }
}

/// `is_plugin()`: the plugin's name, when the file name ends with a plugin suffix after at least one byte.
fn plugin_name(filename: &[u8]) -> Option<&[u8]> {
    [&b".plugin"[..], b"_plugin", b"-plugin"]
        .into_iter()
        .find(|suffix| filename.len() > suffix.len() && filename.ends_with(suffix))
        .map(|suffix| {
            let name = &filename[..filename.len() - suffix.len()];
            &name[..name.len().min(CONFIG_MAX_NAME)]
        })
}

/// What `cd->cmd` holds: `exec <path> <update every> <options>`, cut to `PLUGINSD_CMD_MAX`.
fn command(fullfilename: &[u8], update_every: i32, options: &[u8]) -> Vec<u8> {
    let mut cmd = b"exec ".to_vec();
    cmd.extend_from_slice(fullfilename);
    cmd.extend_from_slice(format!(" {update_every} ").as_bytes());
    cmd.extend_from_slice(options);
    cmd.truncate(CMD_MAX);
    cmd
}

/// `text` cut to `max` bytes, as C's fixed buffers cut it, at a character boundary (the cut of a multi-byte name differs
/// from C's).
fn cut(mut text: String, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}

fn text(v: &[u8]) -> String {
    String::from_utf8_lossy(v).into_owned()
}

/// The `PLUGINSD` thread.
struct Scanner {
    localhost: Arc<Host>,
    /// `netdata.conf`, under its lock.
    shared: Arc<Shared>,
    settings: Settings,
    collectors_cancelled: Arc<AtomicBool>,
    /// `pluginsd_root`: newest first.
    plugins: Vec<Plugind>,
}

impl Scanner {
    /// `service_running(SERVICE_COLLECTORS)` of this thread.
    fn running(&self) -> bool {
        !shutdown::exiting() && !self.collectors_cancelled.load(Ordering::Acquire)
    }

    fn conf(&self) -> std::sync::MutexGuard<'_, Config> {
        self.shared.netdata_conf.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `pluginsd_main()`.
    fn main(&mut self) {
        let (automatic_run, scan_frequency) = {
            let mut conf = self.conf();
            let automatic_run = conf.get_boolean(SECTION_PLUGINS, "enable running new plugins", true);
            let scan_frequency = conf.get_duration_seconds(SECTION_PLUGINS, "check for new plugins every", 60).max(1);
            // disabled by default
            conf.get_boolean(SECTION_PLUGINS, "slabinfo", false);
            // it crashes on Alpine since it is multi-threaded, unless given the device (C)
            if std::env::var_os("NETDATA_LISTENER_PORT").is_some() {
                conf.get_boolean(SECTION_PLUGINS, "freeipmi", false);
            }
            (automatic_run, scan_frequency)
        };
        // each directory's last error, so that a broken one is reported once per change
        let mut directory_errors = vec![0; self.settings.dirs.len()];
        let mut obsolete_logged = false;
        while self.running() {
            let dirs = self.settings.dirs.clone();
            for (idx, directory) in dirs.iter().enumerate() {
                if !self.running() {
                    break;
                }
                let entries = match std::fs::read_dir(directory) {
                    Ok(entries) => entries,
                    Err(err) => {
                        let errno = netdata_agent_log::errno_of(&err);
                        if directory_errors[idx] != errno {
                            directory_errors[idx] = errno;
                            nd_log!(Source::Daemon, Priority::Err, errno = errno;
                                "cannot open plugins directory '{directory}'");
                        }
                        continue;
                    }
                };
                for entry in entries {
                    if !self.running() {
                        break;
                    }
                    let Ok(entry) = entry else { break };
                    let filename = std::os::unix::ffi::OsStrExt::as_bytes(entry.file_name().as_os_str()).to_vec();
                    let Some(name) = plugin_name(&filename) else { continue };
                    if OBSOLETE.contains(&name) {
                        // at info level once, then at debug level
                        if !obsolete_logged {
                            obsolete_logged = true;
                            netdata_log_info!(
                                "skipping obsolete plugin '{}'; the current agent already provides its function",
                                text(&filename)
                            );
                        }
                        continue;
                    }
                    let name = text(name);
                    let enabled = self.conf().get_boolean(SECTION_PLUGINS, &name, automatic_run);
                    if !enabled {
                        continue;
                    }
                    self.found(directory, filename, &name);
                }
            }
            sleep(scan_frequency, &|| self.running());
        }
    }

    /// A plugin file found enabled: a new one is started; one that runs, or that ran and ended, is not again.
    fn found(&mut self, directory: &str, filename: Vec<u8>, name: &str) {
        if let Some(cd) = self.plugins.iter_mut().find(|cd| cd.filename == filename) {
            if !cd.state.running.load(Ordering::Acquire)
                && let Some(thread) = cd.thread.take()
            {
                // it gave up
                cd.state.cancelled.store(true, Ordering::Release);
                let _ = thread.join();
            }
            return;
        }
        // char buf[CONFIG_MAX_NAME]
        let id = cut(format!("plugin:{name}"), CONFIG_MAX_NAME - 1);
        let mut fullfilename = format!("{directory}/").into_bytes();
        fullfilename.extend_from_slice(&filename);
        fullfilename.truncate(FILENAME_MAX);
        let (update_every, options) = {
            let mut conf = self.conf();
            // C keeps it as an int
            let update_every =
                conf.get_duration_seconds(&id, "update every", i64::from(self.settings.update_every)) as i32;
            let options = conf.get(&id, "command options", Some("")).unwrap_or_default();
            (update_every, options)
        };
        let state = Arc::new(State { enabled: AtomicBool::new(true), ..State::default() });
        let worker = Worker {
            hosts: self.settings.hosts.clone(),
            filename: text(&filename).into(),
            fullfilename: text(&fullfilename),
            module: cut(format!("plugins.d[{}]", text(&filename)), MODULE_MAX),
            cmd: command(&fullfilename, update_every, &options),
            update_every,
            parser: ParserConfig { update_every, ..self.settings.parser },
            successful_collections: 0,
            serial_failures: 0,
            pid: 0,
            state: Arc::clone(&state),
            collectors_cancelled: Arc::clone(&self.collectors_cancelled),
        };
        // the thread's tag, PD[<name>], as C cuts it for the thread and for its failure record
        let tag = cut(format!("PD[{name}]"), TAG_BUFFER_MAX);
        let thread = std::thread::Builder::new()
            .name(cut(tag.clone(), THREAD_TAG_MAX))
            .stack_size(self.settings.stack_size)
            .spawn(move || {
                netdata_agent_log::thread_created();
                worker.main();
                netdata_agent_log::thread_finished();
            });
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(err) => {
                netdata_log_error!("{}", netdata_agent_evloop::thread_create_failed(&tag, &err));
                None
            }
        };
        self.plugins.insert(0, Plugind { id, filename, state, thread });
    }

    /// `pluginsd_main_cleanup()`: every plugin thread cancelled and joined, newest first.
    fn cleanup(&mut self) {
        netdata_log_info!("PLUGINSD: cleaning up...");
        let hostname = self.localhost.hostname();
        for mut cd in self.plugins.drain(..) {
            if cd.state.enabled.load(Ordering::Acquire) && cd.state.running.load(Ordering::Acquire) && cd.thread.is_some()
            {
                netdata_log_info!("PLUGINSD: 'host:{hostname}', stopping plugin thread: {}", cd.id);
            }
            if let Some(thread) = cd.thread.take() {
                cd.state.cancelled.store(true, Ordering::Release);
                let _ = thread.join();
            }
        }
        netdata_log_info!("PLUGINSD: cleanup completed.");
    }
}

/// A `PD[<name>]` thread (`pluginsd_worker_thread()`) and its plugin's settings and counters.
struct Worker {
    /// Localhost (the plugin's records' host) and the vnodes it may define.
    hosts: PluginHosts,
    filename: Arc<str>,
    fullfilename: String,
    /// The log's module field: `plugins.d[<file>]`.
    module: String,
    cmd: Vec<u8>,
    update_every: i32,
    parser: ParserConfig,
    successful_collections: u64,
    serial_failures: u64,
    pid: i32,
    state: Arc<State>,
    collectors_cancelled: Arc<AtomicBool>,
}

impl Worker {
    /// The thread's cancellation: by `PLUGINSD`, or by the exit's wait for the collectors.
    fn cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire) || self.collectors_cancelled.load(Ordering::Acquire)
    }

    /// `service_running(SERVICE_COLLECTORS)` of this thread.
    fn running(&self) -> bool {
        !shutdown::exiting() && !self.cancelled()
    }

    fn main(mut self) {
        self.state.running.store(true, Ordering::Release);
        // read at each use, as C's rrdhost_hostname(cd->host): a plugin defining localhost's GUID renames it
        let localhost = Arc::clone(self.hosts.hosts.localhost());
        while self.running() {
            let hostname = localhost.hostname();
            let Some(mut popen) = Popen::run_argv(&[b"/bin/sh".as_slice(), b"-c", &self.cmd]) else {
                netdata_log_error!("PLUGINSD: 'host:{hostname}', cannot popen(\"{}\", \"r\").", text(&self.cmd));
                break;
            };
            self.pid = popen.pid();
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "PLUGINSD: 'host:{hostname}' connected to '{}' running on pid {}",
                self.fullfilename,
                self.pid
            );
            let _frame = push(vec![
                (Field::Module, Value::Txt(self.module.clone())),
                (Field::NidlNode, Value::Txt(hostname.clone())),
                (Field::SrcTransport, Value::txt("pluginsd")),
            ]);
            let (count, retry) = match popen.pipes() {
                (Some(output), Some(input)) => self.process(input, output),
                // pluginsd_process() of a missing descriptor
                _ => {
                    self.state.enabled.store(false, Ordering::Release);
                    (0, false)
                }
            };
            let hostname = localhost.hostname();
            nd_log!(
                Source::Collector,
                Priority::Warning,
                "PLUGINSD: 'host:{hostname}', '{}' (pid {}) disconnected after {count} successful data collections.",
                self.fullfilename,
                self.pid
            );
            let rc = popen.kill(KILL_TIMEOUT_MS, &|| self.cancelled());
            let verdict = verdict(&Run {
                rc,
                retry,
                successful_collections: self.successful_collections,
                serial_failures: self.serial_failures,
                enabled: self.state.enabled.load(Ordering::Acquire),
                hostname: &hostname,
                fullfilename: &self.fullfilename,
                pid: self.pid,
            });
            if let Some((priority, message)) = &verdict.record {
                nd_log!(Source::Daemon, *priority, "{message}");
            }
            if verdict.disable {
                self.state.enabled.store(false, Ordering::Release);
            }
            if verdict.sleep_every != 0 {
                sleep(i64::from(self.update_every) * verdict.sleep_every, &|| self.running());
            }
            self.pid = 0;
            if !self.state.enabled.load(Ordering::Acquire) {
                break;
            }
        }
        self.state.running.store(false, Ordering::Release);
    }

    /// `pluginsd_process()`: the plugin's output parsed line by line until it ends, fails, refuses a line or the
    /// thread stops; then QUIT unless the plugin hung up, the counters, and the charts this thread still holds made
    /// obsolete. Returns the run's data collections and whether to retry.
    fn process(&mut self, input: &mut File, output: &mut File) -> (u64, bool) {
        if !self.state.enabled.load(Ordering::Acquire) {
            return (0, false);
        }
        let mut parser = Parser::plugin(self.hosts.clone(), self.parser, Arc::clone(&self.filename));
        let run = parser.run_frame();
        let mut reader = LineReader::default();
        let mut lines = std::collections::VecDeque::<Vec<u8>>::new();
        let mut buffer = vec![0; LINE_MAX];
        let mut send_quit = true;
        while self.running() {
            if let Some(line) = lines.pop_front() {
                let ok = parser.feed(&line);
                let out = parser.take_output();
                if !out.is_empty() {
                    send_to_plugin(output, &out);
                }
                if !ok {
                    break;
                }
                continue;
            }
            match read(input, &mut buffer, &|| self.cancelled()) {
                Ok(n) => lines.extend(reader.push(&buffer[..n])),
                Err(ret) => {
                    nd_log!(Source::Collector, Priority::Info, "PLUGINSD: buffered reader not OK ({ret})");
                    if ret == READ_POLLERR || ret == READ_POLLHUP {
                        send_quit = false;
                    }
                    break;
                }
            }
        }
        if send_quit {
            nd_log!(Source::Collector, Priority::Debug, "PLUGINSD: sending 'QUIT'  to plugin: {}", self.filename);
            send_to_plugin(output, b"QUIT");
        }
        self.state.enabled.store(parser.enabled, Ordering::Release);
        let (count, retry) = (parser.data_collections_count, parser.retry);
        if count != 0 {
            self.successful_collections += count;
            self.serial_failures = 0;
        } else if !retry {
            self.serial_failures += 1;
        }
        parser.vnodes_offline();
        // every chart of this plugin is obsolete
        let localhost = self.hosts.hosts.localhost();
        localhost.charts().obsolete_created_by(localhost, netdata_agent_log::tid());
        drop(parser);
        drop(run);
        (count, retry)
    }
}

/// `BUFFERED_READER_READ_*`: a read that failed.
const READ_FAILED: i32 = -1;
const READ_POLLERR: i32 = -3;
const READ_POLLHUP: i32 = -4;
const READ_POLLNVAL: i32 = -5;
const READ_POLL_UNKNOWN: i32 = -6;
const READ_POLL_TIMEOUT: i32 = -7;
const READ_POLL_CANCELLED: i32 = -8;

/// `buffered_reader_read_timeout(…, 120 s, true)`: the bytes read, or C's code after its record. The wait ends early
/// only when the thread is cancelled.
fn read(input: &mut File, buffer: &mut [u8], cancelled: &dyn Fn() -> bool) -> Result<usize, i32> {
    let waited = netdata_agent_sys::wait_fd(input.as_fd(), READ_TIMEOUT_MS, PollFlags::POLLIN, cancelled);
    match waited.rc {
        0 => loop {
            match input.read(buffer) {
                Ok(0) => return Err(READ_FAILED),
                Ok(n) => return Ok(n),
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err(READ_FAILED),
            }
        },
        1 => {
            nd_log!(Source::Daemon, Priority::Err, errno = waited.errno; "PARSER: timeout while waiting for data.");
            Err(READ_POLL_TIMEOUT)
        }
        -1 => {
            nd_log!(Source::Daemon, Priority::Err, errno = waited.errno;
                "PARSER: thread cancelled while waiting for data.");
            Err(READ_POLL_CANCELLED)
        }
        _ => {
            let (ret, what) = if waited.revents.contains(PollFlags::POLLERR) {
                (READ_POLLERR, "POLLERR")
            } else if waited.revents.contains(PollFlags::POLLHUP) {
                (READ_POLLHUP, "POLLHUP")
            } else if waited.revents.contains(PollFlags::POLLNVAL) {
                (READ_POLLNVAL, "POLLNVAL")
            } else {
                nd_log!(Source::Daemon, Priority::Err, errno = waited.errno;
                    "PARSER: poll() returned positive number, but POLLIN|POLLERR|POLLHUP|POLLNVAL are not set.");
                return Err(READ_POLL_UNKNOWN);
            };
            nd_log!(Source::Daemon, Priority::Err, errno = waited.errno; "PARSER: read failed: {what}.");
            Err(ret)
        }
    }
}

/// `send_to_plugin()` on the plugin's stdin (`nd_sock_write_persist(…, 100)`): a short result is reported with the
/// failed write's errno.
fn send_to_plugin(output: &mut File, bytes: &[u8]) {
    let (sent, errno) = write_persist(output, bytes, SEND_RETRIES);
    if sent < bytes.len() as isize {
        nd_log!(
            Source::Daemon,
            Priority::Warning,
            errno = errno;
            "PLUGINSD: cannot send command to plugin (fd = {}, sent bytes = {sent} out of {})",
            output.as_raw_fd(),
            bytes.len()
        );
    }
}

/// `send_to_plugin()`'s retries.
const SEND_RETRIES: u32 = 100;

/// `nd_sock_write_persist()`: the bytes written, resuming after short writes at most `retries` more times, or a failed
/// write's result, each with the errno C leaves.
fn write_persist(output: &mut File, bytes: &[u8], retries: u32) -> (isize, i32) {
    let (mut written, mut resumes) = (0, retries);
    loop {
        let (sent, errno) = write_retried(output, &bytes[written..], retries);
        if sent <= 0 {
            return (sent, errno);
        }
        written += sent as usize;
        if written >= bytes.len() || resumes == 0 {
            return (written as isize, 0);
        }
        resumes -= 1;
    }
}

/// `nd_sock_write()` on a pipe: one write, again while it fails with EAGAIN (EWOULDBLOCK) or EINTR, at most `retries`
/// more times.
fn write_retried(output: &mut File, bytes: &[u8], mut retries: u32) -> (isize, i32) {
    loop {
        match output.write(bytes) {
            Ok(n) => return (n as isize, 0),
            Err(err) => {
                let errno = err.raw_os_error().unwrap_or(0);
                if retries == 0 || !matches!(Errno::from_raw(errno), Errno::EAGAIN | Errno::EINTR) {
                    return (-1, errno);
                }
                retries -= 1;
            }
        }
    }
}

/// How a run ended, for [`verdict`].
#[derive(Clone, Copy)]
struct Run<'a> {
    /// `spawn_popen_kill()`'s code: the exit code; 0 for SIGTERM or SIGPIPE; -1 for another signal or a give-up.
    rc: i32,
    retry: bool,
    successful_collections: u64,
    serial_failures: u64,
    enabled: bool,
    hostname: &'a str,
    fullfilename: &'a str,
    pid: i32,
}

/// What the worker does after a run: C's record, then a sleep of `sleep_every` update everies (0: none), or the
/// plugin disabled.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    record: Option<(Priority, String)>,
    sleep_every: i64,
    disable: bool,
}

/// `pluginsd_worker_thread()`'s policy after the kill, with `pluginsd_worker_thread_handle_success()` and
/// `_handle_error()`.
fn verdict(run: &Run<'_>) -> Verdict {
    let Run { rc, retry, successful_collections: collections, serial_failures: serial, enabled, hostname, fullfilename, pid } =
        *run;
    let who = format!("'host:{hostname}', '{fullfilename}' (pid {pid})");
    let go = |record, sleep_every| Verdict { record, sleep_every, disable: false };
    let stop = |record| Verdict { record: Some(record), sleep_every: 0, disable: true };
    if retry && rc != -1 {
        return go(None, 1);
    }
    if rc == 0 {
        if collections != 0 {
            return go(None, 1);
        }
        if serial <= SERIAL_FAILURES_THRESHOLD {
            let then = if enabled {
                "Waiting a bit before starting it again."
            } else {
                "Will not start it again - it is now disabled."
            };
            let message = format!(
                "PLUGINSD: {who} does not generate useful output but it reports success (exits with 0). {then}."
            );
            return go(Some((Priority::Info, message)), 10);
        }
        // with C's stray quote
        return stop((
            Priority::Err,
            format!(
                "PLUGINSD: 'host:'{hostname}', '{fullfilename}' (pid {pid}) does not generate useful output, although it \
                 reports success (exits with 0).We have tried to collect something {serial} times - unsuccessfully. \
                 Disabling it."
            ),
        ));
    }
    if rc == -1 {
        return stop((Priority::Info, format!("PLUGINSD: {who} exited abnormally. Disabling it.")));
    }
    if collections == 0 {
        return stop((
            Priority::Err,
            format!("PLUGINSD: {who} exited with error code {rc} and haven't collected any data. Disabling it."),
        ));
    }
    if serial <= SERIAL_FAILURES_THRESHOLD {
        let then = if enabled {
            "Waiting a bit before starting it again."
        } else {
            "Will not start it again - it is disabled."
        };
        let message = format!(
            "PLUGINSD: {who} exited with error code {rc}, but has given useful output in the past ({collections} times). \
             {then}"
        );
        return go(Some((Priority::Err, message)), 10);
    }
    stop((
        Priority::Err,
        format!(
            "PLUGINSD: {who} exited with error code {rc}, but has given useful output in the past ({collections} \
             times).We tried to restart it {serial} times, but it failed to generate data. Disabling it."
        ),
    ))
}

#[cfg(test)]
mod tests;
