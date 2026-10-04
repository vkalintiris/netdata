//! The evaluation loop and the alert log against C (`tests/oracle/gen-loop-vectors.c`: the tables `units.tsv`,
//! `delay.tsv` and `edit.tsv`; C's own pass over the scenarios of `loop.tsv`, `queue.tsv` and `sql.tsv`).

mod common;

use common::rows;
use netdata_agent_health::sql::edit_command_from_source;
use netdata_agent_text::units::format_value_and_unit;

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
    use std::sync::Arc;

    use netdata_agent_health::alerts::HostAlerts;
    use netdata_agent_health::entry::Entry;
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

    use crate::common::{oracle_config, rows, unescape_logfmt};

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

    /// The metadata database of a scenario that says `database real`: the agent's handle on a new file, and a
    /// second connection for the scenario's own statements and for reading the tables.
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
        /// The `call` rows of the step so far.
        calls: RefCell<Vec<Fields>>,
        transition_ids: Cell<u64>,
    }

    impl World {
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

        /// The stub of `rrdset2value_api_v1_with_owa()`: it records its arguments and answers what the scenario
        /// says, with a window made of the clock and the two ends.
        fn lookup(&self, _: &Arc<Host>, chart: &Arc<Chart>, request: &ValueRequest) -> ValueResult {
            let (code, value, null) = self.script(chart, |script| script.lookup);
            if self.script(chart, |script| std::mem::take(&mut script.free_at_lookup)) {
                self.free(chart);
            }
            // C's numbers of QUERY_SOURCE_HEALTH and of the priority the lookup asks for
            let priority = match request.priority {
                QueryPriority::Synchronous => 7,
                other => panic!("a lookup at priority {other:?}"),
            };
            self.calls.borrow_mut().push(vec![
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
                b"4".to_vec(),
                text(priority),
                text(code),
            ]);
            match code {
                500 => ValueResult { code, value: f64::NAN, window: None, value_is_null: true },
                400 => ValueResult { code, value: f64::NAN, window: Some((0, 0)), value_is_null: true },
                _ => {
                    let window = (self.clock() + request.after + request.before, self.clock() + request.before);
                    ValueResult { code, value, window: Some(window), value_is_null: null }
                }
            }
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
            self.calls.borrow_mut().push(vec![b"load".to_vec(), text(u8::from(database))]);
            let real = self.real.borrow();
            let Some(real) = real.as_ref() else {
                // The generator's scripted load seeds the host's ids and returns. C's own load, over an empty
                // table, looks at the service once (before the step that finds no row) and writes its record: the
                // look is given back here, the record is left out where the records are compared.
                self.free_look.set(database);
                return database.then(Vec::new);
            };
            let (hostname, id, queue) = (host.hostname(), host_id(host), self.aclk_config.get());
            let (now, now_usec) = (self.clock(), self.now_usec());
            let mut transition_id = || self.transition_id();
            let running = || self.is_running();
            real.meta.check_removed_alerts_state(&hostname, &id, &running, queue, now, now_usec, &mut transition_id);
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
            self.calls.borrow_mut().push(call.collect());
            (alarm_id != 0).then_some((alarm_id, next_event_id))
        }

        /// The stub of `metadata_queue_ae_save()`: the queue takes the save, keeping the host's alerts and the
        /// entry's id for the scenario's `store`, or refuses it.
        fn queue_save(&self, alerts: &Arc<HostAlerts>, unique_id: u32) -> bool {
            let accepts = self.queue_accepts.get();
            self.calls.borrow_mut().push(vec![b"queue".to_vec(), text(unique_id), text(u8::from(accepts))]);
            if accepts {
                self.queued.borrow_mut().push((Arc::clone(alerts), unique_id));
            }
            accepts
        }

        /// The stub of `sql_health_alarm_log_save()`: it records the entry as it stands; then a real database
        /// gets its row, and a scripted one marks the entry as saved when the scenario says so.
        fn sql_save(&self, host: &Host, entry: &Entry) -> bool {
            self.calls.borrow_mut().push(vec![b"save".to_vec(), text(entry.unique_id), hex8(entry.flags)]);
            match self.real.borrow().as_ref() {
                Some(real) => {
                    sql::save(&real.meta, &host.hostname(), &host_id(host), entry, self.aclk_config.get())
                }
                None => self.saved.get(),
            }
        }

        fn commit_transitions(&self) {
            self.calls.borrow_mut().push(vec![b"commit_alert_transitions".to_vec()]);
        }

        fn process_pending_queue(&self, _: &Host) -> bool {
            self.calls.borrow_mut().push(vec![b"process_alert_pending_queue".to_vec()]);
            false
        }

        fn notify(&self, entry: &mut Entry) {
            self.calls.borrow_mut().push(vec![
                b"notify".to_vec(),
                text(entry.unique_id),
                text(entry.alarm_id),
                text(entry.alarm_event_id),
                entry.old_status.name().as_bytes().to_vec(),
                entry.new_status.name().as_bytes().to_vec(),
                text(entry.when),
                text(entry.delay_up_to_timestamp),
                hex8(entry.flags),
                text(entry.duration),
                text(entry.non_clear_duration),
                text(entry.delay),
                text(entry.last_repeat),
                double(entry.old_value),
                double(entry.new_value),
                uuid_rank(&entry.transition_id),
            ]);
        }
    }

    fn host() -> Arc<Host> {
        let info = HostInfo {
            hostname: "oracle-host".into(),
            registry_hostname: "oracle-host".into(),
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

    /// A double as both sides print it, in one form.
    fn canonical_double(text: &str) -> String {
        match text {
            "null" | "NaN" | "nan" => "nan".to_owned(),
            text => format!("{:?}", text.parse::<f64>().expect("a double")),
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
        /// What the reader of the rule files recorded, until the first step's rows are compared.
        reading: Vec<Captured>,
        /// The alert log's body an `alarm-log` step made, and the rules' answers a `configs` step made, until the
        /// step's rows are compared.
        body: Option<Vec<u8>>,
        configs: Vec<Fields>,
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
            let early = matches!(directive, "rules" | "database" | "retention" | "log-max");
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
                "aclk-config" => self.world.aclk_config.set(flag(args[0])),
                "sql" => {
                    let real = self.world.real.borrow();
                    let raw = &real.as_ref().unwrap_or_else(|| panic!("{}: no real database", self.name)).raw;
                    raw.execute_batch(rest).unwrap_or_else(|err| panic!("{}: {rest}: {err}", self.name));
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
                    self.world.script(&self.chart(args[0]), |script| script.lookup = lookup);
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
                        if let Some(real) = world.real.borrow().as_ref() {
                            sql::cleanup(&real.meta, &host_id(&self.host), alerts.as_deref(), &|| world.clock());
                        }
                        if let Some(alerts) = &alerts {
                            alerts.log_cleanup(&|| world.clock());
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
            for call in self.world.calls.borrow_mut().drain(..) {
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

            let mut expected = self.expected.remove(&self.step).unwrap_or_default();
            if let Some(calls) = expected.get_mut("call") {
                calls.retain(|call| match &call[0][..] {
                    b"queue" | b"save" | b"lookup" | b"notify" | b"sql_get_alarm_id" | b"load" => true,
                    b"commit_alert_transitions" | b"process_alert_pending_queue" => true,
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
                calls: RefCell::new(Vec::new()),
                transition_ids: Cell::new(0),
            };
            let mut replay = Replay {
                expected: expected.remove(&name).unwrap_or_else(|| panic!("{vectors} has no scenario {name}")),
                name,
                health: None,
                rules: Vec::new(),
                retention_s: None,
                log_max: None,
                reading: Vec::new(),
                body: None,
                configs: Vec::new(),
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
    assert_eq!(replayed("loop"), 218);
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
/// pass, with the REMOVED rows it injects, rows it refuses, and a service that stops while it loads.
#[test]
fn sql_matches_c() {
    assert_eq!(replayed("sql"), 88);
}

/// The steps a family's replay compared; any difference from C's rows fails.
fn replayed(family: &str) -> usize {
    let (steps, failures) = replay::run(family);
    let shown = failures[..failures.len().min(12)].join("\n");
    assert!(failures.is_empty(), "{family}: {} differences over {steps} steps:\n{shown}", failures.len());
    steps
}
