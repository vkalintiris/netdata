//! The evaluation loop, the alert log and the notifications against C (`tests/oracle/gen-loop-vectors.c`: the
//! tables `units.tsv`, `badge-format.tsv`, `delay.tsv`, `edit.tsv`, `sanitize.tsv` and `decide.tsv`; C's own pass over
//! the scenarios of `loop.tsv`, `queue.tsv` and `sql.tsv`).

mod common;

use common::rows;
use netdata_agent_health::sql::edit_command_from_source;
use netdata_agent_text::units::{format_value_and_unit, format_value_and_unit_precision};

/// A double as the loop's vectors hold it: `nan`, or its bits in hex.
fn double(field: &str) -> f64 {
    match field {
        "nan" => f64::NAN,
        bits => f64::from_bits(u64::from_str_radix(bits, 16).expect("a double's bits")),
    }
}

/// C's text for each value under each unit, as an alert log entry's value texts are made.
#[test]
fn units_match_c() {
    let (mut checked, mut failures) = (0, Vec::new());
    for row in rows("units.tsv") {
        let (value, units) = (double(row.str(0)), row.bytes(1));
        let text = format_value_and_unit(value, units);
        if text != row.bytes(2) {
            failures.push(format!(
                "units.tsv:{}: {value:?} {:?}: C {:?}, Rust {:?}",
                row.line,
                String::from_utf8_lossy(units),
                String::from_utf8_lossy(row.bytes(2)),
                String::from_utf8_lossy(&text)
            ));
        }
        checked += 1;
    }
    let shown = failures[..failures.len().min(20)].join("\n");
    assert!(failures.is_empty(), "{} of {checked} differ:\n{shown}", failures.len());
    assert_eq!(checked, 2784);
}

/// `calculate_delay()` of `sqlite_health.c`: how long the Cloud's queue waits on a status change, for every pair of
/// statuses and a number past them.
#[test]
fn aclk_delays_match_c() {
    let mut checked = 0;
    for row in rows("aclk-delay.tsv") {
        let (from, to) = (row.str(0).parse().expect("a status"), row.str(1).parse().expect("a status"));
        let delay: i64 = row.str(2).parse().expect("a delay");
        let rust = netdata_agent_metadata::health_log::calculate_delay(from, to);
        assert_eq!(rust, delay, "aclk-delay.tsv:{}: {from} to {to}", row.line);
        checked += 1;
    }
    assert_eq!(checked, 64);
}

/// C's text for each value under nine units at twelve precisions, as a badge prints its value (the automatic
/// precision is `units_match_c`'s): a fixed precision keeps every zero and stops at 50 digits, and a negative one
/// other than -1 is automatic too.
#[test]
fn values_with_a_precision_match_c() {
    let (mut checked, mut failures) = (0, Vec::new());
    for row in rows("badge-format.tsv") {
        let (value, units) = (double(row.str(0)), row.bytes(1));
        let precision: i32 = row.str(2).parse().expect("a precision");
        let text = format_value_and_unit_precision(value, units, precision);
        if text != row.bytes(3) {
            failures.push(format!(
                "badge-format.tsv:{}: {value:?} {:?} at {precision}: C {:?}, Rust {:?}",
                row.line,
                String::from_utf8_lossy(units),
                String::from_utf8_lossy(row.bytes(3)),
                String::from_utf8_lossy(&text)
            ));
        }
        checked += 1;
    }
    let shown = failures[..failures.len().min(20)].join("\n");
    assert!(failures.is_empty(), "{} of {checked} differ:\n{shown}", failures.len());
    assert_eq!(checked, 9396);
}

/// C's edit command for each source text of a rule: the new form, the old one with an `@`, and texts of neither.
#[test]
fn edit_commands_match_c() {
    let (mut checked, mut failures) = (0, Vec::new());
    for row in rows("edit.tsv") {
        let command = edit_command_from_source(row.bytes(0), b"/oracle/etc", b"registry-host");
        if command != row.bytes(1) {
            failures.push(format!(
                "edit.tsv:{}: {:?}: C {:?}, Rust {:?}",
                row.line,
                String::from_utf8_lossy(row.bytes(0)),
                String::from_utf8_lossy(row.bytes(1)),
                String::from_utf8_lossy(&command)
            ));
        }
        checked += 1;
    }
    assert!(failures.is_empty(), "{} of {checked} differ:\n{}", failures.len(), failures.join("\n"));
    assert_eq!(checked, 25);
}

/// C's `sanitize_command_argument_string()` over its corpus: every byte value between two letters, dashes, quotes,
/// what a shell would expand, and texts that end around the 8,191 bytes an argument may take.
#[test]
fn arguments_are_sanitized_as_c() {
    use netdata_agent_health::notify::sanitize_command_argument;
    let (mut checked, mut failures) = (0, Vec::new());
    for row in rows("sanitize.tsv") {
        let mut sanitized = Vec::new();
        let fits = sanitize_command_argument(&mut sanitized, row.bytes(0));
        let c_fits = row.str(1) == "1";
        if fits != c_fits || (fits && sanitized != row.bytes(2)) {
            let shown = |bytes: &[u8]| String::from_utf8_lossy(&bytes[..bytes.len().min(60)]).into_owned();
            failures.push(format!(
                "sanitize.tsv:{}: {:?} ({} bytes): C fits {c_fits} {:?}, Rust fits {fits} {:?}",
                row.line,
                shown(row.bytes(0)),
                row.bytes(0).len(),
                shown(row.bytes(2)),
                shown(&sanitized)
            ));
        }
        checked += 1;
    }
    let shown = failures[..failures.len().min(20)].join("\n");
    assert!(failures.is_empty(), "{} of {checked} differ:\n{shown}", failures.len());
    assert_eq!(checked, 337);
}

/// C's `health_send_notification()` over one entry per combination of new status, old status, the four flags its
/// decision reads and what the table answers about the alarm's last executed event: for which entries the table
/// is asked, and which are sent, skipped with which record, or skipped in silence.
#[test]
fn decisions_match_c() {
    use netdata_agent_health::alert::Status;
    use netdata_agent_health::notify::{Decision, decide};
    let statuses = [
        Status::Removed,
        Status::Undefined,
        Status::Uninitialized,
        Status::Clear,
        Status::Raised,
        Status::Warning,
        Status::Critical,
    ];
    let status = |name: &str| *statuses.iter().find(|status| status.name() == name).expect("a status");
    let (mut checked, mut failures) = (0, Vec::new());
    for row in rows("decide.tsv") {
        let (new, old) = (status(row.str(0)), status(row.str(1)));
        let flags = u32::from_str_radix(row.str(2), 16).expect("the flags");
        let answer = match row.str(3) {
            "fail" | "none" => None,
            name => Some(status(name) as i32),
        };
        let asked = std::cell::Cell::new(false);
        let decision = decide(flags, new, old, || {
            asked.set(true);
            answer
        });
        // what C decided, from what it left: a spawn, one of its three records, or nothing (an internal status
        // before the table is asked, a first CLEAR after)
        let (c_asked, spawned, message) = (row.str(4) == "1", row.str(5) == "1", row.str(9));
        let c_decision = if spawned {
            Decision::Send
        } else if message.contains("it has no-clear-notification enabled") {
            Decision::NoClear
        } else if message.contains("Health not sending again notification") {
            Decision::Again
        } else if message.contains("command API has disabled notifications") {
            Decision::Silenced
        } else if c_asked {
            Decision::FirstClear
        } else {
            Decision::Internal
        };
        if decision != c_decision || asked.get() != c_asked {
            failures.push(format!(
                "decide.tsv:{}: {} from {} flags {flags:08x} answer {}: C {c_decision:?} asked {c_asked}, Rust \
                 {decision:?} asked {}",
                row.line,
                row.str(0),
                row.str(1),
                row.str(3),
                asked.get()
            ));
        }
        checked += 1;
    }
    let shown = failures[..failures.len().min(20)].join("\n");
    assert!(failures.is_empty(), "{} of {checked} differ:\n{shown}", failures.len());
    assert_eq!(checked, 7056);
}

/// C's delay multiplier over its grid of delays, multipliers and maxima.
#[test]
fn delays_match_c() {
    let mut checked = 0;
    for row in rows("delay.tsv") {
        let (delay, multiplier, maximum, expected): (i32, f32, i32, i32) =
            (row.num(0), common::float(&row, 1), row.num(2), row.num(3));
        let result = netdata_agent_health::pass::delay_apply_multiplier(delay, multiplier, maximum);
        assert_eq!(result, expected, "delay.tsv:{}: {delay} x {multiplier} max {maximum}", row.line);
        checked += 1;
    }
    assert_eq!(checked, 2574);
}

// ------------------------------------------------------------------------------------------------
// the replay of C's own pass: each scenario of tests/corpus/loop/ is played through the Rust loop with the world
// the C generator stubs (tests/oracle/health-loop-stubs.c), and every row of loop.tsv is compared

mod replay {
    use std::cell::{Cell, RefCell};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    use netdata_agent_health::alert::Status;
    use netdata_agent_health::alerts::HostAlerts;
    use netdata_agent_health::badge::api_v1_badge;
    use netdata_agent_health::entry::Entry;
    use netdata_agent_health::notify::{Execution, Waiting};
    use netdata_agent_health::pass::{ChartFacts, Env, Pass};
    use netdata_agent_health::readfile::health_readfile;
    use netdata_agent_health::store::alert_hash_row;
    use netdata_agent_health::{Health, StoreSink, sql};
    use netdata_agent_log::{Captured, Field, Priority, Source};
    use netdata_agent_metadata::health_log::LoadedRow;
    use netdata_agent_metadata::open::{MetaDb, SqliteSettings};
    use netdata_agent_query::value::{Priority as QueryPriority, ValueRequest, ValueResult};
    use netdata_agent_rrd::chart::{Algorithm, Chart, ChartSpec, ChartType, flags};
    use netdata_agent_rrd::host::{Host, HostInfo, pending_flags};
    use netdata_agent_rrd::labels::SRC_CONFIG;
    use netdata_agent_rrd::mode::DbMode;
    use netdata_agent_text::parse::strtoul0;
    use rusqlite::types::ValueRef;

    use netdata_agent_dyncfg::model::Cmds;
    use netdata_agent_health::config::HealthConfig;
    use netdata_agent_health::json::prototype_to_json;
    use netdata_agent_health::prototype::Rule;
    use netdata_agent_nrpc::reply::{ContentType, Reply};

    use crate::common::{Core, OneHost, answer_fields, oracle_config, rows, unescape_logfmt};

    const T0: i64 = 2_000_000_000;

    /// A row's fields after its scenario and step: the kind, then what the kind holds.
    type Fields = Vec<Vec<u8>>;

    fn text(value: impl ToString) -> Vec<u8> {
        value.to_string().into_bytes()
    }

    /// C's NULL string is a lone NUL byte in a field.
    fn nullable(value: &Option<Vec<u8>>) -> Vec<u8> {
        value.clone().unwrap_or_else(|| vec![0])
    }

    fn double(value: f64) -> Vec<u8> {
        if value.is_nan() { b"nan".to_vec() } else { format!("{:016x}", value.to_bits()).into_bytes() }
    }

    fn hex8(value: u32) -> Vec<u8> {
        format!("{value:08x}").into_bytes()
    }

    /// A transition id as the generator prints one: the count its stub of the random UUID gave it, 0 for none.
    fn uuid_rank(id: &[u8; 16]) -> Vec<u8> {
        text(u128::from_be_bytes(*id))
    }

    /// An alarm the table knows: its chart, its name, its alarm id and its next event id.
    type KnownAlarm = (Vec<u8>, Vec<u8>, u32, u32);

    /// The metadata database of a scenario that says `database real`: the agent's handle on a new file (the
    /// scenario's own statements run on its connection), and a second connection for reading the tables.
    struct Real {
        meta: Arc<MetaDb>,
        raw: rusqlite::Connection,
        _dir: tempfile::TempDir,
    }

    /// The host's id in the tables: its machine GUID's bytes.
    fn host_id(host: &Host) -> [u8; 16] {
        let hex: String = host.machine_guid().chars().filter(|c| *c != '-').collect();
        u128::from_str_radix(&hex, 16).expect("a GUID").to_be_bytes()
    }

    /// C's `%.17g`.
    fn g17(value: f64) -> String {
        if value == 0.0 || !value.is_finite() {
            return format!("{value}");
        }
        let scientific = format!("{value:.16e}");
        let (mantissa, exponent) = scientific.split_once('e').expect("an exponent");
        let exponent: i32 = exponent.parse().expect("an exponent");
        let trimmed = |digits: &str| match digits.contains('.') {
            true => digits.trim_end_matches('0').trim_end_matches('.').to_owned(),
            false => digits.to_owned(),
        };
        if (-4..17).contains(&exponent) {
            trimmed(&format!("{value:.*}", (16 - exponent) as usize))
        } else {
            format!("{}e{}{:02}", trimmed(mantissa), if exponent < 0 { '-' } else { '+' }, exponent.abs())
        }
    }

    /// The hash of every rule of `alert_hash` in rowid order, as a UUID's text.
    fn table_hashes(raw: &rusqlite::Connection) -> Vec<Vec<u8>> {
        let mut statement = raw.prepare("SELECT hash_id FROM alert_hash ORDER BY rowid").expect("the table");
        let hashes = statement.query_map([], |row| row.get::<_, [u8; 16]>(0)).expect("the rows");
        let text = |hash: [u8; 16]| {
            let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
            format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]).into_bytes()
        };
        hashes.map(|hash| text(hash.expect("a hash"))).collect()
    }

    /// Every row of a table in rowid order as the generator prints one: the table, then `column=value` for each
    /// column (an integer as it is, a real with 17 digits, a text in quotes, a blob as hex, `NULL`).
    fn table_rows(raw: &rusqlite::Connection, table: &str) -> Vec<Fields> {
        let mut statement = raw.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).expect("the table");
        let names: Vec<String> = statement.column_names().iter().map(|name| (*name).to_owned()).collect();
        let mut rows = statement.query([]).expect("the rows");
        let mut out = Vec::new();
        while let Some(row) = rows.next().expect("a row") {
            let mut fields = vec![table.as_bytes().to_vec()];
            for (i, name) in names.iter().enumerate() {
                let value = match row.get_ref(i).expect("a column") {
                    ValueRef::Null => b"NULL".to_vec(),
                    ValueRef::Integer(value) => text(value),
                    ValueRef::Real(value) => g17(value).into_bytes(),
                    ValueRef::Text(value) => [b"\"", value, b"\""].concat(),
                    ValueRef::Blob(value) => {
                        let hex: String = value.iter().map(|byte| format!("{byte:02x}")).collect();
                        format!("x'{hex}'").into_bytes()
                    }
                };
                fields.push([name.as_bytes(), b"=", &value].concat());
            }
            out.push(fields);
        }
        out
    }

    /// What a scenario says of one chart, beside the chart's own state.
    struct ChartScript {
        chart: Arc<Chart>,
        live: bool,
        first_entry_s: i64,
        last_entry_s: i64,
        /// The lookup's code, value and null flag.
        lookup: (u16, f64, bool),
        /// A 200's window was absolute: a badge's body may then be cached.
        lookup_absolute: bool,
        /// When not 0: the chart leaves the host's index at that many more looks at the gate.
        free_at_gate: usize,
        /// The chart leaves the host's index when its lookup is asked for.
        free_at_lookup: bool,
    }

    /// The stubbed world of the C generator.
    struct World {
        host: Arc<Host>,
        clock_s: Cell<i64>,
        clock_usec: Cell<u64>,
        gate: Cell<bool>,
        gate_for: Cell<usize>,
        running: Cell<bool>,
        running_for: Cell<usize>,
        /// The next look at the service is not one the generator makes: it counts for nothing.
        free_look: Cell<bool>,
        exiting: Cell<bool>,
        saved: Cell<bool>,
        /// Whether the agent has a database. With a real one the alert log's statements run over its file; else
        /// the table is empty, a save does what `saved` says, and the alarms the table knows are `sql_alarms`.
        database: Cell<bool>,
        real: RefCell<Option<Real>>,
        /// Whether the host has its ACLK sync configuration: an inserted entry then writes its alarm's row of the
        /// unclaimed queue.
        aclk_config: Cell<bool>,
        /// Whether the metadata queue takes a save, and whether the thread at work is HEALTH's.
        queue_accepts: Cell<bool>,
        health_thread: Cell<bool>,
        sql_alarms: RefCell<Vec<KnownAlarm>>,
        /// The saves the metadata queue took, in arrival order, until a scenario's `store`.
        queued: RefCell<Vec<(Arc<HostAlerts>, u32)>>,
        charts: RefCell<Vec<ChartScript>>,
        /// The `call` rows of the step so far. A started command writes its own, on whatever thread waits for it.
        calls: Arc<Mutex<Vec<Fields>>>,
        transition_ids: Cell<u64>,
        /// A scenario's `exec` lines: an alert's name or `*`, a status or `*`, and what such a command does. The
        /// last line that matches decides.
        exec_rules: RefCell<Vec<(String, String, Outcome)>>,
        /// Without a real database, what the table answers about an alarm's last executed event: `None` when the
        /// question fails.
        last_executed: Cell<Option<Option<i32>>>,
        /// The monotonic clock of a notification's wait: it stands still but for the slices of a wait that end
        /// with the command running.
        monotonic_usec: Arc<AtomicU64>,
        /// The commands spawned so far: the next one's pid is 1001 plus this.
        pids: Cell<i32>,
        /// A `pass` ends with the wait for the notifications in flight.
        auto_wait: Cell<bool>,
    }

    /// What a notification's command does once it is spawned.
    #[derive(Clone, Copy)]
    enum Outcome {
        /// It runs for so many slices of the wait, then exits with the code.
        Exit(usize, i32),
        /// The spawn fails.
        Fail,
        /// It runs for so many slices, then the wait itself breaks.
        Error(usize),
        Hang,
    }

    /// A spawned command of the scenario.
    struct Command {
        pid: i32,
        outcome: Outcome,
        calls: Arc<Mutex<Vec<Fields>>>,
        monotonic_usec: Arc<AtomicU64>,
    }

    impl Command {
        fn call(&self, kind: &str, fields: &[String]) {
            let row = [kind.to_owned(), self.pid.to_string()].into_iter().chain(fields.iter().cloned());
            self.calls.lock().expect("the calls").push(row.map(String::into_bytes).collect());
        }
    }

    impl Execution for Command {
        fn pid(&self) -> i32 {
            self.pid
        }

        /// The generator's stub of `spawn_popen_timedwait()`: a slice that ends with the command running moves
        /// the monotonic clock on by the slice and leaves ETIMEDOUT.
        fn timedwait(mut self: Box<Self>, timeout_ms: i32) -> Waiting {
            let slices = match &mut self.outcome {
                Outcome::Hang => Some(None),
                Outcome::Exit(slices, _) | Outcome::Error(slices) if *slices > 0 => Some(Some(slices)),
                _ => None,
            };
            if let Some(slices) = slices {
                if let Some(slices) = slices {
                    *slices -= 1;
                }
                self.monotonic_usec.fetch_add(timeout_ms as u64 * 1000, Ordering::Relaxed);
                self.call("timedwait", &[timeout_ms.to_string(), "running".to_owned()]);
                return Waiting::Running(self, 110);
            }
            match self.outcome {
                Outcome::Error(_) => {
                    self.call("timedwait", &[timeout_ms.to_string(), "error".to_owned()]);
                    Waiting::Error(self)
                }
                Outcome::Exit(_, code) => {
                    self.call("timedwait", &[timeout_ms.to_string(), "exited".to_owned(), code.to_string()]);
                    Waiting::Exited(code)
                }
                Outcome::Fail | Outcome::Hang => unreachable!("a command that was not started, or never ends"),
            }
        }

        fn kill(self: Box<Self>, timeout_ms: i32) -> i32 {
            self.call("kill", &[timeout_ms.to_string()]);
            -1
        }
    }

    /// The n-th argument of a command line as `prepare_command()` writes it (`exec 'a0' 'a1' ...`, a quote inside
    /// an argument as the four bytes `'\''`), as the generator's stub reads the alert's name and the new status.
    fn command_argument(command: &[u8], index: usize) -> Option<Vec<u8>> {
        let mut rest = &command[command.iter().position(|&byte| byte == b' ')?..];
        for i in 0.. {
            rest = rest.strip_prefix(b" '")?;
            let mut argument = Vec::new();
            loop {
                if let Some(after) = rest.strip_prefix(b"'\\''") {
                    argument.push(b'\'');
                    rest = after;
                } else if rest.first() == Some(&b'\'') {
                    break;
                } else {
                    argument.push(*rest.first()?);
                    rest = &rest[1..];
                }
            }
            rest = &rest[1..];
            if i == index {
                return Some(argument);
            }
        }
        None
    }

    impl World {
        fn calls(&self) -> MutexGuard<'_, Vec<Fields>> {
            self.calls.lock().expect("the calls")
        }

        /// Whether the service runs, without the look that `running-for` counts.
        fn peek_running(&self) -> bool {
            !self.exiting.get() && (self.running_for.get() > 0 || self.running.get())
        }

        /// C's `service_running()` as the generator stubs it: stopping once the exit began; else running for the
        /// looks the scenario counts down, then stopping.
        fn is_running(&self) -> bool {
            if self.free_look.replace(false) {
                return true;
            }
            if self.exiting.get() {
                return false;
            }
            if self.running_for.get() > 0 {
                self.running_for.set(self.running_for.get() - 1);
                if self.running_for.get() == 0 {
                    self.running.set(false);
                }
                return true;
            }
            self.running.get()
        }

        /// C's `rrdhost_should_run_health()` as the generator stubs it: a chart whose countdown ends leaves the
        /// host's index; the gate is open for the looks the scenario counts down, then closed.
        fn may_run_health(&self) -> bool {
            for script in self.charts.borrow_mut().iter_mut().filter(|script| script.free_at_gate > 0) {
                script.free_at_gate -= 1;
                if script.free_at_gate == 0 {
                    self.free(&script.chart);
                }
            }
            if self.gate_for.get() > 0 {
                self.gate_for.set(self.gate_for.get() - 1);
                if self.gate_for.get() == 0 {
                    self.gate.set(false);
                }
                return true;
            }
            self.gate.get()
        }

        /// The chart leaves the host's index; health hears of it only with the scenario's `unlink`.
        fn free(&self, chart: &Arc<Chart>) {
            assert!(self.host.charts().free_if(chart, |_| true), "{} was freed already", chart.id());
        }

        fn clock(&self) -> i64 {
            self.clock_s.get()
        }

        fn script<T>(&self, chart: &Chart, read: impl FnOnce(&mut ChartScript) -> T) -> T {
            let mut charts = self.charts.borrow_mut();
            let script = charts.iter_mut().find(|script| std::ptr::eq(Arc::as_ptr(&script.chart), chart));
            read(script.expect("a chart without a script"))
        }
    }

    impl World {
        /// The stub of `rrdset2value_api_v1_with_owa()`, for health's lookup and for a badge's value: it records its
        /// arguments (with the query source and the priority it was asked through, as C's numbers) and answers
        /// what the scenario says, with a window made of the clock and the two ends.
        fn looked_up(&self, chart: &Arc<Chart>, request: &ValueRequest, source: u8, priority: u8) -> ValueResult {
            let (code, value, null) = self.script(chart, |script| script.lookup);
            if self.script(chart, |script| std::mem::take(&mut script.free_at_lookup)) {
                self.free(chart);
            }
            self.calls().push(vec![
                b"lookup".to_vec(),
                chart.id().as_bytes().to_vec(),
                nullable(&request.dimensions),
                text(request.points),
                text(request.after),
                text(request.before),
                request.time_group.name().as_bytes().to_vec(),
                nullable(&request.time_group_options),
                text(request.resampling_time),
                hex8(request.options as u32),
                text(request.timeout_ms),
                text(request.tier),
                text(source),
                text(priority),
                text(code),
            ]);
            let failed = |window| ValueResult { code, value: f64::NAN, window, value_is_null: true, relative: false };
            match code {
                500 => failed(None),
                400 => failed(Some((0, 0))),
                _ => {
                    let window = (self.clock() + request.after + request.before, self.clock() + request.before);
                    let relative = !self.script(chart, |script| script.lookup_absolute);
                    ValueResult { code, value, window: Some(window), value_is_null: null, relative }
                }
            }
        }
    }

    /// What a badge asks of the daemon: the chart's last entry as the scenario has it, and the scripted lookup,
    /// asked as C's handler asks (QUERY_SOURCE_API_BADGE, STORAGE_PRIORITY_SYNCHRONOUS_FIRST).
    impl netdata_agent_health::badge::Source for World {
        fn last_entry_s(&self, chart: &Chart) -> i64 {
            self.script(chart, |script| script.last_entry_s)
        }

        fn value(&self, _: &Arc<Host>, chart: &Arc<Chart>, request: &ValueRequest) -> ValueResult {
            let priority = match request.priority {
                QueryPriority::SynchronousFirst => 4,
                other => panic!("a badge's value at priority {other:?}"),
            };
            self.looked_up(chart, request, 2, priority)
        }
    }

    impl Env for World {
        fn facts(&self, chart: &Chart) -> ChartFacts {
            let collection = chart.collection();
            ChartFacts {
                obsolete: chart.flags() & flags::OBSOLETE != 0,
                last_collected_s: collection.last_collected.0,
                counter_done: collection.counter_done,
                update_every: chart.update_every(),
            }
        }

        fn retention(&self, chart: &Chart) -> (i64, i64) {
            self.script(chart, |script| (script.first_entry_s, script.last_entry_s))
        }

        /// Health's lookup, through the stub of `looked_up`.
        fn lookup(&self, _: &Arc<Host>, chart: &Arc<Chart>, request: &ValueRequest) -> ValueResult {
            // C's numbers of QUERY_SOURCE_HEALTH and of the priority the lookup asks for
            let priority = match request.priority {
                QueryPriority::Synchronous => 7,
                other => panic!("a lookup at priority {other:?}"),
            };
            self.looked_up(chart, request, 4, priority)
        }

        fn now_usec(&self) -> u64 {
            self.clock() as u64 * 1_000_000 + self.clock_usec.get()
        }

        fn transition_id(&self) -> [u8; 16] {
            self.transition_ids.set(self.transition_ids.get() + 1);
            u128::from(self.transition_ids.get()).to_be_bytes()
        }

        fn exiting(&self) -> bool {
            self.exiting.get()
        }

        fn is_health_thread(&self) -> bool {
            self.health_thread.get()
        }

        fn service_running(&self) -> bool {
            self.is_running()
        }

        /// The stub of `sql_health_alarm_log_load()`: it records whether there is a database. With a real one C's
        /// statements run: the REMOVED rows, then the load's query. With a scripted one the table is empty.
        fn load(&self, host: &Host) -> Option<Vec<LoadedRow>> {
            let database = self.database.get();
            self.calls().push(vec![b"load".to_vec(), text(u8::from(database))]);
            let real = self.real.borrow();
            let Some(real) = real.as_ref() else {
                // The generator's scripted load seeds the host's ids and returns. C's own load, over an empty
                // table, looks at the service once (before the step that finds no row) and writes its record: the
                // look is given back here, the record is left out where the records are compared.
                self.free_look.set(database);
                return database.then(Vec::new);
            };
            let (hostname, id, queue) = (host.hostname(), host_id(host), self.aclk_config.get());
            let mut now_usec = || self.now_usec();
            let mut transition_id = || self.transition_id();
            let (running, health) = (|| self.is_running(), self.health_thread.get());
            let meta = &real.meta;
            meta.check_removed_alerts_state(&hostname, &id, &running, queue, health, &mut now_usec, &mut transition_id);
            let mut rows = Vec::new();
            let prepared = real.meta.load_health_log(&id, |row| {
                rows.push(row);
                true
            });
            prepared.then_some(rows)
        }

        /// The stub of `sql_get_alarm_id()`: what the real table has for that chart and name, else the alarm the
        /// scenario put in the scripted table, the last one when several match.
        fn sql_alarm_id(&self, host: &Host, chart: &[u8], name: Option<&[u8]>) -> Option<(u32, u32)> {
            let known = match self.real.borrow().as_ref() {
                Some(real) => real.meta.get_alarm_id(&host_id(host), chart, name),
                None => {
                    let (alarms, name) = (self.sql_alarms.borrow(), name.unwrap_or(b""));
                    let known = alarms.iter().rev().find(|known| known.0 == chart && known.1 == name);
                    known.map(|known| (known.2, known.3))
                }
            };
            let name = name.unwrap_or(b"");
            let (alarm_id, next_event_id) = known.unwrap_or((0, 0));
            let answer = [text(alarm_id), text(next_event_id)];
            let call = [b"sql_get_alarm_id".to_vec(), chart.to_vec(), name.to_vec()].into_iter().chain(answer);
            self.calls().push(call.collect());
            (alarm_id != 0).then_some((alarm_id, next_event_id))
        }

        /// The stub of `metadata_queue_ae_save()`: the queue takes the save, keeping the host's alerts and the
        /// entry's id for the scenario's `store`, or refuses it.
        fn queue_save(&self, alerts: &Arc<HostAlerts>, unique_id: u32) -> bool {
            let accepts = self.queue_accepts.get();
            self.calls().push(vec![b"queue".to_vec(), text(unique_id), text(u8::from(accepts))]);
            if accepts {
                self.queued.borrow_mut().push((Arc::clone(alerts), unique_id));
            }
            accepts
        }

        /// The stub of `sql_health_alarm_log_save()`: it records the entry as it stands; then a real database
        /// gets its row, and a scripted one marks the entry as saved when the scenario says so.
        fn sql_save(&self, host: &Host, entry: &Entry) -> bool {
            // a repeat's entry is in no log: this row is all the trace has of it
            self.calls().push(vec![
                b"save".to_vec(),
                text(entry.unique_id),
                hex8(entry.flags),
                text(entry.alarm_id),
                text(entry.alarm_event_id),
                entry.old_status.name().as_bytes().to_vec(),
                entry.new_status.name().as_bytes().to_vec(),
                text(entry.when),
                text(entry.delay_up_to_timestamp),
                text(entry.duration),
                text(entry.non_clear_duration),
                text(entry.delay),
                text(entry.last_repeat),
                double(entry.old_value),
                double(entry.new_value),
                uuid_rank(&entry.transition_id),
                text(entry.exec_run_timestamp),
                text(entry.exec_code),
                text(entry.updated_by_id),
            ]);
            match self.real.borrow().as_ref() {
                Some(real) => {
                    let (hostname, queue, health) = (host.hostname(), self.aclk_config.get(), self.health_thread.get());
                    sql::save(&real.meta, &hostname, &host_id(host), entry, queue, health)
                }
                None => self.saved.get(),
            }
        }

        fn commit_transitions(&self) {
            self.calls().push(vec![b"commit_alert_transitions".to_vec()]);
        }

        fn process_pending_queue(&self, _: &Host) -> bool {
            self.calls().push(vec![b"process_alert_pending_queue".to_vec()]);
            false
        }

        /// The stub of `sql_health_get_last_executed_event()`: the real table's answer, else the scenario's.
        fn last_executed_event(&self, host: &Host, alarm_id: u32, unique_id: u32) -> Option<i32> {
            let answer = match self.real.borrow().as_ref() {
                Some(real) => {
                    real.meta.get_last_executed_event(&host_id(host), alarm_id, unique_id, self.health_thread.get())
                }
                None => self.last_executed.get(),
            };
            let (ret, status) = match answer {
                None => (-1, b"-".to_vec()),
                Some(None) => (0, b"-".to_vec()),
                Some(Some(status)) => (1, status_name(status).as_bytes().to_vec()),
            };
            self.calls().push(vec![b"last_executed".to_vec(), text(unique_id), text(ret), status]);
            answer.flatten()
        }

        /// The stub of `spawn_popen_run()`: the command line is recorded and nothing starts; what the command then
        /// does is what the scenario's `exec` says of its alert (argument 7) and new status (argument 9).
        fn exec(&self, command: &[u8]) -> Option<Box<dyn Execution>> {
            let argument = |index| command_argument(command, index).unwrap_or_default();
            let (alert, status) = (argument(7), argument(9));
            let matches = |pattern: &str, value: &[u8]| pattern == "*" || pattern.as_bytes() == value;
            let rules = self.exec_rules.borrow();
            let rule = rules.iter().rev().find(|rule| matches(&rule.0, &alert) && matches(&rule.1, &status));
            let outcome = rule.map_or(Outcome::Exit(0, 0), |rule| rule.2);
            if matches!(outcome, Outcome::Fail) {
                self.calls().push(vec![b"spawn".to_vec(), b"0".to_vec(), command.to_vec()]);
                return None;
            }
            let pid = 1001 + self.pids.get();
            self.pids.set(self.pids.get() + 1);
            self.calls().push(vec![b"spawn".to_vec(), text(pid), command.to_vec()]);
            let (calls, monotonic_usec) = (Arc::clone(&self.calls), Arc::clone(&self.monotonic_usec));
            Some(Box::new(Command { pid, outcome, calls, monotonic_usec }))
        }

        fn monotonic_usec(&self) -> u64 {
            self.calls().push(vec![b"monotonic".to_vec()]);
            self.monotonic_usec.load(Ordering::Relaxed)
        }

        /// The generator's user configuration directory and its localhost's registry hostname.
        fn edit_context(&self) -> (Vec<u8>, Vec<u8>) {
            (b"/oracle/etc".to_vec(), b"oracle-registry".to_vec())
        }
    }

    const STATUSES: [Status; 7] = [
        Status::Removed,
        Status::Undefined,
        Status::Uninitialized,
        Status::Clear,
        Status::Raised,
        Status::Warning,
        Status::Critical,
    ];

    /// `rrdcalc_status2string()` of a status as the table holds it.
    fn status_name(status: i32) -> &'static str {
        STATUSES.iter().find(|known| **known as i32 == status).map_or("UNKNOWN", |known| known.name())
    }

    /// The status a scenario names.
    fn status_of(name: &str) -> Status {
        *STATUSES.iter().find(|known| known.name() == name).unwrap_or_else(|| panic!("no status {name}"))
    }

    fn host() -> Arc<Host> {
        let info = HostInfo {
            hostname: "oracle-host".into(),
            registry_hostname: "oracle-registry".into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: DbMode::Ram,
            history_entries: 60,
            health_enabled: true,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        };
        Arc::new(Host::new("11111111-2222-4333-8444-555555555555", true, info))
    }

    /// A second of a scenario: an offset from T0, or 0 for none.
    fn second(text: &str) -> i64 {
        if text == "0" { 0 } else { T0 + text.parse::<i64>().expect("a second") }
    }

    fn number(text: &str) -> f64 {
        if text == "nan" { f64::NAN } else { text.parse().expect("a number") }
    }

    /// The host's two pending flags, left as they are.
    fn host_pending(host: &Host) -> u32 {
        let pending = host.take_health_pending();
        host.raise_pending_flags(pending);
        pending
    }

    // -------------------------------------------------------------------------------------------
    // C's records

    /// A record as the comparison sees it: source, level, the fields by C's key, the message.
    type Record = (String, String, Vec<(String, String)>, Vec<u8>);

    /// The seconds since the epoch of a UTC date as C's logfmt prints one (`2033-05-18T03:33:25Z`).
    fn epoch_of(date: &str) -> i64 {
        let n = |range: std::ops::Range<usize>| date[range].parse::<i64>().expect("a date");
        let (y, m, d) = (n(0..4), n(5..7), n(8..10));
        // days from the civil date (Howard Hinnant's algorithm)
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        days * 86400 + n(11..13) * 3600 + n(14..16) * 60 + n(17..19)
    }

    /// A double as both sides print it, in one form: the log's own (`print_netdata_double()`, at most seven
    /// decimals), which is what C's record holds; a captured record holds the value itself.
    fn canonical_double(text: &str) -> String {
        match text {
            "null" | "NaN" | "nan" => "nan".to_owned(),
            text => netdata_agent_text::print::netdata_double_to_string(text.parse::<f64>().expect("a double")),
        }
    }

    /// One logfmt line of C: `key=value` pairs, a value quoted when it needs to be.
    fn c_record(line: &[u8]) -> Record {
        let (mut source, mut level, mut message, mut fields) = (String::new(), String::new(), Vec::new(), Vec::new());
        let mut i = 0;
        while i < line.len() {
            if line[i] == b' ' {
                i += 1;
                continue;
            }
            let equals = i + line[i..].iter().position(|&c| c == b'=').expect("a key");
            let key = String::from_utf8_lossy(&line[i..equals]).into_owned();
            i = equals + 1;
            let value = if line.get(i) == Some(&b'"') {
                let mut end = i + 1;
                while line[end] != b'"' {
                    end += if line[end] == b'\\' { 2 } else { 1 };
                }
                let value = unescape_logfmt(&line[i + 1..end]);
                i = end + 1;
                value
            } else {
                let end = i + line[i..].iter().position(|&c| c == b' ').unwrap_or(line.len() - i);
                let value = line[i..end].to_vec();
                i = end;
                value
            };
            let as_text = String::from_utf8_lossy(&value).into_owned();
            match key.as_str() {
                "time" | "tid" => {}
                "source" => source = as_text,
                "level" => level = as_text,
                "msg" => message = value,
                "alert_value" => fields.push((key, canonical_double(&as_text))),
                // the key of the old value and of the old status
                "alert_value_old" if as_text == "null" || as_text.parse::<f64>().is_ok() => {
                    fields.push((key, canonical_double(&as_text)));
                }
                "alert_notification_timestamp" => fields.push((key, epoch_of(&as_text).to_string())),
                // the number of `<number>, <its text>`
                "errno" => fields.push((key, as_text.split(',').next().unwrap_or_default().to_owned())),
                _ => fields.push((key, as_text)),
            }
        }
        fields.sort();
        (source, level, fields, message)
    }

    /// A record of the Rust loop, with its fields under C's keys.
    fn rust_record(record: &Captured) -> Record {
        let source = match record.source {
            Source::Health => "health",
            Source::Daemon => "daemon",
            other => panic!("a record of source {other:?}"),
        };
        let level = match record.priority {
            Priority::Emerg => "emergency",
            Priority::Alert => "alert",
            Priority::Crit => "critical",
            Priority::Err => "error",
            Priority::Warning => "warning",
            Priority::Notice => "notice",
            Priority::Info => "info",
            Priority::Debug => "debug",
        };
        let mut fields = Vec::new();
        if record.errno != 0 {
            fields.push(("errno".to_owned(), record.errno.to_string()));
        }
        for (field, value) in &record.fields {
            let key = match field {
                Field::MessageId => "msg_id",
                Field::NidlNode => "node",
                Field::NidlInstance => "instance",
                Field::NidlContext => "context",
                Field::ResponseCode => "code",
                Field::AlertId => "alert_id",
                Field::AlertUniqueId => "alert_unique_id",
                Field::AlertEventId => "alert_event_id",
                Field::AlertConfigHash => "alert_config",
                Field::AlertName => "alert",
                Field::AlertClass => "alert_class",
                Field::AlertComponent => "alert_component",
                Field::AlertType => "alert_type",
                Field::AlertExec => "alert_exec",
                Field::AlertRecipient => "alert_recipient",
                Field::AlertDuration => "alert_duration",
                Field::AlertValue => "alert_value",
                Field::AlertValueOld => "alert_value_old",
                Field::AlertStatus => "alert_status",
                // C's table gives the old status the old value's key
                Field::AlertStatusOld => "alert_value_old",
                Field::AlertUnits => "alert_units",
                Field::AlertSummary => "alert_summary",
                Field::AlertInfo => "alert_info",
                Field::AlertNotificationRealtimeUsec => "alert_notification_timestamp",
                Field::AlertTransitionId => "alert_transition_id",
                // the source has no key in C's table
                Field::AlertSource => continue,
                other => panic!("a record with the field {other:?}"),
            };
            let value = match field {
                Field::AlertValue | Field::AlertValueOld => canonical_double(value),
                Field::AlertNotificationRealtimeUsec => {
                    let usec: u64 = value.parse().expect("a time");
                    // C does not print a time of zero
                    if usec == 0 {
                        continue;
                    }
                    (usec / 1_000_000).to_string()
                }
                _ => value.clone(),
            };
            fields.push((key.to_owned(), value));
        }
        fields.sort();
        (source.to_owned(), level.to_owned(), fields, record.message.clone().unwrap_or_default().into_bytes())
    }

    // -------------------------------------------------------------------------------------------
    // one scenario

    struct Replay {
        name: String,
        health: Option<Arc<Health>>,
        rules: Vec<String>,
        /// The configuration's retention and limit of the alert log, when the scenario sets them.
        retention_s: Option<u32>,
        log_max: Option<u32>,
        /// `timeout`, `use-summary`, `default-exec`: the configuration's notification keys.
        timeout_s: Option<i32>,
        use_summary: Option<bool>,
        default_exec: Option<Vec<u8>>,
        /// What the reader of the rule files recorded, until the first step's rows are compared.
        reading: Vec<Captured>,
        /// The alert log's body an `alarm-log` step made, and the rules' answers a `configs` step made, until the
        /// step's rows are compared.
        body: Option<Vec<u8>>,
        configs: Vec<Fields>,
        /// The rows a `load`, a `manage` or a DynCfg step made (`list`, `reply`, `file`, `answer`, `stored`,
        /// `store`), until the step's rows are compared.
        rows: Vec<(&'static str, Fields)>,
        /// `enabled-alarms`: the configuration's pattern of alert names.
        enabled_alarms: Option<Vec<u8>>,
        /// What the model of the DynCfg core knows of saved files: the jobs with their payloads, and the ids the
        /// user disabled; and whether the Cloud has a rule's hash.
        dyncfg_saved: Vec<(Vec<u8>, Vec<u8>)>,
        dyncfg_user_disabled: Vec<Vec<u8>>,
        cloud_has: bool,
        /// The last `store` row of each name of the rules' store.
        store_rows: HashMap<Vec<u8>, Fields>,
        /// `health-dirs`: the two health.d trees a `reload` reads.
        dirs: netdata_agent_health::ConfigDirs,
        /// The silencers' file is in a directory of the scenario's own; C's rows name it `{file}`.
        silencers_dir: tempfile::TempDir,
        host: Arc<Host>,
        world: World,
        step: usize,
        /// The last row of each entry of the log, by unique id.
        entry_rows: HashMap<u32, Fields>,
        /// C's rows of this scenario: per step, per kind, in order.
        expected: BTreeMap<usize, BTreeMap<String, Vec<Fields>>>,
        failures: Vec<String>,
    }

    impl Replay {
        /// The health plugin, made at the first step: every rule file, the database and the configuration are
        /// known by then. With a real database each rule's `alert_hash` row is stored as the rule is read.
        fn health(&mut self) -> Arc<Health> {
            if self.health.is_none() {
                let mut config = oracle_config();
                config.health_log_retention_s = self.retention_s.unwrap_or(config.health_log_retention_s);
                config.health_log_entries_max = self.log_max.unwrap_or(config.health_log_entries_max);
                config.notification_execution_timeout_s =
                    self.timeout_s.unwrap_or(config.notification_execution_timeout_s);
                config.use_summary_for_notifications = self.use_summary.unwrap_or(config.use_summary_for_notifications);
                if let Some(default_exec) = self.default_exec.take() {
                    config.default_exec = default_exec;
                }
                if let Some(pattern) = self.enabled_alarms.take() {
                    config.enabled_alerts = HealthConfig::enabled_alerts_pattern(&pattern);
                }
                config.silencers_filename = self.silencers_file().into_os_string().into_encoded_bytes();
                let meta = self.world.real.borrow().as_ref().map(|real| Arc::clone(&real.meta));
                let store: StoreSink = match meta {
                    Some(meta) => Box::new(move |rule| {
                        assert!(meta.store_alert_config(&alert_hash_row(rule)), "a rule's row was not stored");
                    }),
                    None => Box::new(|_| {}),
                };
                let health = Health::init(config, store);
                // the generator reads the files as their directives come: what the reader records is in the rows
                // of the scenario's first step
                let ((), reading) = netdata_agent_log::capture(|| {
                    for path in &self.rules {
                        // the generator clears the thread's error number before each directive
                        netdata_agent_log::take_errno();
                        assert!(health_readfile(&health, path.as_bytes(), false), "cannot read {path}");
                    }
                });
                self.reading = reading;
                self.health = Some(health);
            }
            Arc::clone(self.health.as_ref().expect("made above"))
        }

        fn silencers_file(&self) -> std::path::PathBuf {
            self.silencers_dir.path().join("health.silencers.json")
        }

        /// A DynCfg step, off the HEALTH thread as in the daemon: `f` gets health and the scenario's model of the
        /// DynCfg core, whose calls go where the world's go.
        fn with_core<T>(&mut self, f: impl FnOnce(&Health, &Core<'_>) -> T) -> (T, Vec<Captured>) {
            let health = self.health();
            let (world, hosts) = (&self.world, OneHost(Arc::clone(&self.host)));
            let health_thread = world.health_thread.replace(false);
            let core = Core {
                health: &health,
                hosts: &hosts,
                env: world,
                clock: &|| world.clock(),
                saved: &self.dyncfg_saved,
                user_disabled: &self.dyncfg_user_disabled,
                cloud_has: self.cloud_has,
                record: &|call| world.calls().push(call),
            };
            let result = netdata_agent_log::capture(|| f(&health, &core));
            world.health_thread.set(health_thread);
            result
        }

        /// A payload of a scenario: none (`-`), a file's bytes (`@<path>`), what a GET of a node answers (`=<id>`),
        /// or the text itself.
        fn payload(&mut self, text: &str) -> Option<Vec<u8>> {
            if text.is_empty() || text == "-" {
                return None;
            }
            if let Some(path) = text.strip_prefix('@') {
                return Some(std::fs::read(path).unwrap_or_else(|e| panic!("{}: {path}: {e}", self.name)));
            }
            if let Some(id) = text.strip_prefix('=') {
                let ((code, body), _) = self.with_core(|health, core| {
                    let mut reply = Reply::new(ContentType::TextPlain);
                    let code = health.dyncfg_callback(&core.ctx(), &mut reply, id.as_bytes(), Cmds::GET, None, None);
                    (code, reply.body)
                });
                assert_eq!(code, 200, "{}: no GET of {id}", self.name);
                return Some(body);
            }
            Some(text.as_bytes().to_vec())
        }

        /// The rules' store after a DynCfg step: how many names and which, then a row for each name that is new or
        /// changed: whether it is enabled, how many rules its chain has, and the chain as a GET prints it.
        fn store_rows(&mut self) {
            let health = self.health();
            let prototypes = health.prototypes();
            let names: Vec<&[u8]> = prototypes.iter().map(|(name, _)| name).collect();
            let listed = if names.is_empty() { b"-".to_vec() } else { names.join(&b' ') };
            self.rows.push(("stored", vec![text(names.len()), listed]));
            for (name, prototype) in prototypes.iter() {
                let rules: Vec<&Rule> = prototype.rules().iter().collect();
                let fields = vec![
                    name.to_vec(),
                    text(u8::from(prototype.enabled())),
                    text(rules.len()),
                    prototype_to_json(name, &rules, false),
                ];
                if self.store_rows.get(name) != Some(&fields) {
                    self.store_rows.insert(name.to_vec(), fields.clone());
                    self.rows.push(("store", fields));
                }
            }
        }

        /// A chart of the scenario, also one that left the host's index.
        fn chart(&self, id: &str) -> Arc<Chart> {
            let charts = self.world.charts.borrow();
            let script = charts.iter().find(|script| script.chart.id() == id);
            Arc::clone(&script.unwrap_or_else(|| panic!("{}: no chart {id}", self.name)).chart)
        }

        fn directive(&mut self, line: &str) {
            let mut words = line.splitn(2, ' ');
            let (directive, rest) = (words.next().expect("a directive"), words.next().unwrap_or(""));
            let args: Vec<&str> = rest.split(' ').collect();
            let flag = |text: &str| text != "0";
            // the generator reads these at once; here they make the plugin, at the first step
            let early = matches!(
                directive,
                "rules" | "database" | "retention" | "log-max" | "timeout" | "use-summary" | "default-exec" | "enabled-alarms"
            );
            assert!(self.health.is_none() || !early, "{}: {line}", self.name);
            match directive {
                "rules" => self.rules.push(args[0].to_owned()),
                "database" if args[0] == "real" => {
                    let dir = tempfile::tempdir().expect("a directory");
                    let meta = MetaDb::open(dir.path(), &SqliteSettings::default()).expect("the metadata database");
                    let raw = rusqlite::Connection::open(MetaDb::path(dir.path())).expect("a second connection");
                    self.world.database.set(true);
                    *self.world.real.borrow_mut() = Some(Real { meta: Arc::new(meta), raw, _dir: dir });
                }
                "database" => self.world.database.set(flag(args[0])),
                "retention" => self.retention_s = Some(args[0].parse().expect("seconds")),
                "log-max" => self.log_max = Some(args[0].parse().expect("a count")),
                "timeout" => self.timeout_s = Some(args[0].parse().expect("seconds")),
                "use-summary" => self.use_summary = Some(flag(args[0])),
                "default-exec" => {
                    self.default_exec = Some(if rest == "-" { Vec::new() } else { rest.as_bytes().to_vec() });
                }
                "exec" => {
                    let count = |text: &str| text.parse::<usize>().expect("a count of slices");
                    let outcome = match args[2] {
                        "exit" => Outcome::Exit(count(args[3]), args[4].parse().expect("an exit code")),
                        "fail" => Outcome::Fail,
                        "error" => Outcome::Error(count(args[3])),
                        "hang" => Outcome::Hang,
                        other => panic!("{}: exec {other}", self.name),
                    };
                    self.world.exec_rules.borrow_mut().push((args[0].to_owned(), args[1].to_owned(), outcome));
                }
                "last-executed" => self.world.last_executed.set(match args[0] {
                    "fail" => None,
                    "none" => Some(None),
                    name => Some(Some(status_of(name) as i32)),
                }),
                "auto-wait" => self.world.auto_wait.set(flag(args[0])),
                // the wait HEALTH makes after the hosts of an iteration, alone
                "wait" => {
                    let (health, world) = (self.health(), &self.world);
                    let ((), records) = netdata_agent_log::capture(|| health.wait_for_notifications(world));
                    self.dump(line, None, records);
                }
                "aclk-config" => self.world.aclk_config.set(flag(args[0])),
                "sql" => {
                    // on the agent's own connection, as the generator runs it on `db_meta`: a schema change made
                    // on another connection would reach the agent's statements only when they step
                    let real = self.world.real.borrow();
                    let meta = &real.as_ref().unwrap_or_else(|| panic!("{}: no real database", self.name)).meta;
                    meta.lock().execute_batch(rest).unwrap_or_else(|err| panic!("{}: {rest}: {err}", self.name));
                }
                "hostlabel" => {
                    let (name, value) = rest.split_once(' ').expect("a label");
                    // the generator adds the label and raises nothing
                    let pending = self.host.take_health_pending();
                    self.host.update_labels(|set| set.add(name.as_bytes(), value.as_bytes(), SRC_CONFIG));
                    self.host.take_health_pending();
                    self.host.raise_pending_flags(pending);
                }
                "chart" => {
                    let (type_, id) = args[0].split_once('.').expect("type.id");
                    let name = args[1].strip_prefix(&format!("{type_}.")).expect("a name of the chart's type");
                    let dash = |text: &'static str| if text == "-" { "" } else { text };
                    let (family, units): (&str, &str) = (args[3], args[4]);
                    let (family, units) = (dash(leak(family)), dash(leak(units)));
                    let (chart, _) = self.host.charts().create(&ChartSpec {
                        type_,
                        id,
                        name: Some(name),
                        family: Some(family),
                        context: Some(args[2]),
                        title: "title",
                        units,
                        plugin: "loop.plugin",
                        module: Some("loop"),
                        priority: 1000,
                        update_every: args[5].parse().expect("an update every"),
                        chart_type: ChartType::Line,
                        mode: DbMode::Ram,
                        history_entries: 60,
                        page_size: 4096,
                    });
                    // rrdset_update_permanent_labels()
                    chart.update_meta(|meta| {
                        meta.labels.add(b"_collect_plugin", b"loop.plugin", SRC_CONFIG);
                        meta.labels.add(b"_collect_module", b"loop", SRC_CONFIG);
                    });
                    let clock = self.world.clock();
                    chart.update_collection(|collection| {
                        collection.counter_done = 2;
                        collection.last_collected = (clock, 0);
                    });
                    // what a new chart asks of health
                    chart.flags_set_and_clear(flags::PENDING_HEALTH_INITIALIZATION, 0);
                    self.host.raise_pending_flags(pending_flags::HEALTH_INITIALIZATION);
                    self.world.charts.borrow_mut().push(ChartScript {
                        chart,
                        live: true,
                        first_entry_s: T0 - 2 * 86400,
                        last_entry_s: clock,
                        lookup: (200, f64::NAN, true),
                        lookup_absolute: false,
                        free_at_gate: 0,
                        free_at_lookup: false,
                    });
                }
                "label" => {
                    let chart = self.chart(args[0]);
                    let (name, value) = rest.split_once(' ').expect("a chart").1.split_once(' ').expect("a label");
                    let (pending, chart_pending) = (self.host.take_health_pending(), chart.take_health_pending());
                    chart.update_meta(|meta| meta.labels.add(name.as_bytes(), value.as_bytes(), SRC_CONFIG));
                    self.host.take_health_pending();
                    chart.take_health_pending();
                    self.host.raise_pending_flags(pending);
                    chart.flags_set_and_clear(chart_pending, 0);
                }
                "dim" => {
                    let chart = self.chart(args[0]);
                    let (dim, _) = chart.dim_add(args[1], Some(args[2]), 1, 1, Algorithm::Absolute);
                    let value = number(args[3]);
                    dim.update_collection(|collection| collection.last_stored_value = value);
                }
                "var" => match args[0] {
                    "host" => self.host.set_variable(args[1], number(args[2])),
                    chart => self.chart(chart).set_variable(args[1], number(args[2])),
                },
                "live" => self.world.script(&self.chart(args[0]), |script| script.live = flag(args[1])),
                "collected" => {
                    let (counter_done, last) = (args[1].parse().expect("a count"), second(args[2]));
                    self.chart(args[0]).update_collection(|collection| {
                        collection.counter_done = counter_done;
                        collection.last_collected = (last, 0);
                    });
                }
                "entries" => {
                    let (first, last) = (second(args[1]), second(args[2]));
                    self.world.script(&self.chart(args[0]), |script| {
                        script.first_entry_s = first;
                        script.last_entry_s = last;
                    });
                }
                "obsolete" => {
                    let chart = self.chart(args[0]);
                    if flag(args[1]) {
                        chart.update_meta(|meta| meta.flags |= flags::OBSOLETE);
                    } else {
                        chart.update_meta(|meta| meta.flags &= !flags::OBSOLETE);
                    }
                }
                "lookup" => {
                    let lookup = (args[1].parse().expect("a code"), number(args[2]), flag(args[3]));
                    let absolute = match args.get(4) {
                        None => false,
                        Some(&"absolute") => true,
                        Some(other) => panic!("{}: {line}: an unknown word {other}", self.name),
                    };
                    self.world.script(&self.chart(args[0]), |script| {
                        script.lookup = lookup;
                        script.lookup_absolute = absolute;
                    });
                }
                "gate" => self.world.gate.set(flag(args[0])),
                "gate-for" => self.world.gate_for.set(args[0].parse().expect("a count")),
                "free-at-gate" => {
                    let looks = args[1].parse().expect("a count");
                    self.world.script(&self.chart(args[0]), |script| script.free_at_gate = looks);
                }
                "free-at-lookup" => self.world.script(&self.chart(args[0]), |script| script.free_at_lookup = true),
                "running" => self.world.running.set(flag(args[0])),
                "running-for" => self.world.running_for.set(args[0].parse().expect("a count")),
                "saved" => self.world.saved.set(flag(args[0])),
                "queue" => self.world.queue_accepts.set(flag(args[0])),
                "thread" => self.world.health_thread.set(match args[0] {
                    "health" => true,
                    "other" => false,
                    other => panic!("{}: thread {other}", self.name),
                }),
                "sql-alarm" => {
                    let (alarm_id, next_event_id) = (args[2].parse().expect("an id"), args[3].parse().expect("an id"));
                    let known = (args[0].as_bytes().to_vec(), args[1].as_bytes().to_vec(), alarm_id, next_event_id);
                    self.world.sql_alarms.borrow_mut().push(known);
                }
                "exiting" => self.world.exiting.set(true),
                "delay-up-to" => self.host.set_health_delay_up_to(second(args[0])),
                "pending" => match args[0] {
                    "host-init" => self.host.raise_pending_flags(pending_flags::HEALTH_INITIALIZATION),
                    "host-recheck" => self.host.raise_pending_flags(pending_flags::LABEL_RECHECK),
                    "chart-init" => {
                        self.chart(args[1]).flags_set_and_clear(flags::PENDING_HEALTH_INITIALIZATION, 0);
                    }
                    "chart-recheck" => {
                        self.chart(args[1]).flags_set_and_clear(flags::PENDING_LABEL_RECHECK, 0);
                    }
                    other => panic!("{}: pending {other}", self.name),
                },
                "clock" => {
                    self.world.clock_s.set(T0 + args[0].parse::<i64>().expect("a second"));
                    self.world.clock_usec.set(args.get(1).map_or(123_456, |usec| usec.parse().expect("microseconds")));
                }
                "pass" => {
                    let now = T0 + args[0].parse::<i64>().expect("a second");
                    let hibernate = args.get(1) == Some(&"hibernate");
                    let clock = self.world.clock();
                    for script in self.world.charts.borrow_mut().iter_mut().filter(|script| script.live) {
                        script.chart.update_collection(|collection| collection.last_collected = (clock, 0));
                        script.last_entry_s = clock;
                    }
                    let health = self.health();
                    let mut next_run = now + i64::from(health.config().run_at_least_every_s);
                    let world = &self.world;
                    let ((), records) = netdata_agent_log::capture(|| {
                        let pass = Pass {
                            now,
                            apply_hibernation_delay: hibernate,
                            next_run: &mut next_run,
                            gate: &|| world.may_run_health(),
                        };
                        health.host_pass(&self.host, pass, world, &|| world.clock(), &|| world.is_running());
                        // the daemon's loop, after its hosts and unless the service stops
                        if world.auto_wait.get() && world.peek_running() {
                            health.wait_for_notifications(world);
                        }
                    });
                    self.dump(line, Some(next_run), records);
                }
                "unlink" => {
                    let (health, chart, world) = (self.health(), self.chart(args[0]), &self.world);
                    let ((), records) = netdata_agent_log::capture(|| {
                        health.chart_freed(self.host.machine_guid(), &chart, world, &|| world.clock());
                    });
                    self.dump(line, None, records);
                }
                "apply" => {
                    let (health, world) = (self.health(), &self.world);
                    let ((), records) = netdata_agent_log::capture(|| {
                        health.apply_prototypes_to_host(&self.host, world, &|| world.clock(), &|| world.is_running());
                    });
                    self.dump(line, None, records);
                }
                // `store_alert_transitions()` on a worker of the metadata thread: the queued saves in arrival order
                "store" => {
                    let (world, host) = (&self.world, &self.host);
                    let health_thread = world.health_thread.replace(false);
                    let queued = std::mem::take(&mut *world.queued.borrow_mut());
                    let ((), records) = netdata_agent_log::capture(|| {
                        for (alerts, unique_id) in queued {
                            alerts.save_queued(unique_id, &|entry| world.sql_save(host, entry));
                        }
                    });
                    world.health_thread.set(health_thread);
                    self.dump(line, None, records);
                }
                // A new process on the same database: the host's alerts, its log and what the queue held are
                // gone; the rules are those read before, and every chart asks for its alerts again.
                "restart" => {
                    let health = self.health();
                    self.world.queued.borrow_mut().clear();
                    health.host_freed(&self.host);
                    self.world.exiting.set(false);
                    self.host.take_health_pending();
                    self.host.raise_pending_flags(pending_flags::HEALTH_INITIALIZATION);
                    self.host.set_health_delay_up_to(0);
                    for script in self.world.charts.borrow().iter().filter(|script| !script.chart.is_freed()) {
                        script.chart.take_health_pending();
                        script.chart.flags_set_and_clear(flags::PENDING_HEALTH_INITIALIZATION, 0);
                    }
                    self.dump(line, None, Vec::new());
                }
                // the metadata thread's hourly cleanup of the host's alert log, then the memory log's once more
                "cleanup" => {
                    let (health, world) = (self.health(), &self.world);
                    let alerts = health.host(&self.host);
                    let ((), records) = netdata_agent_log::capture(|| {
                        // the generator makes the daemon's one call, C's table cleanup, which ends with the
                        // memory's unless its statement fails to prepare; without a database that statement never
                        // prepares, and the generator calls the memory's cleanup alone
                        if let Some(real) = world.real.borrow().as_ref() {
                            let (host, id) = (&self.host, host_id(&self.host));
                            sql::cleanup(&real.meta, host, &id, alerts.as_deref(), &|| world.clock());
                        } else if let Some(alerts) = &alerts {
                            alerts.log_cleanup(self.host.health_log_retention_s(), &|| world.clock());
                        }
                    });
                    self.dump(line, None, records);
                }
                // `/api/v1/alarm_log`'s body: the entries above an id (read as C's `strtoul(.., 0)` reads it), of a
                // chart when one is named, at most the host's limit
                "alarm-log" => {
                    let (health, world) = (self.health(), &self.world);
                    let after = strtoul0(args[0].as_bytes()).0 as i64;
                    let chart = rest.split_once(' ').map(|(_, chart)| chart.as_bytes());
                    let limit = health.host(&self.host).map_or(0, |alerts| alerts.log_max());
                    let info = self.host.info();
                    let (default_exec, default_recipient) = health.host_defaults(&self.host);
                    let view = sql::LogView {
                        hostname: info.hostname.as_bytes(),
                        utc_offset: info.utc_offset,
                        abbrev_timezone: info.abbrev_timezone.as_bytes(),
                        default_exec,
                        default_recipient,
                        user_config_dir: b"/oracle/etc",
                        registry_hostname: info.registry_hostname.as_bytes(),
                    };
                    let (body, records) = netdata_agent_log::capture(|| {
                        let real = world.real.borrow();
                        let meta = &real.as_ref().unwrap_or_else(|| panic!("{}: no real database", self.name)).meta;
                        sql::alarm_log_json(meta, &host_id(&self.host), &view, after, chart, limit)
                    });
                    self.body = Some(body);
                    self.dump(line, None, records);
                }
                // `/api/v2/alert_config` for every rule the table has, in the table's order, then for a hash no rule
                // has: the hash, how many rules were found, and the body when one was
                "configs" => {
                    let (health, world) = (self.health(), &self.world);
                    let (configs, records) = netdata_agent_log::capture(|| {
                        let real = world.real.borrow();
                        let real = real.as_ref().unwrap_or_else(|| panic!("{}: no real database", self.name));
                        let mut hashes = table_hashes(&real.raw);
                        hashes.push(b"5a1e0000-0000-4000-8000-00000000dead".to_vec());
                        let recipient = health.host_defaults(&self.host).1;
                        let config = |hash: Vec<u8>| match sql::alert_config_json(Some(&real.meta), &hash, recipient) {
                            sql::ConfigAnswer::Found(body) => vec![hash, b"1".to_vec(), body],
                            sql::ConfigAnswer::NotFound => vec![hash, b"0".to_vec(), Vec::new()],
                            sql::ConfigAnswer::Failed => vec![hash, b"-1".to_vec(), Vec::new()],
                        };
                        hashes.into_iter().map(config).collect::<Vec<Fields>>()
                    });
                    self.configs = configs;
                    self.dump(line, None, records);
                }
                // the silencers' file as the scenario leaves it for the next `load`
                "silencers-file" => {
                    let path = self.silencers_file();
                    if rest == "-" {
                        let _ = std::fs::remove_file(path);
                    } else {
                        std::fs::write(path, rest).expect("the silencers' file");
                    }
                }
                // the file's read at health's start
                "load" => {
                    let health = self.health();
                    let ((), records) = netdata_agent_log::capture(|| health.silencers().init());
                    self.rows.push(("list", vec![health.silencers().to_json()]));
                    self.dump(line, None, records);
                }
                // a request of `/api/v1/manage/health`: with the management key, another text, or no token
                "manage" => {
                    let health = self.health();
                    let token = crate::management_token(args[0]);
                    let query = rest.split_once(' ').map_or("", |(_, query)| query);
                    let (reply, records) = netdata_agent_log::capture(|| {
                        health.silencers().request(token, crate::MANAGEMENT_KEY, query.as_bytes())
                    });
                    self.rows.push(("reply", vec![text(reply.code), text(u8::from(reply.json)), reply.body]));
                    let file = std::fs::read(self.silencers_file());
                    self.rows.push(("file", vec![text(u8::from(file.is_ok())), file.unwrap_or_default()]));
                    self.dump(line, None, records);
                }
                // `/api/v1/badge.svg` with the rest of the line as its decoded query
                "badge" => {
                    let (health, world, host) = (self.health(), &self.world, &self.host);
                    let alerts = health.host(host);
                    let (badge, records) = netdata_agent_log::capture(|| {
                        api_v1_badge(host, alerts.as_deref(), rest.as_bytes(), 3, world, &|| world.clock())
                    });
                    let clock = world.clock();
                    let after = |at: i64| if at == 0 { b"-".to_vec() } else { text(at - clock) };
                    self.rows.push((
                        "badge",
                        vec![
                            text(badge.code),
                            if badge.svg { b"svg".to_vec() } else { b"text".to_vec() },
                            if badge.no_cacheable { b"n".to_vec() } else { b"c".to_vec() },
                            after(badge.date),
                            after(badge.expires),
                            badge.headers,
                            badge.body,
                        ],
                    ));
                    self.dump(line, None, records);
                }
                "enabled-alarms" => self.enabled_alarms = Some(rest.as_bytes().to_vec()),
                "dyncfg-saved" => {
                    let (name, payload) = rest.split_once(' ').expect("a name and a payload");
                    let payload = self.payload(payload).expect("a saved job's payload");
                    self.dyncfg_saved.push((name.as_bytes().to_vec(), payload));
                }
                "dyncfg-user-disabled" => self.dyncfg_user_disabled.push(args[0].as_bytes().to_vec()),
                "cloud-has" => self.cloud_has = flag(args[0]),
                "dyncfg-forget" => {
                    let before = self.dyncfg_saved.len() + self.dyncfg_user_disabled.len();
                    self.dyncfg_saved.retain(|(name, _)| name != args[0].as_bytes());
                    self.dyncfg_user_disabled.retain(|id| id != args[0].as_bytes());
                    assert!(self.dyncfg_saved.len() + self.dyncfg_user_disabled.len() < before, "{}: {line}", self.name);
                }
                "health-dirs" => {
                    self.dirs.user = args[0].as_bytes().to_vec();
                    self.dirs.stock = (args[1] != "-").then(|| args[1].as_bytes().to_vec());
                }
                "health-enabled" => self.host.set_health_enabled(flag(args[0])),
                // a child's detach, as the streaming receiver makes it, off the HEALTH thread. The host's health
                // is turned off before health is told, as `Host::clear_receiver_then` does; C tells first and
                // turns it off after, and the vectors, which are C's, show that it makes no difference
                "disconnect" => {
                    let (health, world, host) = (self.health(), &self.world, &self.host);
                    let health_thread = world.health_thread.replace(false);
                    world.gate.set(false);
                    host.set_health_enabled(false);
                    let ((), records) = netdata_agent_log::capture(|| {
                        health.child_disconnected(host, world, &|| world.clock());
                    });
                    world.health_thread.set(health_thread);
                    self.dump(line, None, records);
                }
                // `health_plugin_reload()`, off the HEALTH thread
                "reload" => {
                    let dirs = self.dirs.clone();
                    let ((), records) = self.with_core(|health, core| health.plugin_reload(&dirs, &core.ctx()));
                    self.store_rows();
                    self.dump(line, None, records);
                }
                // a user's request as the DynCfg core hands it to health
                "dyncfg" => {
                    let mut words = rest.splitn(4, ' ');
                    let mut word = || words.next().unwrap_or_else(|| panic!("{}: {line}", self.name));
                    let (id, action, name) = (word(), word(), word());
                    let payload = words.next().unwrap_or("-");
                    let cmd = match action {
                        "none" => Cmds::NONE,
                        action => Cmds::parse(action.as_bytes()),
                    };
                    assert!(cmd != Cmds::NONE || action == "none", "{}: {line}", self.name);
                    let name = (name != "-").then_some(name.as_bytes());
                    let payload = self.payload(payload);
                    let now = self.world.clock();
                    let ((code, reply), records) = self.with_core(|health, core| {
                        let mut reply = Reply::new(ContentType::TextPlain);
                        let code =
                            health.dyncfg_callback(&core.ctx(), &mut reply, id.as_bytes(), cmd, name, payload.as_deref());
                        (code, reply)
                    });
                    self.rows.push(("answer", answer_fields(code, &reply, now)));
                    self.store_rows();
                    self.dump(line, None, records);
                }
                // the registration of a start or a reload
                "register" => {
                    let ((), records) = self.with_core(|health, core| health.dyncfg_register_all(&core.ctx()));
                    self.store_rows();
                    self.dump(line, None, records);
                }
                "unregister" => {
                    let ((), records) = self.with_core(|health, core| health.dyncfg_unregister_all(&core.ctx()));
                    self.dump(line, None, records);
                }
                other => panic!("{}: directive {other}", self.name),
            }
        }

        /// The rows of a step as the generator writes them, compared with C's kind by kind.
        fn dump(&mut self, directive: &str, next_run: Option<i64>, records: Vec<Captured>) {
            let mut actual: BTreeMap<String, Vec<Fields>> = BTreeMap::new();
            let mut put = |kind: &str, fields: Fields| actual.entry(kind.to_owned()).or_default().push(fields);

            put("do", vec![directive.as_bytes().to_vec()]);
            if let Some(next_run) = next_run {
                put("next_run", vec![text(next_run)]);
            }
            for call in self.world.calls().drain(..) {
                put("call", call);
            }

            let health = self.health();
            let alerts = health.host(&self.host);
            let pending = host_pending(&self.host);
            let mut pending_text = Vec::new();
            if pending & pending_flags::HEALTH_INITIALIZATION != 0 {
                pending_text.push(b'i');
            }
            if pending & pending_flags::LABEL_RECHECK != 0 {
                pending_text.push(b'r');
            }
            let (next_log_id, next_alarm_id, last_processed_id) =
                alerts.as_ref().map_or((0, 0, 0), |alerts| alerts.log_counters());
            let counts = alerts.as_ref().and_then(|alerts| alerts.pass_counts());
            let published = counts.unwrap_or_default();
            let chart_flags: Vec<String> = self
                .world
                .charts
                .borrow()
                .iter()
                .filter_map(|script| {
                    let flags = script.chart.flags();
                    let init = if flags & flags::PENDING_HEALTH_INITIALIZATION != 0 { "i" } else { "" };
                    let recheck = if flags & flags::PENDING_LABEL_RECHECK != 0 { "r" } else { "" };
                    (!init.is_empty() || !recheck.is_empty()).then(|| format!("{}={init}{recheck}", script.chart.id()))
                })
                .collect();
            put(
                "host",
                vec![
                    pending_text,
                    text(alerts.as_ref().map_or(0, |alerts| alerts.transitions())),
                    text(last_processed_id),
                    text(next_log_id),
                    text(next_alarm_id),
                    text(alerts.as_ref().map_or(0, |alerts| alerts.pending_transitions())),
                    text(self.host.health_delay_up_to()),
                    text(u8::from(counts.is_some())),
                    text(published.clear),
                    text(published.warning),
                    text(published.critical),
                    text(published.undefined),
                    text(published.uninitialized),
                    text(alerts.as_ref().map_or(0, |alerts| alerts.pass_counts_generation())),
                    if chart_flags.is_empty() { b"-".to_vec() } else { chart_flags.join(" ").into_bytes() },
                ],
            );

            for alert in alerts.as_ref().map(|alerts| alerts.alerts()).unwrap_or_default() {
                let (run, snapshot) = (alert.run(), alert.snapshot());
                put(
                    "alert",
                    vec![
                        alert.key.clone(),
                        alert.chart.id().as_bytes().to_vec(),
                        text(alert.id),
                        text(run.next_event_id),
                        text(run.status.name()),
                        text(run.old_status.name()),
                        double(run.value),
                        double(run.old_value),
                        double(run.last_status_change_value),
                        hex8(run.run_flags),
                        text(run.last_updated),
                        text(run.next_update),
                        text(run.last_status_change),
                        text(run.db_after),
                        text(run.db_before),
                        text(run.delay_up_to_timestamp),
                        text(run.delay_last),
                        text(run.delay_up_current),
                        text(run.delay_down_current),
                        text(run.last_repeat),
                        text(run.times_repeat),
                        nullable(&snapshot.summary),
                        nullable(&snapshot.info),
                    ],
                );
                put(
                    "published",
                    vec![
                        alert.key.clone(),
                        text(snapshot.status.name()),
                        double(snapshot.value),
                        hex8(snapshot.run_flags),
                        text(snapshot.last_updated),
                        text(snapshot.next_update),
                        text(snapshot.last_status_change),
                        double(snapshot.last_status_change_value),
                        text(snapshot.global_id),
                        text(snapshot.db_after),
                        text(snapshot.db_before),
                        text(snapshot.delay_up_to_timestamp),
                        text(snapshot.delay_last),
                        text(snapshot.last_repeat),
                        text(snapshot.times_repeat),
                        uuid_rank(&snapshot.last_transition_id),
                    ],
                );
            }

            let entries = alerts.as_ref().map(|alerts| alerts.log_entries()).unwrap_or_default();
            let ids: Vec<String> = entries.iter().map(|entry| entry.unique_id.to_string()).collect();
            put("log", vec![if ids.is_empty() { b"-".to_vec() } else { ids.join(" ").into_bytes() }]);
            for entry in &entries {
                let row = entry_row(entry);
                if self.entry_rows.get(&entry.unique_id) != Some(&row) {
                    self.entry_rows.insert(entry.unique_id, row.clone());
                    put("entry", row);
                }
            }

            if let Some(real) = self.world.real.borrow().as_ref() {
                for table in ["health_log", "health_log_detail", "alert_queue", "aclk_queue"] {
                    for row in table_rows(&real.raw, table) {
                        put("sql", row);
                    }
                }
            }

            if let Some(body) = self.body.take() {
                put("body", vec![body]);
            }
            for config in self.configs.drain(..) {
                put("config", config);
            }
            for (kind, fields) in self.rows.drain(..) {
                put(kind, fields);
            }

            let mut expected = self.expected.remove(&self.step).unwrap_or_default();
            if let Some(calls) = expected.get_mut("call") {
                calls.retain(|call| match &call[0][..] {
                    b"queue" | b"save" | b"lookup" | b"sql_get_alarm_id" | b"load" => true,
                    b"commit_alert_transitions" | b"process_alert_pending_queue" => true,
                    b"last_executed" | b"spawn" | b"monotonic" | b"timedwait" | b"kill" => true,
                    b"dyncfg_add" | b"dyncfg_del" | b"dyncfg_status" | b"echo" | b"echoed" => true,
                    b"alert_hash_has_transitioned" | b"aclk_send_alert_configuration" => true,
                    // an entry freed with a save still queued is kept aside inside the host's alerts: it shows in
                    // the store job's saves
                    b"queue_deletion" => false,
                    other => panic!("{}: a call row of kind {}", self.name, String::from_utf8_lossy(other)),
                });
                if calls.is_empty() {
                    expected.remove("call");
                }
            }
            let expected_records: Vec<Record> =
                expected.remove("record").unwrap_or_default().iter().map(|row| c_record(&row[0])).collect();
            // the generator's scripted load writes no record (see `World::load`)
            let scripted = self.world.real.borrow().is_none();
            let empty_load = b"Table health_log, loaded 0 alarm entries, errors in 0 entries.";
            let load_record = |record: &Record| record.3.ends_with(empty_load);
            let records: Vec<Captured> = self.reading.drain(..).chain(records).collect();
            let mut actual_records: Vec<Record> = records.iter().map(rust_record).collect();
            let file = self.silencers_file();
            for record in &mut actual_records {
                record.3 = crate::name_file(&record.3, file.as_os_str().as_encoded_bytes());
            }
            actual_records.retain(|record| !(scripted && load_record(record)));
            if expected_records != actual_records {
                let first = expected_records.iter().zip(&actual_records).position(|(e, a)| e != a);
                let first = first.unwrap_or(expected_records.len().min(actual_records.len()));
                self.failures.push(format!(
                    "{} step {} ({directive}): records: {} of C, {} of Rust; first difference at {first}:\n  \
                     C    {}\n  Rust {}",
                    self.name,
                    self.step,
                    expected_records.len(),
                    actual_records.len(),
                    show_record(expected_records.get(first)),
                    show_record(actual_records.get(first)),
                ));
            }
            let kinds: std::collections::BTreeSet<&String> = expected.keys().chain(actual.keys()).collect();
            for kind in kinds {
                let (none, e, a) = (Vec::new(), expected.get(kind), actual.get(kind));
                let (e, a) = (e.unwrap_or(&none), a.unwrap_or(&none));
                if e == a {
                    continue;
                }
                let first = e.iter().zip(a).position(|(e, a)| e != a).unwrap_or(e.len().min(a.len()));
                self.failures.push(format!(
                    "{} step {} ({directive}): {kind} rows: {} of C, {} of Rust; first difference at {first}:\n  \
                     C    {}\n  Rust {}",
                    self.name,
                    self.step,
                    e.len(),
                    a.len(),
                    show_row(e.get(first)),
                    show_row(a.get(first)),
                ));
            }
            self.step += 1;
        }
    }

    /// A text that lives as long as the test: a chart's spec borrows its texts.
    fn leak(text: &str) -> &'static str {
        Box::leak(text.to_owned().into_boxed_str())
    }

    fn entry_row(entry: &Entry) -> Fields {
        let config_hash: String = entry.config_hash_id.iter().map(|byte| format!("{byte:02x}")).collect();
        vec![
            text(entry.unique_id),
            text(entry.alarm_id),
            text(entry.alarm_event_id),
            text(entry.old_status.name()),
            text(entry.new_status.name()),
            text(entry.when),
            text(entry.duration),
            text(entry.non_clear_duration),
            text(entry.delay),
            text(entry.delay_up_to_timestamp),
            hex8(entry.flags),
            text(entry.updated_by_id),
            text(entry.updates_id),
            double(entry.old_value),
            double(entry.new_value),
            entry.old_value_string.clone(),
            entry.new_value_string.clone(),
            text(entry.global_id),
            text(entry.exec_run_timestamp),
            text(entry.exec_code),
            text(entry.last_repeat),
            nullable(&entry.name),
            entry.chart.clone(),
            entry.chart_context.clone(),
            entry.chart_name.clone(),
            nullable(&entry.units),
            nullable(&entry.summary),
            nullable(&entry.info),
            nullable(&entry.classification),
            nullable(&entry.component),
            nullable(&entry.r#type),
            nullable(&entry.exec),
            nullable(&entry.recipient),
            nullable(&entry.source),
            config_hash.into_bytes(),
            uuid_rank(&entry.transition_id),
            text(entry.pending_save_count),
        ]
    }

    fn show_row(row: Option<&Fields>) -> String {
        match row {
            Some(fields) => {
                let fields: Vec<_> = fields.iter().map(|field| String::from_utf8_lossy(field).into_owned()).collect();
                fields.join(" | ")
            }
            None => "(none)".to_owned(),
        }
    }

    fn show_record(record: Option<&Record>) -> String {
        match record {
            Some((source, level, fields, message)) => {
                format!("{source}/{level} {fields:?} {:?}", String::from_utf8_lossy(message))
            }
            None => "(none)".to_owned(),
        }
    }

    /// Every scenario of a family (`tests/corpus/<family>/`, C's rows in `<family>.tsv`), replayed; returns the
    /// steps compared and the differences found.
    pub fn run(family: &str) -> (usize, Vec<String>) {
        // C's rows by scenario
        let vectors = format!("{family}.tsv");
        let mut expected: BTreeMap<String, BTreeMap<usize, BTreeMap<String, Vec<Fields>>>> = BTreeMap::new();
        for row in rows(&vectors) {
            let (scenario, step, kind) = (row.str(0).to_owned(), row.num::<usize>(1), row.str(2).to_owned());
            let steps = expected.entry(scenario).or_default();
            steps.entry(step).or_default().entry(kind).or_default().push(row.fields[3..].to_vec());
        }

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus").join(family);
        let entries = std::fs::read_dir(&dir).expect("the scenarios");
        let mut scenarios: Vec<_> = entries.map(|entry| entry.expect("an entry").path()).collect();
        scenarios.retain(|path| path.extension().is_some_and(|extension| extension == "scn"));
        scenarios.sort();

        let (mut steps, mut failures) = (0, Vec::new());
        for path in scenarios {
            let name = path.file_stem().expect("a name").to_string_lossy().into_owned();
            let script = std::fs::read_to_string(&path).expect("a scenario");
            let host = host();
            let world = World {
                host: Arc::clone(&host),
                clock_s: Cell::new(T0),
                clock_usec: Cell::new(123_456),
                gate: Cell::new(true),
                gate_for: Cell::new(0),
                running: Cell::new(true),
                running_for: Cell::new(0),
                free_look: Cell::new(false),
                exiting: Cell::new(false),
                saved: Cell::new(false),
                database: Cell::new(true),
                real: RefCell::new(None),
                aclk_config: Cell::new(false),
                queue_accepts: Cell::new(false),
                health_thread: Cell::new(true),
                sql_alarms: RefCell::new(Vec::new()),
                queued: RefCell::new(Vec::new()),
                charts: RefCell::new(Vec::new()),
                calls: Arc::default(),
                exec_rules: RefCell::new(Vec::new()),
                last_executed: Cell::new(Some(None)),
                monotonic_usec: Arc::default(),
                pids: Cell::new(0),
                auto_wait: Cell::new(true),
                transition_ids: Cell::new(0),
            };
            let mut replay = Replay {
                expected: expected.remove(&name).unwrap_or_else(|| panic!("{vectors} has no scenario {name}")),
                name,
                health: None,
                rules: Vec::new(),
                retention_s: None,
                log_max: None,
                timeout_s: None,
                use_summary: None,
                default_exec: None,
                reading: Vec::new(),
                body: None,
                configs: Vec::new(),
                rows: Vec::new(),
                enabled_alarms: None,
                dyncfg_saved: Vec::new(),
                dyncfg_user_disabled: Vec::new(),
                cloud_has: true,
                store_rows: HashMap::new(),
                dirs: netdata_agent_health::ConfigDirs {
                    user: b"/oracle/etc/health.d".to_vec(),
                    stock: Some(b"/oracle/lib/health.d".to_vec()),
                },
                silencers_dir: tempfile::tempdir().expect("a directory"),
                host,
                world,
                step: 0,
                entry_rows: HashMap::new(),
                failures: Vec::new(),
            };
            for line in script.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
                replay.directive(line);
            }
            assert!(replay.expected.is_empty(), "{}: C has steps the scenario does not", replay.name);
            steps += replay.step;
            failures.append(&mut replay.failures);
        }
        assert!(expected.is_empty(), "{vectors} has scenarios without a file: {:?}", expected.keys());
        (steps, failures)
    }
}

/// Every scenario of `tests/corpus/loop/` through the Rust loop, against C's own pass: after each step the host's
/// counters, every alert with its published snapshot, the log's entries, the calls into what C stubs, and the
/// records.
#[test]
fn loop_matches_c() {
    assert_eq!(replayed("loop"), 229);
}

/// Every scenario of `tests/corpus/queue/` against C's pass with its save queue in play: the metadata queue takes
/// the saves of the entries that links and unlinks log, or refuses them; the store job saves them later, off the
/// HEALTH thread; the table knows some alarms' ids.
#[test]
fn queue_matches_c() {
    assert_eq!(replayed("queue"), 39);
}

/// Every scenario of `tests/corpus/sql/` against C's pass with C's own alert log SQL over a real database file:
/// here the Rust statements run over a new metadata database, and after each step the rows of the alert log's four
/// tables are compared too. A scenario's `restart` is a new process on the same file: the load at the host's first
/// pass, with the REMOVED rows it injects, rows it refuses, and a service that stops while it loads. In `fail` a
/// trigger refuses each of the statements in turn: C's two records per failed step, and what is left behind.
#[test]
fn sql_matches_c() {
    assert_eq!(replayed("sql"), 133);
}

/// Every scenario of `tests/corpus/notify/` against C's own `health_send_notification()` and its waits, over a
/// scripted spawn: which entries are notified, the command line of each byte for byte, the marks and times on the
/// entry at each save, the slices of each wait, the kill at a deadline, a stop and a broken wait, the exit code in
/// memory and, later, in the row.
#[test]
fn notify_matches_c() {
    assert_eq!(replayed("notify"), 124);
}

/// Every scenario of `tests/corpus/silencers/` against C's pass with C's own `health_silencers.c`: requests through
/// C's handler, the file's read, and what the silencers then do to each alert's flags, entries and notifications:
/// SILENCE ALL and DISABLE ALL, selectors by alarm, chart, context and host, a selector without a command, a
/// disabled alert that repeats, is obsolete, or lost its chart, and an entry made before or during a silence.
#[test]
fn silencers_match_c() {
    assert_eq!(replayed("silencers"), 99);
}

/// Health's DynCfg nodes over hosts that run: the registration of a start with saved jobs and nodes the user
/// disabled, a user's add, update, disable, enable and remove with what each does to the alerts and their log, the
/// Cloud's copy of a rule, the queue of saves off the HEALTH thread, a host whose health did not run, a name the
/// configuration excludes. The DynCfg core is the generator's model of it (`tests/common`).
#[test]
fn dyncfg_matches_c() {
    assert_eq!(replayed("dyncfg"), 137);
}

/// A streaming child's detach and return: its alerts go with their entries and are linked again with their ids,
/// before and after the metadata thread stored the entries, with the gate closed, at the agent's exit, postponed,
/// with an entry that waits, on a host that never ran or whose health is off.
#[test]
fn child_matches_c() {
    assert_eq!(replayed("child"), 65);
}

/// `/api/v1/badge.svg` through C's own `api_v1_badge()`: a chart value with every parameter, the stale rule, the
/// refresh headers over a relative and an absolute window, alerts in each status, an alert found through its
/// chart's name, an unknown chart or alarm, numbers at their edges: the code, the type, the cache word, date and
/// expiry, the header line the handler adds and the body.
#[test]
fn badge_matches_c() {
    // 98 requests and the three alert scenarios' two passes each
    assert_eq!(replayed("badge"), 104);
}

/// The reload of health's configuration: the nodes unregistered and registered with the model core's echoes, every
/// alert deleted and linked again, over the same files, changed files, a missing directory, a stock tree, saved
/// DynCfg jobs and a job the user disabled, before a host's first pass, with health off, under the silencers, and
/// while the service stops (the walk is not HEALTH's, so it goes on to its end).
#[test]
fn reload_matches_c() {
    assert_eq!(replayed("reload"), 96);
}

/// The management key of the generator's world (`api_secret` of its stubs).
const MANAGEMENT_KEY: &[u8] = b"oracle-key";

/// A request's token as the vectors name it: the key, another text, none.
fn management_token(name: &str) -> Option<&'static [u8]> {
    match name {
        "ok" => Some(MANAGEMENT_KEY),
        "bad" => Some(b"another-key"),
        "-" => None,
        other => panic!("a token named {other}"),
    }
}

/// `text` with every occurrence of the silencers' file's path written `{file}`, as C's rows have it.
fn name_file(text: &[u8], path: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.windows(path.len()).position(|window| window == path) {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(b"{file}");
        rest = &rest[at + path.len()..];
    }
    out.extend_from_slice(rest);
    out
}

/// The messages of what `f` records, joined as the generator joins them (`-` for none), the file named `{file}`.
fn messages_of<T>(path: &std::path::Path, f: impl FnOnce() -> T) -> (T, Vec<u8>) {
    let (result, records) = netdata_agent_log::capture(f);
    let messages: Vec<String> = records.into_iter().filter_map(|record| record.message).collect();
    let joined = if messages.is_empty() { "-".to_owned() } else { messages.join(" | ") };
    (result, name_file(joined.as_bytes(), path.as_os_str().as_encoded_bytes()))
}

/// C's `health_silencers_init()` over 98 file texts: the state it leaves, as the list prints it, and its records.
/// Five texts kill C (an element of the list that is no object): Rust skips such an element (D210 F4). Seven more
/// are read differently by decision: what json-c takes and `serde_json` refuses is refused (D46.1), and a byte that
/// is not UTF-8 is read as U+FFFD (D89).
#[test]
fn the_silencers_file_is_read_as_c() {
    use netdata_agent_health::silencers::Silencers;
    const NONE: &[u8] = b"{\n\t\"all\": false,\n\t\"type\": \"None\",\n\t\"silencers\": []\n}\n";
    const ONE: &[u8] = b"{\n\t\"all\": false,\n\t\"type\": \"None\",\n\t\"silencers\": [\
        \n\t\t{\n\t\t\t\"alarm\": \"a\"\n\t\t}\n\t]\n}\n";
    const PARSED: &[u8] = b"Parsed health silencers file {file}";
    const REFUSED: &[u8] = b"JSON: Invalid json string. | Parsed health silencers file {file}";
    // the texts Rust reads otherwise than C, with what it makes of each
    let decided: [(&[u8], &[u8], &[u8]); 12] = [
        (b"{\"silencers\":[\"x\"]}", NONE, PARSED),
        (b"{\"silencers\":[5]}", NONE, PARSED),
        (b"{\"silencers\":[true]}", NONE, PARSED),
        (b"{\"silencers\":[[{\"alarm\":\"a\"}]]}", NONE, PARSED),
        (b"{\"silencers\":[{\"alarm\":\"a\"},\"x\"]}", ONE, PARSED),
        (b"{\"all\":true,}", NONE, REFUSED),
        (b"{'all':true}", NONE, REFUSED),
        (b"/* a comment */ {\"all\":true}", NONE, REFUSED),
        (b"{\"all\":TRUE}", NONE, REFUSED),
        (b"{\"silencers\":[{\"alarm\\u0000x\":\"a\"}]}", NONE, REFUSED),
        (b"{\"silencers\":[{\"alarm\":\"a\\ud800b\"}]}", NONE, REFUSED),
        (
            b"{\"silencers\":[{\"alarm\":\"\xff\"}]}",
            b"{\n\t\"all\": false,\n\t\"type\": \"None\",\n\t\"silencers\": [\
              \n\t\t{\n\t\t\t\"alarm\": \"\xef\xbf\xbd\"\n\t\t}\n\t]\n}\n",
            PARSED,
        ),
    ];
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join("health.silencers.json");
    let (mut checked, mut crashes, mut differing, mut failures) = (0, 0, 0, Vec::new());
    for row in rows("silencers-file.tsv") {
        let text = row.bytes(0);
        std::fs::write(&path, text).expect("the file");
        let silencers = Silencers::new(path.clone().into_os_string().into_encoded_bytes());
        let ((), messages) = messages_of(&path, || silencers.init());
        let list = silencers.to_json();
        let c_died = row.fields.len() == 2 && row.str(1).starts_with("signal ");
        crashes += usize::from(c_died);
        let (want_list, want_messages) = match decided.iter().find(|(decided, ..)| *decided == text) {
            Some((_, list, messages)) => {
                // a decided difference is one: C's answer is another, or C died
                if !c_died && (row.bytes(1), row.bytes(2)) == (*list, *messages) {
                    failures.push(format!("silencers-file.tsv:{}: C answers as Rust does: not a difference", row.line));
                }
                differing += 1;
                (*list, *messages)
            }
            None => {
                assert!(!c_died, "silencers-file.tsv:{}: C died and nothing is decided for the text", row.line);
                (row.bytes(1), row.bytes(2))
            }
        };
        if list != want_list || messages != want_messages {
            failures.push(format!(
                "silencers-file.tsv:{}: {:?}\n  want {:?} | {:?}\n  Rust {:?} | {:?}",
                row.line,
                String::from_utf8_lossy(&text[..text.len().min(120)]),
                String::from_utf8_lossy(want_list),
                String::from_utf8_lossy(want_messages),
                String::from_utf8_lossy(&list),
                String::from_utf8_lossy(&messages),
            ));
        }
        checked += 1;
    }
    let shown = failures[..failures.len().min(12)].join("\n");
    assert!(failures.is_empty(), "{} of {checked} differ:\n{shown}", failures.len());
    assert_eq!((checked, crashes, differing), (98, 5, decided.len()));
}

/// C's request handler over 33 sequences of requests (C's own test script's among them): each reply's code, content
/// type and body, the file the request wrote, if any, and its records. The file is removed before each request; in
/// the sequence `unwritable` its path leads through a regular file.
#[test]
fn management_requests_answer_as_c() {
    use netdata_agent_health::silencers::Silencers;
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join("health.silencers.json");
    let (mut checked, mut failures) = (0, Vec::new());
    let mut sequence: Option<(String, Silencers)> = None;
    for row in rows("manage.tsv") {
        let name = row.str(0);
        let unwritable = name == "unwritable";
        if sequence.as_ref().is_none_or(|(known, _)| known != name) {
            assert_eq!(row.str(1), "0", "manage.tsv:{}: a sequence starts at its first request", row.line);
            let mut filename = path.clone().into_os_string().into_encoded_bytes();
            if unwritable {
                std::fs::write(&path, b"").expect("the file");
                filename.extend_from_slice(b"/x");
            }
            sequence = Some((name.to_owned(), Silencers::new(filename)));
        }
        let silencers = &sequence.as_ref().expect("made above").1;
        if !unwritable {
            let _ = std::fs::remove_file(&path);
        }
        let (reply, messages) =
            messages_of(&path, || silencers.request(management_token(row.str(2)), MANAGEMENT_KEY, row.bytes(3)));
        let file = std::fs::read(&path);
        let actual = [
            reply.code.to_string().into_bytes(),
            u8::from(reply.json).to_string().into_bytes(),
            reply.body,
            u8::from(file.is_ok()).to_string().into_bytes(),
            file.unwrap_or_default(),
            messages,
        ];
        if actual[..] != row.fields[4..] {
            let shown = |fields: &[Vec<u8>]| -> Vec<String> {
                fields.iter().map(|field| String::from_utf8_lossy(field).into_owned()).collect()
            };
            failures.push(format!(
                "manage.tsv:{}: {name} request {} {:?}\n  C    {:?}\n  Rust {:?}",
                row.line,
                row.str(1),
                String::from_utf8_lossy(row.bytes(3)),
                shown(&row.fields[4..]),
                shown(&actual),
            ));
        }
        checked += 1;
    }
    let shown = failures[..failures.len().min(12)].join("\n");
    assert!(failures.is_empty(), "{} of {checked} differ:\n{shown}", failures.len());
    assert_eq!(checked, 155);
}

/// C's match over 36 states, three alerts and four sets of run flags: the type the selectors answer for the alert,
/// and what the update makes of its flags, with its record.
#[test]
fn selectors_match_as_c() {
    use netdata_agent_health::alert::run_flags;
    use netdata_agent_health::silencers::{Silencers, Subject, changed_record};
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join("health.silencers.json");
    let (mut checked, mut failures) = (0, Vec::new());
    let mut state: Option<(Vec<u8>, Silencers)> = None;
    for row in rows("silencers-match.tsv") {
        if state.as_ref().is_none_or(|(known, _)| known != row.bytes(0)) {
            let silencers = Silencers::new(path.clone().into_os_string().into_encoded_bytes());
            for query in row.str(0).split(" ; ").filter(|query| !query.is_empty()) {
                assert_eq!(silencers.request(Some(MANAGEMENT_KEY), MANAGEMENT_KEY, query.as_bytes()).code, 200);
            }
            state = Some((row.bytes(0).to_vec(), silencers));
        }
        let silencers = &state.as_ref().expect("made above").1;
        // an alert without a chart has no context: a selector that tests one does not take it
        let context = (row.bytes(3) != [0]).then(|| row.bytes(3));
        let subject = Subject {
            name: row.bytes(1),
            chart: row.bytes(2),
            context: &|pattern| context.is_some_and(|context| pattern.matches(context)),
            hostname: row.bytes(4),
        };
        let before = u32::from_str_radix(row.str(5), 16).expect("the flags");
        let stype = silencers.check(&subject);
        let after = silencers.update(&subject, before);
        let ((), messages) = messages_of(&path, || {
            if after != before {
                changed_record(row.bytes(4), row.bytes(1), before, after);
            }
        });
        let actual = [
            stype.name().as_bytes().to_vec(),
            u8::from(after & run_flags::DISABLED != 0).to_string().into_bytes(),
            format!("{after:08x}").into_bytes(),
            messages,
        ];
        if actual[..] != row.fields[6..] {
            let shown = |fields: &[Vec<u8>]| -> Vec<String> {
                fields.iter().map(|field| String::from_utf8_lossy(field).into_owned()).collect()
            };
            failures.push(format!(
                "silencers-match.tsv:{}: {:?} alert {} flags {}\n  C    {:?}\n  Rust {:?}",
                row.line,
                row.str(0),
                row.str(1),
                row.str(5),
                shown(&row.fields[6..]),
                shown(&actual),
            ));
        }
        checked += 1;
    }
    let shown = failures[..failures.len().min(12)].join("\n");
    assert!(failures.is_empty(), "{} of {checked} differ:\n{shown}", failures.len());
    assert_eq!(checked, 432);
}

/// The steps a family's replay compared; any difference from C's rows fails.
fn replayed(family: &str) -> usize {
    let (steps, failures) = replay::run(family);
    let shown = failures[..failures.len().min(12)].join("\n");
    assert!(failures.is_empty(), "{family}: {} differences over {steps} steps:\n{shown}", failures.len());
    steps
}
