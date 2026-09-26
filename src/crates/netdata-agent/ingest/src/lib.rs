//! The protocol handlers that apply plugins.d/streaming keywords to hosts, charts and storage, ported from
//! `src/plugins.d/pluginsd_parser.c`, `pluginsd_replication.c`, `pluginsd_internals.h` and
//! `src/streaming/stream-replication-receiver.c` (`replicate_chart_request()`). Spec: `knowledge/spec-ingest.md` §2 and
//! §3.6 in the status repository.
//!
//! One `Parser` per connection, driven by the stream thread that owns the connection (decisions D8). Every handler
//! error disconnects, as on the C streaming parser.

#![forbid(unsafe_code)]

mod jsonc;
pub mod stream_path;

use std::sync::Arc;

use netdata_agent_log::{
    ErrorLimit, Field, FrameGuard, Priority, Source, Value, nd_log, nd_log_limit, push,
};

thread_local! {
    /// The line being parsed, for the log records written while it is (C's parser `request` callback reads the
    /// splitter's words). Empty between lines and for deferred payload lines.
    static LINE: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A record under the parser's frame.
macro_rules! plog {
    ($self:expr, $($arg:tt)+) => {{
        let _frame = $self.log_frame();
        nd_log!($($arg)+)
    }};
}
use netdata_agent_nrpc as nrpc;
use netdata_agent_pluginsd_proto::{
    CHART_SLOT_MAX, DIMENSION_SLOT_MAX, Deferred, DeferredBody, Keyword, MAX_DEFERRED_SIZE,
    Repertoire, Words, caps,
};
use netdata_agent_rrd::chart::{Algorithm, Chart, ChartSpec, ChartType, Dim, dim_flags, flags};
use netdata_agent_rrd::collection;
use netdata_agent_rrd::contexts;
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::labels::{self, Labels};
use netdata_agent_storage::storage_number::{SN_EMPTY_SLOT, SN_FLAG_NOT_ANOMALOUS, SN_FLAG_RESET};
use netdata_agent_text::parse::{
    str2i, str2ll, str2ll_encoded, str2ndd_encoded, str2u, str2ul, str2ull_encoded,
    uuid_parse_flexi,
};

/// `PLUGINS_FUNCTIONS_TIMEOUT_DEFAULT`: seconds.
const FUNCTIONS_TIMEOUT_DEFAULT: i32 = 10;

/// The fixed inputs of a parser.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// The negotiated stream capabilities.
    pub capabilities: u32,
    /// `parser->user.cd->update_every`: `[db] update every`.
    pub update_every: i32,
    /// `sysconf(_SC_PAGESIZE)`, for ring sizes.
    pub page_size: i64,
    /// The wall clock as (seconds, microseconds) (`now_realtime_timeval()`), replaceable in tests.
    pub now: fn() -> (i64, i64),
    /// `gap_when_lost_iterations_above`: `[db] gap when lost iterations above` + 2.
    pub gap_when_lost_iterations_above: i64,
}

/// `parser->user.v2`.
#[derive(Debug, Clone, Copy, Default)]
struct V2 {
    end_time: i64,
}

/// `parser->user.replay`.
#[derive(Debug, Clone, Copy, Default)]
struct Replay {
    start_time: i64,
    end_time: i64,
    rset_enabled: bool,
}

/// A handler refused the line: the connection ends. The text, when set, is what `PLUGINSD_DISABLE_PLUGIN` logs.
#[derive(Debug)]
struct Refused(Option<String>);

type Rc = Result<(), Refused>;

fn refuse() -> Rc {
    Err(Refused(None))
}

fn refuse_with(keyword: &str, why: &str) -> Rc {
    Err(Refused(Some(format!("PLUGINSD: keyword {keyword}: {why}"))))
}

fn text(v: &[u8]) -> String {
    String::from_utf8_lossy(v).into_owned()
}

/// `pluginsd_parse_storage_number_flags()`: `A` not anomalous, `R` reset, `E` an empty slot at once.
fn parse_sn_flags(flags: &[u8]) -> u32 {
    let mut out = 0;
    for &c in flags {
        match c {
            b'A' => out |= SN_FLAG_NOT_ANOMALOUS,
            b'R' => out |= SN_FLAG_RESET,
            b'E' => return SN_EMPTY_SLOT,
            _ => {}
        }
    }
    out
}

/// What finishing a deferred body does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnDone {
    Nothing,
    /// `pluginsd_function_result_end()` counts a data collection.
    CountCollection,
    /// `pluginsd_json_stream_paths()`.
    StreamPath,
}

/// The receiver side of one connection.
pub struct Parser {
    host: Arc<Host>,
    /// `localhost`, whose entry the stream path sent back to the child carries.
    localhost: Arc<Host>,
    config: Config,
    line: usize,
    scope: Option<Arc<Chart>>,
    clabel_count: usize,
    clabel_changed: bool,
    v2: V2,
    replay: Replay,
    /// `rpt->replication.first_time_s`: the oldest `after` this connection requested.
    replication_first_s: i64,
    /// `host->stream.rcv.pluginsd_chart_slots`.
    chart_slots: Vec<Option<Arc<Chart>>>,
    /// `parser->user.data_collections_count`.
    pub data_collections_count: u64,
    deferred: Option<DeferredBody>,
    on_done: OnDone,
    /// `parser->user.new_host_labels`: collected by LABEL until OVERWRITE.
    new_host_labels: Option<Labels>,
    /// Bytes for the child (`send_to_plugin`), drained by the caller.
    out: Vec<u8>,
}

impl Parser {
    pub fn new(host: Arc<Host>, localhost: Arc<Host>, config: Config) -> Self {
        Parser {
            host,
            localhost,
            config,
            line: 0,
            scope: None,
            clabel_count: 0,
            clabel_changed: false,
            v2: V2::default(),
            replay: Replay::default(),
            replication_first_s: 0,
            chart_slots: Vec::new(),
            data_collections_count: 0,
            deferred: None,
            on_done: OnDone::Nothing,
            new_host_labels: None,
            out: Vec::new(),
        }
    }

    /// What must be written to the child.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Processes one line (newline included). `false` ends the connection.
    pub fn feed(&mut self, line: &[u8]) -> bool {
        self.line += 1;
        if let Some(body) = &mut self.deferred {
            return match body.feed(line) {
                Deferred::Continue => true,
                // ML_MODEL payloads come with ML.
                Deferred::Done(body) => {
                    self.deferred = None;
                    match self.on_done {
                        OnDone::Nothing => {}
                        OnDone::CountCollection => self.data_collections_count += 1,
                        OnDone::StreamPath => self.stream_path_received(&body),
                    }
                    true
                }
                // Only JSON bodies are kept, and a receiver's plugin has no file name.
                Deferred::TooBig(size) => {
                    plog!(
                        self,
                        Source::Daemon,
                        Priority::Err,
                        "PLUGINSD: deferred response is too big ({size} bytes, limit {MAX_DEFERRED_SIZE} bytes) while waiting for keyword 'JSON_PAYLOAD_END' from plugin '' (transaction 'none'). Stopping this plugin."
                    );
                    false
                }
            };
        }
        let words = Words::split(line);
        let Some(first) = words.get(0) else {
            return true;
        };
        LINE.with(|l| {
            let mut l = l.borrow_mut();
            l.clear();
            l.extend_from_slice(line);
        });
        let ok = self.act(first, &words);
        LINE.with(|l| l.borrow_mut().clear());
        ok
    }

    /// `parser_action()`: the keyword's handler; a refused line is logged and ends the connection.
    fn act(&mut self, first: &[u8], words: &Words) -> bool {
        let result = match Keyword::lookup(first) {
            Some(keyword) if keyword.repertoire().contains(Repertoire::STREAMING) => {
                self.dispatch(keyword, words)
            }
            _ => Err(Refused(None)),
        };
        match result {
            Ok(()) => true,
            Err(Refused(why)) => {
                if let Some(why) = why {
                    // PLUGINSD_DISABLE_PLUGIN(): one limiter shared by every keyword and parser
                    static DISABLED: ErrorLimit = ErrorLimit::new(1, 0);
                    let _frame = self.log_frame();
                    nd_log_limit!(&DISABLED, Source::Collector, Priority::Info, "{why}");
                }
                let line_no = self.line;
                let shown = text(&words.reconstruct());
                plog!(
                    self,
                    Source::Daemon,
                    Priority::Err,
                    "PLUGINSD: parser_action('{}') failed on line {line_no}: {{ {shown} }} (quotes added to show parsing)",
                    text(first)
                );
                false
            }
        }
    }

    /// The fields C's parser callbacks add to every record written while this parser works: the line being parsed
    /// (re-quoted word by word), the host, and the scope chart's name and context. The missing ones are set but
    /// print nothing, as a callback returning false.
    pub fn log_frame(&self) -> FrameGuard {
        let none = || Value::lazy(|_| false);
        let request = LINE.with(|l| {
            let l = l.borrow();
            let words = Words::split(&l);
            if words.get(0).is_some() {
                Value::Txt(text(&words.reconstruct()))
            } else {
                none()
            }
        });
        let (instance, context) = match &self.scope {
            Some(chart) => {
                let meta = chart.meta();
                let name = meta.name.unwrap_or_else(|| chart.id().to_string());
                (Value::Str(name), Value::Str(meta.context))
            }
            None => (none(), none()),
        };
        push(vec![
            (Field::Request, request),
            (Field::NidlNode, Value::Str(self.host.hostname())),
            (Field::NidlInstance, instance),
            (Field::NidlContext, context),
        ])
    }

    fn dispatch(&mut self, keyword: Keyword, w: &Words) -> Rc {
        match keyword {
            Keyword::Begin => self.begin(w),
            Keyword::Set => self.set(w),
            Keyword::End => self.end(w),
            Keyword::Chart => self.chart(w),
            Keyword::Dimension => self.dimension(w),
            Keyword::Clabel => self.clabel(w),
            Keyword::ClabelCommit => self.clabel_commit(),
            Keyword::Begin2 => self.begin2(w),
            Keyword::Set2 => self.set2(w),
            Keyword::End2 => self.end2(),
            Keyword::ChartDefinitionEnd => self.chart_definition_end(w),
            Keyword::Rbegin => self.replay_begin(w),
            Keyword::Rset => self.replay_set(w),
            Keyword::Rdstate => self.replay_rrddim_state(w),
            Keyword::Rsstate => self.replay_rrdset_state(w),
            Keyword::Rend => self.replay_end(w),
            Keyword::Variable => self.variable(w),
            Keyword::Label => self.label(w),
            Keyword::Overwrite => self.overwrite(),
            Keyword::ClaimedId => self.claimed_id(w),
            Keyword::Json => {
                self.json(w);
                Ok(())
            }
            Keyword::Function => self.function(w),
            Keyword::FunctionDel => self.function_del(w),
            Keyword::FunctionResultBegin => {
                self.function_result_begin(w);
                Ok(())
            }
            Keyword::FunctionProgress => {
                self.function_progress(w);
                Ok(())
            }
            // Obsolete: accepted without effect (`pluginsd_dyncfg_noop()`).
            Keyword::DyncfgEnable
            | Keyword::DyncfgRegisterModule
            | Keyword::DyncfgRegisterJob
            | Keyword::DyncfgReset
            | Keyword::ReportJobStatus
            | Keyword::DeleteJob => Ok(()),
            // Outside the streaming repertoire: `feed()` never dispatches these.
            _ => refuse(),
        }
    }

    // ---- scope and caches (pluginsd_internals.h) ----

    /// `pluginsd_clear_scope_chart()`.
    fn clear_scope(&mut self) {
        self.scope = None;
        self.clabel_count = 0;
        self.clabel_changed = false;
    }

    /// `pluginsd_set_scope_chart()`.
    fn set_scope(&mut self, chart: &Arc<Chart>) {
        self.clear_scope();
        chart.receiver().pos = 0;
        self.scope = Some(Arc::clone(chart));
    }

    /// `pluginsd_require_scope_chart()`.
    fn require_scope(&mut self, keyword: &str, parent: &str) -> Result<Arc<Chart>, Refused> {
        match &self.scope {
            Some(chart) => Ok(Arc::clone(chart)),
            None => {
                plog!(
                    self,
                    Source::Daemon,
                    Priority::Err,
                    "PLUGINSD: command {keyword} requires a chart defined via command {parent}, but is not set."
                );
                Err(Refused(None))
            }
        }
    }

    /// `pluginsd_find_chart()`.
    fn find_chart(&mut self, id: Option<&[u8]>, keyword: &str) -> Option<Arc<Chart>> {
        let hostname = self.host.hostname();
        let Some(id) = id.filter(|i| !i.is_empty()) else {
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: 'host:{hostname}' got a {keyword} without a chart id."
            );
            return None;
        };
        let chart = self.host.charts().find(&text(id));
        if chart.is_none() {
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: 'host:{hostname}/chart:{}' got a {keyword} but chart does not exist.",
                text(id)
            );
        }
        chart
    }

    /// `pluginsd_rrdset_cache_put_to_slot()`.
    fn chart_to_slot(&mut self, chart: &Arc<Chart>, slot: Option<u64>) {
        for entry in &mut self.chart_slots {
            if entry.as_ref().is_some_and(|c| Arc::ptr_eq(c, chart)) {
                *entry = None;
            }
        }
        let Some(slot) = slot.filter(|&s| s >= 1 && s < i32::MAX as u64) else {
            return;
        };
        let slot = slot as usize;
        if slot > self.chart_slots.len() {
            self.chart_slots
                .resize(slot.max(self.chart_slots.len() * 2), None);
        }
        self.chart_slots[slot - 1] = Some(Arc::clone(chart));
    }

    /// `pluginsd_rrdset_cache_get_from_slot()`: a cached slot wins over the id.
    fn chart_from_slot(
        &mut self,
        id: Option<&[u8]>,
        slot: Option<u64>,
        keyword: &str,
    ) -> Option<Arc<Chart>> {
        match slot {
            Some(s) if s >= 1 && (s as usize) <= self.chart_slots.len() => {
                if let Some(chart) = &self.chart_slots[s as usize - 1] {
                    return Some(Arc::clone(chart));
                }
                let chart = self.find_chart(id, keyword)?;
                self.chart_to_slot(&chart, Some(s));
                Some(chart)
            }
            _ => self.find_chart(id, keyword),
        }
    }

    /// `pluginsd_rrddim_put_to_slot()`.
    fn dim_to_slot(chart: &Chart, dim: &Arc<Dim>, slot: Option<u64>) {
        let count = chart.dim_count();
        let mut state = chart.receiver();
        let wanted = match slot.filter(|&s| s >= 1) {
            Some(s) => {
                state.dims_with_slots = true;
                s as usize
            }
            None => {
                state.dims_with_slots = false;
                count
            }
        };
        if wanted > state.prd.len() {
            state.prd.resize(wanted, None);
        }
        if let Some(s) =
            slot.filter(|&s| state.dims_with_slots && s >= 1 && s as usize <= state.prd.len())
        {
            let entry = &mut state.prd[s as usize - 1];
            if !entry.as_ref().is_some_and(|d| Arc::ptr_eq(d, dim)) {
                *entry = Some(Arc::clone(dim));
            }
        }
    }

    /// `pluginsd_acquire_dimension()`.
    fn acquire_dim(
        &mut self,
        chart: &Chart,
        id: Option<&[u8]>,
        slot: Option<u64>,
        keyword: &str,
    ) -> Option<Arc<Dim>> {
        let hostname = self.host.hostname();
        let Some(id) = id.filter(|i| !i.is_empty()) else {
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: 'host:{hostname}/chart:{}' got a {keyword}, without a dimension.",
                chart.id()
            );
            return None;
        };
        let id = text(id);
        let mut state = chart.receiver();
        if state.prd.is_empty() {
            drop(state);
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: 'host:{hostname}/chart:{}' got a {keyword}, but the chart has no dimensions.",
                chart.id()
            );
            return None;
        }
        let size = state.prd.len();
        let position = if state.dims_with_slots {
            let s = slot.unwrap_or(0);
            if s < 1 || s as usize > size {
                drop(state);
                plog!(
                    self,
                    Source::Daemon,
                    Priority::Err,
                    "PLUGINSD: 'host:{hostname}/chart:{}' got a {keyword} with slot {}, but slots in the range [1 - {size}] are expected.",
                    chart.id(),
                    slot.map_or(-1, |s| s as i64)
                );
                return None;
            }
            if let Some(dim) = &state.prd[s as usize - 1] {
                return Some(Arc::clone(dim));
            }
            s as usize - 1
        } else {
            let pos = if state.pos >= size { 0 } else { state.pos };
            state.pos = pos + 1;
            match &state.prd[pos] {
                Some(dim) if dim.id() == id => return Some(Arc::clone(dim)),
                Some(_) => state.prd[pos] = None,
                None => {}
            }
            pos
        };
        drop(state);
        let Some(dim) = chart.dim(&id) else {
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: 'host:{hostname}/chart:{}/dim:{id}' got a {keyword} but dimension does not exist.",
                chart.id()
            );
            return None;
        };
        chart.receiver().prd[position] = Some(Arc::clone(&dim));
        Some(dim)
    }

    // ---- metadata ----

    /// `pluginsd_chart()`.
    fn chart(&mut self, w: &Words) -> Rc {
        let slot = w.slot(CHART_SLOT_MAX);
        let mut idx = if slot.is_some() { 2 } else { 1 };
        let mut next = || {
            let word = w.get(idx);
            idx += 1;
            word
        };
        let type_id = next();
        let name = next();
        let title = next();
        let units = next();
        let family = next();
        let context = next();
        let chart_type = next();
        let priority_s = next();
        let update_every_s = next();
        let options = next();
        let plugin = next();
        let module = next();
        let (type_, id) = match type_id.and_then(|t| {
            t.iter()
                .position(|&c| c == b'.')
                .map(|dot| (&t[..dot], &t[dot + 1..]))
        }) {
            Some((t, i)) if !t.is_empty() && !i.is_empty() => (text(t), text(i)),
            _ => return refuse_with("CHART", "missing parameters"),
        };
        let mut name = name.map(|n| {
            let prefix = format!("{type_}.");
            n.strip_prefix(prefix.as_bytes()).unwrap_or(n)
        });
        if let Some(n) = name {
            if n == id.as_bytes()
                || n.eq_ignore_ascii_case(b"NULL")
                || n.eq_ignore_ascii_case(b"(NULL)")
            {
                name = None;
            }
        }
        let priority = match priority_s.filter(|p| !p.is_empty()) {
            Some(p) => i64::from(str2i(p)),
            None => 1000,
        };
        let mut update_every = self.config.update_every;
        if let Some(u) = update_every_s.filter(|u| !u.is_empty()) {
            update_every = str2i(u);
        }
        if update_every == 0 {
            update_every = self.config.update_every;
        }
        let chart_type = chart_type.map_or(ChartType::Line, ChartType::from_name);
        let non_empty = |v: Option<&[u8]>| v.filter(|v| !v.is_empty()).map(text);
        let name = non_empty(name);
        let family = non_empty(family);
        let context = non_empty(context);
        let title = title.map_or_else(String::new, text);
        let units = units.map_or_else(|| "unknown".to_string(), text);
        let plugin = non_empty(plugin).unwrap_or_default();
        let module = module.map(text);
        let info = self.host.info();
        let (chart, _) = self.host.charts().create(&ChartSpec {
            type_: &type_,
            id: &id,
            name: name.as_deref(),
            family: family.as_deref(),
            context: context.as_deref(),
            title: &title,
            units: &units,
            plugin: &plugin,
            module: module.as_deref(),
            priority,
            update_every,
            chart_type,
            mode: info.db_mode,
            history_entries: info.history_entries,
            page_size: self.config.page_size,
        });
        match options.filter(|o| !o.is_empty()) {
            Some(o) => {
                let has = |what: &[u8]| o.windows(what.len()).any(|x| x == what);
                if has(b"obsolete") {
                    chart.is_obsolete();
                } else {
                    chart.isnot_obsolete();
                }
                chart.update_meta(|m| {
                    for (word, flag) in [
                        (&b"hidden"[..], flags::HIDDEN),
                        (b"store_first", flags::STORE_FIRST),
                    ] {
                        if has(word) {
                            m.flags |= flag;
                        } else {
                            m.flags &= !flag;
                        }
                    }
                });
            }
            None => chart.update_meta(|m| m.flags &= !flags::STORE_FIRST),
        }
        self.set_scope(&chart);
        self.chart_to_slot(&chart, slot);
        Ok(())
    }

    /// `pluginsd_dimension()`.
    fn dimension(&mut self, w: &Words) -> Rc {
        let slot = w.slot(DIMENSION_SLOT_MAX);
        let base = if slot.is_some() { 2 } else { 1 };
        let id = w.get(base);
        let name = w.get(base + 1);
        let algorithm = w.get(base + 2);
        let multiplier_s = w.get(base + 3);
        let divisor_s = w.get(base + 4);
        let options = w.get(base + 5);
        let chart = self.require_scope("DIMENSION", "CHART")?;
        let Some(id) = id.filter(|i| !i.is_empty()) else {
            return refuse_with("DIMENSION", "missing dimension id");
        };
        let number = |v: Option<&[u8]>| match v.filter(|v| !v.is_empty()).map(str2ll_encoded) {
            Some(0) | None => 1,
            Some(n) => n,
        };
        let multiplier = number(multiplier_s) as i32;
        let divisor = number(divisor_s) as i32;
        let algorithm = algorithm
            .filter(|a| !a.is_empty())
            .map_or(Algorithm::Absolute, Algorithm::from_name);
        let name = name.map(text);
        let (dim, _) = chart.dim_add(&text(id), name.as_deref(), multiplier, divisor, algorithm);
        let options = options.filter(|o| !o.is_empty());
        let has = |what: &[u8]| options.is_some_and(|o| o.windows(what.len()).any(|x| x == what));
        if has(b"obsolete") {
            chart.dim_is_obsolete(&dim);
        } else {
            chart.dim_isnot_obsolete(&dim);
        }
        dim.update_meta(|m| {
            m.flags &= !dim_flags::DONT_DETECT_RESETS_OR_OVERFLOWS;
            // Without options every word is absent: shown, resets detected, the value type kept.
            let hidden = has(b"hidden");
            if has(b"noreset") || has(b"nooverflow") {
                m.flags |= dim_flags::DONT_DETECT_RESETS_OR_OVERFLOWS;
            }
            if has(b"type=float") {
                if m.flags & dim_flags::FLOAT == 0 {
                    dim.update_collection(|c| {
                        c.collected_value = 0;
                        c.collected_value_float = 0.0;
                    });
                }
                m.flags |= dim_flags::FLOAT;
            } else if has(b"type=int") {
                if m.flags & dim_flags::FLOAT != 0 {
                    dim.update_collection(|c| {
                        c.collected_value = 0;
                        c.collected_value_float = 0.0;
                    });
                }
                m.flags &= !dim_flags::FLOAT;
            }
            if hidden {
                m.flags |= dim_flags::HIDDEN;
            } else {
                m.flags &= !dim_flags::HIDDEN;
            }
        });
        Self::dim_to_slot(&chart, &dim, slot);
        Ok(())
    }

    /// `pluginsd_clabel()`.
    fn clabel(&mut self, w: &Words) -> Rc {
        let (Some(name), Some(value), Some(source)) = (w.get(1), w.get(2), w.get(3)) else {
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "Ignoring malformed or empty CHART LABEL command."
            );
            return refuse();
        };
        let Some(chart) = self.scope.clone() else {
            return refuse_with("CLABEL", "Got CHART LABEL without a chart");
        };
        let first = self.clabel_count == 0;
        self.clabel_count += 1;
        let source = netdata_agent_text::parse::str2l(source) as u32;
        let changed = chart.update_meta(|m| {
            if first {
                m.labels.unmark_all();
            }
            m.labels.add_changed(name, value, source)
        });
        match changed {
            Ok(true) => self.clabel_changed = true,
            Ok(false) => {}
            Err(message) => plog!(self, Source::Daemon, Priority::Err, "{}", message),
        }
        Ok(())
    }

    /// `pluginsd_clabel_commit()`.
    fn clabel_commit(&mut self) -> Rc {
        let chart = self.require_scope("CLABEL_COMMIT", "BEGIN")?;
        if self.clabel_count == 0 {
            let hostname = self.host.hostname();
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: 'host:{hostname}' got CLABEL_COMMIT, without a CHART or BEGIN. Ignoring it."
            );
            return refuse();
        }
        let changed = chart.update_meta(|m| m.labels.remove_all_unmarked_and_changed())
            || self.clabel_changed;
        if changed {
            chart.update_meta(|m| m.flags |= flags::METADATA_UPDATE);
            chart.metadata_updated();
        }
        self.clabel_count = 0;
        self.clabel_changed = false;
        Ok(())
    }

    // ---- host metadata ----

    /// `pluginsd_variable()`: `[GLOBAL|HOST|LOCAL|CHART] name value`; the default is the chart in scope, else the
    /// host.
    fn variable(&mut self, w: &Words) -> Rc {
        let mut name = w.get(1);
        let mut value = w.get(2);
        let chart = self.scope.clone();
        let mut global = chart.is_none();
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            if n == b"GLOBAL" || n == b"HOST" {
                global = true;
                name = w.get(2);
                value = w.get(3);
            } else if n == b"LOCAL" || n == b"CHART" {
                global = false;
                name = w.get(2);
                value = w.get(3);
            }
        }
        let Some(name) = name.filter(|n| !n.is_empty()).map(text) else {
            return refuse_with("VARIABLE", "missing variable name");
        };
        let hostname = self.host.hostname();
        let chart_id = chart
            .as_ref()
            .map_or_else(|| "UNSET".to_string(), |c| c.id().to_string());
        let Some(value) = value.filter(|v| !v.is_empty()) else {
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: 'host:{hostname}/chart:{chart_id}' cannot set {} VARIABLE '{name}' to an empty value",
                if global { "HOST" } else { "CHART" }
            );
            return Ok(());
        };
        if !global && chart.is_none() {
            return refuse_with("VARIABLE", "no chart is defined and no GLOBAL is given");
        }
        let (v, used) = str2ndd_encoded(value);
        if used < value.len() {
            let message = if used == 0 {
                format!(
                    "PLUGINSD: 'host:{hostname}/chart:{chart_id}' the value '{}' of VARIABLE '{name}' cannot be parsed as a number",
                    text(value)
                )
            } else {
                format!(
                    "PLUGINSD: 'host:{hostname}/chart:{chart_id}' the value '{}' of VARIABLE '{name}' has leftovers: '{}'",
                    text(value),
                    text(&value[used..])
                )
            };
            plog!(self, Source::Daemon, Priority::Err, "{}", message);
        }
        match (global, &chart) {
            (false, Some(chart)) => chart.set_variable(&name, v),
            _ => self.host.set_variable(&name, v),
        }
        Ok(())
    }

    /// `pluginsd_label()`: `name source value...`; extra words join the value with single spaces.
    fn label(&mut self, w: &Words) -> Rc {
        let (Some(name), Some(source), Some(first)) = (w.get(1), w.get(2), w.get(3)) else {
            return refuse_with("LABEL", "missing parameters");
        };
        let mut value = first.to_vec();
        if w.len() > 4 {
            let mut remaining = netdata_agent_pluginsd_proto::LINE_MAX;
            value.clear();
            let mut i = 3;
            while i < w.len() && remaining > 2 {
                let Some(word) = w.get(i) else { break };
                if i > 3 {
                    value.push(b' ');
                    remaining -= 1;
                }
                let length = word.len().min(remaining);
                remaining -= length;
                value.extend_from_slice(&word[..length]);
                i += 1;
            }
        }
        let source = netdata_agent_text::parse::str2l(source) as u32;
        self.new_host_labels
            .get_or_insert_with(Labels::default)
            .add(name, &value, source);
        Ok(())
    }

    /// `pluginsd_overwrite()`: the collected labels replace the host's; `_is_ephemeral`, `_os` and `_hostname` are
    /// kept up to date.
    fn overwrite(&mut self) -> Rc {
        let new = self.new_host_labels.take();
        let info = self.host.info();
        let ephemeral = self.host.update_labels(|labels| {
            if let Some(new) = &new {
                labels.migrate_to_these(new);
            }
            // pluginsd_update_host_ephemerality()
            let ephemeral = labels
                .get(b"_is_ephemeral")
                .is_some_and(|v| !v.is_empty() && netdata_agent_inicfg::test_boolean_value(v));
            labels.add(
                b"_is_ephemeral",
                if ephemeral { b"true" } else { b"false" },
                labels::SRC_CONFIG,
            );
            if !labels.exists(b"_os") {
                labels.add(b"_os", info.os.as_bytes(), labels::SRC_AUTO);
            }
            if !labels.exists(b"_hostname") {
                labels.add(b"_hostname", info.hostname.as_bytes(), labels::SRC_AUTO);
            }
            ephemeral
        });
        self.host.set_ephemeral(ephemeral);
        Ok(())
    }

    /// `stream_receiver_pluginsd_claimed_id()`.
    fn claimed_id(&mut self, w: &Words) -> Rc {
        let (Some(guid), Some(claim)) = (w.get(1), w.get(2)) else {
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: command CLAIMED_ID came malformed, machine_guid '{}', claim_id '{}'",
                w.get(1).map_or_else(|| "[unset]".to_string(), text),
                w.get(2).map_or_else(|| "[unset]".to_string(), text)
            );
            return refuse();
        };
        if uuid_parse_flexi(guid).is_none() {
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: parameter machine guid to CLAIMED_ID command is not valid UUID. Received: '{}'.",
                text(guid)
            );
            return refuse();
        }
        let claim_uuid = if claim == b"NULL" {
            [0; 16]
        } else {
            match uuid_parse_flexi(claim) {
                Some(u) => u,
                None => {
                    plog!(
                        self,
                        Source::Daemon,
                        Priority::Err,
                        "PLUGINSD: parameter claim id to CLAIMED_ID command is not valid UUID. Received: '{}'.",
                        text(claim)
                    );
                    return refuse();
                }
            }
        };
        if guid != self.host.machine_guid().as_bytes() {
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: received claim id for host '{}' but it came over the connection of '{}'",
                text(guid),
                self.host.machine_guid()
            );
            return Ok(());
        }
        self.host.set_claim_id_of_origin(claim_uuid);
        Ok(())
    }

    // ---- functions (pluginsd_functions.c) ----

    /// `pluginsd_function()`: `[GLOBAL] name timeout help tags access priority version`, always host-wide.
    fn function(&mut self, w: &Words) -> Rc {
        let global = w.len() >= 2 && w.get(1) == Some(b"GLOBAL");
        let i = if global { 2 } else { 1 };
        let name = w.get(i);
        let timeout_s = w.get(i + 1);
        let help = w.get(i + 2);
        let tags = w.get(i + 3);
        let access = w.get(i + 4);
        let priority = w.get(i + 5);
        let version = w.get(i + 6);
        let hostname = self.host.hostname();
        let (Some(name), Some(timeout_s), Some(help)) = (name, timeout_s, help) else {
            let shown = |v: Option<&[u8]>| v.map_or_else(|| "(unset)".to_string(), text);
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: 'host:{hostname}' got a FUNCTION, without providing the required data (global = '{}', name = '{}', timeout = '{}', priority = '{}', version = '{}', help = '{}'). Ignoring it.",
                if global { "yes" } else { "no" },
                shown(name),
                shown(timeout_s),
                shown(priority),
                shown(version),
                shown(help)
            );
            return refuse();
        };
        if !global && let Some(chart) = &self.scope {
            plog!(
                self,
                Source::Daemon,
                Priority::Notice,
                "PLUGINSD: 'host:{hostname}' got a FUNCTION '{}' within chart '{}' scope - chart-scoped functions are no longer supported, registering it host-wide",
                text(name),
                chart.id()
            );
        }
        let positive_or =
            |v: Option<&[u8]>, default: i32| match v.filter(|v| !v.is_empty()).map(str2i) {
                Some(n) if n > 0 => n,
                _ => default,
            };
        let registered = self.host.functions().register(
            &hostname,
            &nrpc::MethodDesc {
                name,
                help,
                tags: tags.unwrap_or(b""),
                timeout_s: positive_or(Some(timeout_s), FUNCTIONS_TIMEOUT_DEFAULT),
                priority: positive_or(priority, nrpc::PRIORITY_DEFAULT),
                version: version
                    .filter(|v| !v.is_empty())
                    .map_or(nrpc::VERSION_DEFAULT, str2u),
                access: nrpc::access::from_hex_mapping_old_roles(access.unwrap_or(b"")),
                sync: false,
                source: nrpc::Source::Stream,
            },
        );
        if let Err(warning) = registered {
            plog!(self, Source::Daemon, Priority::Warning, "{}", warning);
        }
        self.data_collections_count += 1;
        Ok(())
    }

    /// `pluginsd_function_del()`: `[GLOBAL] name`.
    fn function_del(&mut self, w: &Words) -> Rc {
        let i = if w.len() >= 2 && w.get(1) == Some(b"GLOBAL") {
            2
        } else {
            1
        };
        let hostname = self.host.hostname();
        let Some(name) = w.get(i).filter(|n| !n.is_empty()) else {
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: 'host:{hostname}' got a FUNCTION_DEL without a name. Ignoring it."
            );
            return refuse();
        };
        match self.host.functions().unregister(name, nrpc::Source::Stream) {
            nrpc::Unregistered::Removed => {}
            not_removed => {
                if let nrpc::Unregistered::Refused(warning) = not_removed {
                    plog!(self, Source::Daemon, Priority::Warning, "{}", warning);
                }
                plog!(
                    self,
                    Source::Daemon,
                    Priority::Debug,
                    "PLUGINSD: 'host:{hostname}' FUNCTION_DEL '{}' - function not found or ownership mismatch",
                    text(name)
                );
            }
        }
        self.data_collections_count += 1;
        Ok(())
    }

    /// `pluginsd_call_acquire()`: this parent never calls a child's functions yet, so no transaction is known.
    fn call_not_found(&mut self, keyword: &str, transaction: Option<&[u8]>) {
        plog!(
            self,
            Source::Daemon,
            Priority::Err,
            "got a {keyword} for transaction '{}', but the transaction is not found.",
            transaction.map_or_else(|| "(unset)".to_string(), text)
        );
    }

    /// `pluginsd_function_result_begin()`: `transaction status content_type expires`, then the body up to
    /// `FUNCTION_RESULT_END`.
    fn function_result_begin(&mut self, w: &Words) {
        let transaction = w.get(1);
        let fields = [w.get(1), w.get(2), w.get(3), w.get(4)];
        if fields.iter().any(|f| f.is_none_or(<[u8]>::is_empty)) {
            let shown = |v: Option<&[u8]>| v.map_or_else(|| "(unset)".to_string(), text);
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "got a FUNCTION_RESULT_BEGIN without providing the required data (key = '{}', status = '{}', format = '{}', expires = '{}').",
                shown(fields[0]),
                shown(fields[1]),
                shown(fields[2]),
                shown(fields[3])
            );
        }
        self.call_not_found("FUNCTION_RESULT_BEGIN", transaction);
        self.deferred = Some(DeferredBody::discarding("FUNCTION_RESULT_END"));
        self.on_done = OnDone::CountCollection;
    }

    /// `pluginsd_function_progress()`: `transaction done all`.
    fn function_progress(&mut self, w: &Words) {
        self.call_not_found("FUNCTION_PROGRESS", w.get(1));
    }

    /// `pluginsd_json()`: `JSON keyword`, then the payload up to `JSON_PAYLOAD_END`.
    fn json(&mut self, w: &Words) {
        let keyword = w.get(1).unwrap_or(b"");
        self.on_done = OnDone::Nothing;
        if keyword == b"STREAM_PATH" {
            self.on_done = OnDone::StreamPath;
        } else if keyword != b"ML_MODEL" {
            // ML_MODEL payloads come with ML
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: invalid JSON payload keyword '{}'",
                text(keyword)
            );
        }
        self.deferred = Some(DeferredBody::new("JSON_PAYLOAD_END"));
    }

    /// `pluginsd_json_stream_paths()` → `stream_path_set_from_json()`: a changed path goes back to the child with
    /// this agent's entry (`stream_path_send_to_child()`), inline, before anything the next lines produce.
    fn stream_path_received(&mut self, body: &[u8]) {
        if stream_path::set_from_json(&self.host, body) {
            self.send_stream_path(None);
        }
    }

    /// `stream_path_retention_updated()` for the child of this connection: the host's first time became
    /// `first_time_s`.
    pub fn retention_updated(&mut self, first_time_s: i64) {
        self.send_stream_path(Some(first_time_s));
    }

    /// `stream_path_send_to_child()`: only to a child that negotiated paths.
    fn send_stream_path(&mut self, first_time_t: Option<i64>) {
        if self.config.capabilities & caps::PATHS != 0 {
            let message = stream_path::message(&self.host, &self.localhost, first_time_t);
            self.out.extend_from_slice(&message);
        }
    }

    // ---- v1 data ----

    /// `now_realtime_timeval()`: v1 collection interpolates by the microseconds too.
    fn now_tv(&self) -> (i64, i64) {
        (self.config.now)()
    }

    /// `now_realtime_sec()`.
    fn now_s(&self) -> i64 {
        (self.config.now)().0
    }

    /// `pluginsd_begin()`: the duration since the previous collection, trusted as streaming does.
    fn begin(&mut self, w: &Words) -> Rc {
        let slot = w.slot(CHART_SLOT_MAX);
        let base = if slot.is_some() { 2 } else { 1 };
        let id = w.get(base);
        let microseconds_s = w.get(base + 1);
        let Some(chart) = self.chart_from_slot(id, slot, "BEGIN") else {
            return refuse();
        };
        self.set_scope(&chart);
        let microseconds = match microseconds_s.filter(|m| !m.is_empty()) {
            Some(m) => str2ll(m).0.max(0) as u64,
            None => 0,
        };
        if chart.collection().counter_done != 0 {
            let now = self.now_tv();
            if microseconds != 0 {
                collection::next_usec_unfiltered(&chart, now, microseconds);
            } else {
                collection::timed_next(&chart, now, 0);
            }
        }
        Ok(())
    }

    /// `pluginsd_set()`: an empty value collects nothing.
    fn set(&mut self, w: &Words) -> Rc {
        let slot = w.slot(DIMENSION_SLOT_MAX);
        let base = if slot.is_some() { 2 } else { 1 };
        let dimension = w.get(base);
        let value = w.get(base + 1);
        let chart = self.require_scope("SET", "CHART")?;
        let Some(dim) = self.acquire_dim(&chart, dimension, slot, "SET") else {
            return refuse();
        };
        chart.receiver().set = true;
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            let now = self.now_tv();
            if dim.meta().flags & dim_flags::FLOAT != 0 {
                collection::set_value_float(&dim, now, str2ndd_encoded(value).0);
            } else {
                collection::set_value(&dim, now, str2ll_encoded(value));
            }
        }
        Ok(())
    }

    /// `pluginsd_end()`: the collection time is the child's (words 1-2) or this host's clock.
    fn end(&mut self, w: &Words) -> Rc {
        let tv_sec = w.get(1);
        let tv_usec = w.get(2);
        let pending_next = w.get(3).is_some_and(|p| !p.is_empty());
        let chart = self.require_scope("END", "BEGIN")?;
        self.clear_scope();
        self.data_collections_count += 1;
        let number = |v: Option<&[u8]>| v.filter(|v| !v.is_empty()).map_or(0, |v| str2ll(v).0);
        let mut tv = (number(tv_sec), number(tv_usec));
        if tv.0 == 0 {
            tv = self.now_tv();
        }
        collection::timed_done(
            &chart,
            tv,
            pending_next,
            self.config.gap_when_lost_iterations_above,
        );
        Ok(())
    }

    // ---- v2 data ----

    /// `pluginsd_begin_v2()`.
    fn begin2(&mut self, w: &Words) -> Rc {
        let slot = w.slot(CHART_SLOT_MAX);
        let base = if slot.is_some() { 2 } else { 1 };
        let (Some(id), Some(ue), Some(end), Some(wall)) = (
            w.get(base),
            w.get(base + 1),
            w.get(base + 2),
            w.get(base + 3),
        ) else {
            return refuse_with("BEGIN2", "missing parameters");
        };
        let Some(chart) = self.chart_from_slot(Some(id), slot, "BEGIN2") else {
            return refuse();
        };
        self.set_scope(&chart);
        chart.isnot_obsolete();
        let update_every = str2ull_encoded(ue) as i64;
        let end_time = str2ull_encoded(end) as i64;
        let _wall_clock = if wall.first() == Some(&b'#') {
            end_time
        } else {
            str2ull_encoded(wall) as i64
        };
        if update_every != i64::from(chart.update_every()) {
            chart.set_update_every(update_every);
        }
        self.v2 = V2 { end_time };
        let entries = chart.entries();
        chart.update_collection(|c| {
            c.last_collected = (end_time, 0);
            c.last_updated = (end_time, 0);
            c.counter += 1;
            c.counter_done += 1;
            c.current_entry += 1;
            if c.current_entry >= entries {
                c.current_entry -= entries;
            }
        });
        Ok(())
    }

    /// `pluginsd_set_v2()`.
    fn set2(&mut self, w: &Words) -> Rc {
        let slot = w.slot(DIMENSION_SLOT_MAX);
        let base = if slot.is_some() { 2 } else { 1 };
        let (Some(dimension), Some(collected_s), Some(value_s), Some(flags_s)) = (
            w.get(base),
            w.get(base + 1),
            w.get(base + 2),
            w.get(base + 3),
        ) else {
            return refuse_with("SET2", "missing parameters");
        };
        let chart = self.require_scope("SET2", "BEGIN2")?;
        let Some(dim) = self.acquire_dim(&chart, Some(dimension), slot, "SET2") else {
            return refuse();
        };
        chart.receiver().set = true;
        chart.dim_isnot_obsolete(&dim);
        let is_float = dim.meta().flags & dim_flags::FLOAT != 0;
        let sender_sent_float = is_float && self.config.capabilities & caps::FLOAT_BASELINE != 0;
        let (collected, collected_d) = if sender_sent_float {
            (0, str2ndd_encoded(collected_s).0)
        } else {
            (str2ll_encoded(collected_s), 0.0)
        };
        let mut value = if value_s.first() == Some(&b'#') {
            if sender_sent_float {
                collected_d
            } else {
                collected as f64
            }
        } else {
            str2ndd_encoded(value_s).0
        };
        let mut sn_flags = parse_sn_flags(flags_s);
        // ML is not running here: without ML_MODELS the child's anomaly bits are kept as received.
        if !value.is_finite() || sn_flags == SN_EMPTY_SLOT {
            value = f64::NAN;
            sn_flags = SN_EMPTY_SLOT;
        }
        let end_time = self.v2.end_time;
        dim.store_metric(end_time as u64 * 1_000_000, value, sn_flags);
        dim.update_collection(|c| {
            c.last_collected_time = (end_time, 0);
            if sender_sent_float || is_float {
                c.collected_value_float = if sender_sent_float {
                    collected_d
                } else {
                    collected as f64
                };
            } else {
                c.collected_value = collected;
            }
            c.last_stored_value = value;
            c.last_calculated_value = value;
            c.counter += 1;
        });
        dim.update_meta(|m| m.flags |= dim_flags::UPDATED);
        Ok(())
    }

    /// `pluginsd_end_v2()`.
    fn end2(&mut self) -> Rc {
        let chart = self.require_scope("END2", "BEGIN2")?;
        self.data_collections_count += 1;
        for dim in chart.dims() {
            dim.update_collection(|c| {
                c.collected_value = 0;
                c.collected_value_float = 0.0;
                c.calculated_value = 0.0;
            });
            dim.update_meta(|m| m.flags &= !dim_flags::UPDATED);
        }
        self.v2 = V2::default();
        contexts::collected_rrdset(&chart);
        Ok(())
    }

    // ---- replication ----

    /// `pluginsd_chart_definition_end()`: the first of a round asks the child for the missing data (sent inline,
    /// decisions D16).
    fn chart_definition_end(&mut self, w: &Words) -> Rc {
        let chart = self.require_scope("CHART_DEFINITION_END", "CHART")?;
        let number = |v: Option<&[u8]>| v.filter(|v| !v.is_empty()).map_or(0, |v| str2ul(v) as i64);
        let first_entry = number(w.get(1));
        let last_entry = number(w.get(2));
        let wall = match w.get(3).filter(|v| !v.is_empty()) {
            Some(v) => str2ul(v) as i64,
            None => self.now_s(),
        };
        let was_in_progress = chart.update_meta(|m| {
            let old = m.flags;
            m.flags |= flags::RECEIVER_REPLICATION_IN_PROGRESS;
            m.flags &= !flags::RECEIVER_REPLICATION_FINISHED;
            old & flags::RECEIVER_REPLICATION_IN_PROGRESS != 0
        });
        if !was_in_progress {
            self.replicate_chart_request(&chart, first_entry, last_entry, wall, 0, 0);
        }
        Ok(())
    }

    /// `replicate_chart_request()`: sends `REPLAY_CHART` for what the child has and this host lacks, one step at a
    /// time; an empty request (`"true" 0 0`) ends replication for the chart.
    fn replicate_chart_request(
        &mut self,
        chart: &Chart,
        child_first: i64,
        mut child_last: i64,
        child_wall: i64,
        prev_after: i64,
        prev_before: i64,
    ) {
        let now = self.now_s();
        let info = self.host.info();
        if child_last > child_wall {
            child_last = child_wall;
        }
        let (local_first, local_last) = retention_for_collected_chart(chart, now);
        let gap_from = if prev_after == 0 || prev_before == 0 {
            if local_last != 0 {
                local_last
            } else if now > info.replication_period {
                now - info.replication_period
            } else {
                0
            }
        } else {
            prev_before
        };
        let gap_to = now;
        let _ = local_first;
        let empty = !info.replication_enabled
            || chart.dim_count() == 0
            || child_first == 0
            || child_last == 0
            || child_first < 0
            || child_last < 0
            || child_first > child_wall
            || child_first > child_last
            || local_last >= child_last;
        let (after, before, start_streaming) = if empty {
            (0, 0, true)
        } else {
            let after = child_first.max(gap_from);
            let mut before = if gap_to - after > info.replication_step {
                after + info.replication_step
            } else {
                gap_to
            };
            before = before.min(child_last);
            if after > before {
                (0, 0, true)
            } else {
                let start = now - after <= info.replication_step
                    || before >= child_last
                    || before >= child_wall
                    || before >= now;
                (after, before, start)
            }
        };
        {
            let mut state = chart.receiver();
            state.replay_start_streaming = start_streaming;
            state.replay_after = after;
            state.replay_before = before;
        }
        // send_replay_chart_cmd()
        if self.replication_first_s == 0 || after < self.replication_first_s {
            self.replication_first_s = after;
        }
        self.out.extend_from_slice(
            format!(
                "REPLAY_CHART \"{}\" \"{}\" {after} {before}\n",
                chart.id(),
                if start_streaming { "true" } else { "false" }
            )
            .as_bytes(),
        );
    }

    /// `stream_parse_enable_streaming()`.
    fn parse_enable_streaming(&self, v: Option<&[u8]>) -> bool {
        match v {
            None | Some(b"") => {
                plog!(
                    self,
                    Source::Daemon,
                    Priority::Err,
                    "REPLAY: malformed start_streaming boolean value empty"
                );
                false
            }
            Some(b"false") => false,
            Some(b"true") => true,
            Some(other) => {
                plog!(
                    self,
                    Source::Daemon,
                    Priority::Err,
                    "REPLAY: malformed start_streaming boolean value '{}'",
                    text(other)
                );
                false
            }
        }
    }

    /// `pluginsd_replay_begin()`.
    fn replay_begin(&mut self, w: &Words) -> Rc {
        let slot = w.slot(CHART_SLOT_MAX);
        let base = if slot.is_some() { 2 } else { 1 };
        let id = w.get(base);
        let start_s = w.get(base + 1);
        let end_s = w.get(base + 2);
        let child_now_s = w.get(base + 3);
        let chart = match id.filter(|i| !i.is_empty()) {
            None => self.require_scope("RBEGIN", "RBEGIN")?,
            Some(id) => match self.chart_from_slot(Some(id), slot, "RBEGIN") {
                Some(chart) => chart,
                None => return refuse(),
            },
        };
        self.set_scope(&chart);
        if let (Some(start_s), Some(end_s)) = (start_s, end_s) {
            let start = str2ull_encoded(start_s) as i64;
            let end = str2ull_encoded(end_s) as i64;
            let update_every = i64::from(chart.update_every());
            let (mut wall, mut tolerance) = (0, 0);
            if let Some(c) = child_now_s {
                wall = str2ull_encoded(c) as i64;
                tolerance = update_every + 1;
            }
            if wall <= 0 {
                wall = self.now_s();
                tolerance = update_every + 5;
            }
            if start != 0
                && end != 0
                && start < wall + tolerance
                && end < wall + tolerance
                && start < end
            {
                if end - start != update_every {
                    chart.set_update_every(end - start);
                }
                let entries = chart.entries();
                chart.update_collection(|c| {
                    c.last_collected = (end, 0);
                    c.last_updated = (end, 0);
                    c.counter += 1;
                    c.counter_done += 1;
                    c.current_entry += 1;
                    if c.current_entry >= entries {
                        c.current_entry -= entries;
                    }
                });
                self.replay = Replay {
                    start_time: start,
                    end_time: end,
                    rset_enabled: true,
                };
                return Ok(());
            }
            let hostname = self.host.hostname();
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "PLUGINSD REPLAY ERROR: 'host:{hostname}/chart:{}' got a RBEGIN from {start} to {end}, but timestamps are invalid (now is {wall} [{}], tolerance {tolerance}). Ignoring RSET",
                chart.id(),
                if child_now_s.is_some() && wall > 0 {
                    "child wall clock"
                } else {
                    "parent wall clock"
                }
            );
        }
        self.replay = Replay::default();
        Ok(())
    }

    /// `pluginsd_replay_set()`.
    fn replay_set(&mut self, w: &Words) -> Rc {
        let slot = w.slot(DIMENSION_SLOT_MAX);
        let base = if slot.is_some() { 2 } else { 1 };
        let dimension = w.get(base);
        let value_s = w.get(base + 1);
        let flags_s = w.get(base + 2);
        let chart = self.require_scope("RSET", "RBEGIN")?;
        if !self.replay.rset_enabled {
            thread_local! {
                // nd_log_limit_static_thread_var(): once a second per thread
                static DISABLED: ErrorLimit = const { ErrorLimit::new(1, 0) };
            }
            let hostname = self.host.hostname();
            let _frame = self.log_frame();
            DISABLED.with(|limit| {
                nd_log_limit!(
                    limit,
                    Source::Collector,
                    Priority::Err,
                    "PLUGINSD REPLAY ERROR: 'host:{hostname}/chart:{}' got a RSET but it is disabled by RBEGIN errors",
                    chart.id()
                );
            });
            return Ok(());
        }
        let Some(dim) = self.acquire_dim(&chart, dimension, slot, "RSET") else {
            return refuse();
        };
        chart.receiver().set = true;
        if self.replay.start_time == 0 || self.replay.end_time == 0 {
            return refuse();
        }
        let value_s = value_s.filter(|v| !v.is_empty()).unwrap_or(b"NAN");
        let mut value = str2ndd_encoded(value_s).0;
        let mut sn_flags = parse_sn_flags(flags_s.unwrap_or(b""));
        if !value.is_finite() || sn_flags == SN_EMPTY_SLOT {
            value = f64::NAN;
            sn_flags = SN_EMPTY_SLOT;
        }
        let end = self.replay.end_time;
        dim.store_metric(end as u64 * 1_000_000, value, sn_flags);
        dim.update_collection(|c| {
            c.last_collected_time = (end, 0);
            c.counter += 1;
        });
        Ok(())
    }

    /// `pluginsd_replay_rrddim_collection_state()`.
    fn replay_rrddim_state(&mut self, w: &Words) -> Rc {
        if !self.replay.rset_enabled {
            return Ok(());
        }
        let slot = w.slot(DIMENSION_SLOT_MAX);
        let base = if slot.is_some() { 2 } else { 1 };
        let dimension = w.get(base);
        let last_collected_ut_s = w.get(base + 1);
        let last_collected_value_s = w.get(base + 2);
        let last_calculated_value_s = w.get(base + 3);
        let last_stored_value_s = w.get(base + 4);
        let chart = self.require_scope("RDSTATE", "RBEGIN")?;
        {
            let mut state = chart.receiver();
            if state.set {
                state.pos = 0;
                state.set = false;
            }
        }
        let Some(dim) = self.acquire_dim(&chart, dimension, slot, "RDSTATE") else {
            return refuse();
        };
        let is_float = dim.meta().flags & dim_flags::FLOAT != 0;
        let sender_sent_float = is_float && self.config.capabilities & caps::FLOAT_BASELINE != 0;
        dim.update_collection(|c| {
            let current =
                c.last_collected_time.0 as u64 * 1_000_000 + c.last_collected_time.1 as u64;
            let got = last_collected_ut_s.map_or(0, str2ull_encoded);
            if got > current {
                c.last_collected_time = ((got / 1_000_000) as i64, (got % 1_000_000) as i64);
            }
            if sender_sent_float {
                c.collected_value_float =
                    last_collected_value_s.map_or(0.0, |v| str2ndd_encoded(v).0);
            } else if is_float {
                c.collected_value_float =
                    last_collected_value_s.map_or(0.0, |v| str2ll_encoded(v) as f64);
            } else {
                c.last_collected_value = last_collected_value_s.map_or(0, str2ll_encoded);
            }
            c.last_calculated_value = last_calculated_value_s.map_or(0.0, |v| str2ndd_encoded(v).0);
            c.last_stored_value = last_stored_value_s.map_or(0.0, |v| str2ndd_encoded(v).0);
        });
        Ok(())
    }

    /// `pluginsd_replay_rrdset_collection_state()`.
    fn replay_rrdset_state(&mut self, w: &Words) -> Rc {
        if !self.replay.rset_enabled {
            return Ok(());
        }
        let last_collected_ut = w.get(1).map_or(0, str2ull_encoded);
        let last_updated_ut = w.get(2).map_or(0, str2ull_encoded);
        let chart = self.require_scope("RSSTATE", "RBEGIN")?;
        chart.update_collection(|c| {
            let as_ut = |t: (i64, i64)| t.0 as u64 * 1_000_000 + t.1 as u64;
            let split = |ut: u64| ((ut / 1_000_000) as i64, (ut % 1_000_000) as i64);
            if last_collected_ut > as_ut(c.last_collected) {
                c.last_collected = split(last_collected_ut);
            }
            if last_updated_ut > as_ut(c.last_updated) {
                c.last_updated = split(last_updated_ut);
            }
            c.counter += 1;
            c.counter_done += 1;
        });
        Ok(())
    }

    /// `pluginsd_replay_end()`.
    fn replay_end(&mut self, w: &Words) -> Rc {
        if w.len() < 7 {
            plog!(
                self,
                Source::Daemon,
                Priority::Err,
                "REPLAY: malformed REND command"
            );
            if let Some(chart) = &self.scope {
                chart.receiver().replication_empty_response_count = 0;
            }
            return refuse();
        }
        let number = |i: usize| w.get(i).map_or(0, |v| str2ull_encoded(v) as i64);
        let update_every_child = number(1);
        let first_entry_child = number(2);
        let last_entry_child = number(3);
        let start_streaming = self.parse_enable_streaming(w.get(4));
        let first_requested = number(5);
        let last_requested = number(6);
        let child_world_time = match w.get(7).filter(|v| !v.is_empty()) {
            Some(v) => str2ull_encoded(v) as i64,
            None => self.now_s(),
        };
        let chart = self.require_scope("REND", "RBEGIN")?;
        self.data_collections_count += 1;
        if self.replay.rset_enabled {
            chart.receiver().replication_empty_response_count = 0;
        }
        // the replication completion of the disconnect record
        if self.replay.rset_enabled && self.host.receiver().is_some() {
            let (started, current) = (self.replication_first_s, self.replay.end_time);
            if started != 0 && current > started {
                let now = self.now_s();
                self.host.set_replication_percent(
                    (current - started) as f64 * 100.0 / (now - started) as f64,
                );
            }
        }
        self.replay = Replay::default();
        chart.update_collection(|c| {
            c.counter += 1;
            c.counter_done += 1;
        });
        {
            let mut state = chart.receiver();
            state.replay_start_streaming = false;
            state.replay_after = 0;
            state.replay_before = 0;
            if start_streaming {
                state.replication_empty_response_count = 0;
            }
        }
        if start_streaming {
            if i64::from(chart.update_every()) != update_every_child {
                chart.set_update_every(update_every_child);
            }
            let was_finished = chart.update_meta(|m| {
                let old = m.flags;
                m.flags |= flags::RECEIVER_REPLICATION_FINISHED;
                m.flags &= !(flags::RECEIVER_REPLICATION_IN_PROGRESS | flags::SYNC_CLOCK);
                old & flags::RECEIVER_REPLICATION_FINISHED != 0
            });
            if was_finished {
                let hostname = self.host.hostname();
                plog!(
                    self,
                    Source::Daemon,
                    Priority::Warning,
                    "PLUGINSD REPLAY ERROR: 'host:{hostname}/chart:{}' got a REND with enable_streaming = true, but there was no replication in progress for this chart.",
                    chart.id()
                );
            }
            self.clear_scope();
            return Ok(());
        }
        let (_, local_last) = retention_for_collected_chart(&chart, self.now_s());
        let caught_up = local_last >= last_entry_child;
        let suspicious = (first_requested != 0 || last_requested != 0) && caught_up;
        let stuck = {
            let mut state = chart.receiver();
            if suspicious {
                state.replication_empty_response_count += 1;
                state.replication_empty_response_count >= 3
            } else {
                state.replication_empty_response_count = 0;
                false
            }
        };
        if stuck {
            let hostname = self.host.hostname();
            // info when the parent's data is also recent (under 5 minutes old), else warning
            let recent = local_last > 0 && self.now_s() - local_last < 300;
            plog!(
                self,
                Source::Daemon,
                if recent {
                    Priority::Info
                } else {
                    Priority::Warning
                },
                "PLUGINSD REPLAY: 'host:{hostname}/chart:{}' detected stuck replication loop. Parent last entry: {local_last}, Child last entry: {last_entry_child}, Gap: 0 seconds, Empty responses: {}. Forcing replication to finish.",
                chart.id(),
                chart.receiver().replication_empty_response_count
            );
            chart.receiver().replication_empty_response_count = 0;
            chart.update_meta(|m| {
                m.flags |= flags::RECEIVER_REPLICATION_FINISHED;
                m.flags &= !(flags::RECEIVER_REPLICATION_IN_PROGRESS | flags::SYNC_CLOCK);
            });
            self.clear_scope();
            self.replicate_chart_request(
                &chart,
                first_entry_child,
                last_entry_child,
                child_world_time,
                0,
                0,
            );
            return Ok(());
        }
        self.clear_scope();
        contexts::updated_retention_rrdset(&chart);
        self.replicate_chart_request(
            &chart,
            first_entry_child,
            last_entry_child,
            child_world_time,
            first_requested,
            last_requested,
        );
        Ok(())
    }
}

/// `rrdset_get_retention_of_tier_for_collected_chart(st, ..., now, 0)`.
fn retention_for_collected_chart(chart: &Chart, now: i64) -> (i64, i64) {
    let (mut first, tier_last) = chart.tier0_retention();
    let mut last = chart.collection().last_updated.0;
    if last == 0 {
        last = tier_last;
        if last == 0 {
            first = 0;
        }
    }
    if last > now {
        last = now;
    }
    if first != 0 && last != 0 && first >= last {
        first = last - i64::from(chart.update_every());
    }
    if first == 0 && last != 0 {
        first = last - i64::from(chart.update_every());
    }
    (first, last)
}

#[cfg(test)]
mod tests;
