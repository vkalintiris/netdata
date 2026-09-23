//! The protocol handlers that apply plugins.d/streaming keywords to hosts, charts and storage, ported from
//! `src/plugins.d/pluginsd_parser.c`, `pluginsd_replication.c`, `pluginsd_internals.h` and
//! `src/streaming/stream-replication-receiver.c` (`replicate_chart_request()`). Spec: `knowledge/spec-ingest.md` §2 and
//! §3.6 in the status repository.
//!
//! One `Parser` per connection, driven by the stream thread that owns the connection (decisions D8). Every handler
//! error disconnects, as on the C streaming parser.

#![forbid(unsafe_code)]

use std::sync::Arc;

use netdata_agent_inicfg::LogLevel;
use netdata_agent_pluginsd_proto::{
    CHART_SLOT_MAX, DIMENSION_SLOT_MAX, Deferred, DeferredBody, Keyword, Repertoire, Words,
};
use netdata_agent_rrd::chart::{Algorithm, Chart, ChartSpec, ChartType, Dim, dim_flags, flags};
use netdata_agent_rrd::host::Host;
use netdata_agent_storage::storage_number::{SN_EMPTY_SLOT, SN_FLAG_NOT_ANOMALOUS, SN_FLAG_RESET};
use netdata_agent_text::parse::{str2i, str2ll_encoded, str2ndd_encoded, str2ul, str2ull_encoded};

/// `STREAM_CAP_FLOAT_BASELINE` and `STREAM_CAP_ML_MODELS`: the capabilities the handlers consult.
pub const CAP_FLOAT_BASELINE: u32 = 1 << 27;
pub const CAP_ML_MODELS: u32 = 1 << 26;

/// Where the parser writes daemon log lines.
pub type Logger = Box<dyn FnMut(LogLevel, &str) + Send>;

/// The fixed inputs of a parser.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// The negotiated stream capabilities.
    pub capabilities: u32,
    /// `parser->user.cd->update_every`: `[db] update every`.
    pub update_every: i32,
    /// `sysconf(_SC_PAGESIZE)`, for ring sizes.
    pub page_size: i64,
    /// The wall clock in seconds (`now_realtime_sec()`), replaceable in tests.
    pub now: fn() -> i64,
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

/// `stream_parse_enable_streaming()`.
fn parse_enable_streaming(v: Option<&[u8]>, log: &mut Logger) -> bool {
    match v {
        None | Some(b"") => {
            log(
                LogLevel::Error,
                "REPLAY: malformed start_streaming boolean value empty",
            );
            false
        }
        Some(b"false") => false,
        Some(b"true") => true,
        Some(other) => {
            log(
                LogLevel::Error,
                &format!(
                    "REPLAY: malformed start_streaming boolean value '{}'",
                    text(other)
                ),
            );
            false
        }
    }
}

/// The receiver side of one connection.
pub struct Parser {
    host: Arc<Host>,
    config: Config,
    log: Logger,
    line: usize,
    scope: Option<Arc<Chart>>,
    clabel_count: usize,
    clabel_changed: bool,
    v2: V2,
    replay: Replay,
    /// `host->stream.rcv.pluginsd_chart_slots`.
    chart_slots: Vec<Option<Arc<Chart>>>,
    /// `parser->user.data_collections_count`.
    pub data_collections_count: u64,
    deferred: Option<DeferredBody>,
    /// Bytes for the child (`send_to_plugin`), drained by the caller.
    out: Vec<u8>,
}

impl Parser {
    pub fn new(host: Arc<Host>, config: Config, log: Logger) -> Self {
        Parser {
            host,
            config,
            log,
            line: 0,
            scope: None,
            clabel_count: 0,
            clabel_changed: false,
            v2: V2::default(),
            replay: Replay::default(),
            chart_slots: Vec::new(),
            data_collections_count: 0,
            deferred: None,
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
                // STREAM_PATH and ML_MODEL payloads come with paths and ML.
                Deferred::Done(_) => {
                    self.deferred = None;
                    true
                }
                Deferred::TooBig => false,
            };
        }
        let words = Words::split(line);
        let Some(first) = words.get(0) else {
            return true;
        };
        let result = match Keyword::lookup(first) {
            Some(keyword) if keyword.repertoire().contains(Repertoire::STREAMING) => {
                self.dispatch(keyword, &words)
            }
            _ => Err(Refused(None)),
        };
        match result {
            Ok(()) => true,
            Err(Refused(why)) => {
                if let Some(why) = why {
                    (self.log)(LogLevel::Error, &why);
                }
                let line_no = self.line;
                let shown = text(&words.reconstruct());
                (self.log)(
                    LogLevel::Error,
                    &format!(
                        "PLUGINSD: parser_action('{}') failed on line {line_no}: {{ {shown} }} (quotes added to show parsing)",
                        text(first)
                    ),
                );
                false
            }
        }
    }

    fn dispatch(&mut self, keyword: Keyword, w: &Words) -> Rc {
        match keyword {
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
            Keyword::Json => {
                self.deferred = Some(DeferredBody::new("JSON_PAYLOAD_END"));
                Ok(())
            }
            Keyword::FunctionResultBegin => {
                self.deferred = Some(DeferredBody::new("FUNCTION_RESULT_END"));
                Ok(())
            }
            // The v1 data path, variables, host labels, claiming, Functions and dynamic configuration are ported
            // next (agent/plan.md); until then they change nothing.
            _ => Ok(()),
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
                (self.log)(
                    LogLevel::Error,
                    &format!(
                        "PLUGINSD: command {keyword} requires a chart defined via command {parent}, but is not set."
                    ),
                );
                Err(Refused(None))
            }
        }
    }

    /// `pluginsd_find_chart()`.
    fn find_chart(&mut self, id: Option<&[u8]>, keyword: &str) -> Option<Arc<Chart>> {
        let hostname = self.host.hostname();
        let Some(id) = id.filter(|i| !i.is_empty()) else {
            (self.log)(
                LogLevel::Error,
                &format!("PLUGINSD: 'host:{hostname}' got a {keyword} without a chart id."),
            );
            return None;
        };
        let chart = self.host.charts().find(&text(id));
        if chart.is_none() {
            (self.log)(
                LogLevel::Error,
                &format!(
                    "PLUGINSD: 'host:{hostname}/chart:{}' got a {keyword} but chart does not exist.",
                    text(id)
                ),
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
            (self.log)(
                LogLevel::Error,
                &format!(
                    "PLUGINSD: 'host:{hostname}/chart:{}' got a {keyword}, without a dimension.",
                    chart.id()
                ),
            );
            return None;
        };
        let id = text(id);
        let mut state = chart.receiver();
        if state.prd.is_empty() {
            drop(state);
            (self.log)(
                LogLevel::Error,
                &format!(
                    "PLUGINSD: 'host:{hostname}/chart:{}' got a {keyword}, but the chart has no dimensions.",
                    chart.id()
                ),
            );
            return None;
        }
        let size = state.prd.len();
        let position = if state.dims_with_slots {
            let s = slot.unwrap_or(0);
            if s < 1 || s as usize > size {
                drop(state);
                (self.log)(
                    LogLevel::Error,
                    &format!(
                        "PLUGINSD: 'host:{hostname}/chart:{}' got a {keyword} with slot {}, but slots in the range [1 - {size}] are expected.",
                        chart.id(),
                        slot.map_or(-1, |s| s as i64)
                    ),
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
            (self.log)(
                LogLevel::Error,
                &format!(
                    "PLUGINSD: 'host:{hostname}/chart:{}/dim:{id}' got a {keyword} but dimension does not exist.",
                    chart.id()
                ),
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
        chart.update_meta(|m| match options.filter(|o| !o.is_empty()) {
            Some(o) => {
                let has = |what: &[u8]| o.windows(what.len()).any(|x| x == what);
                for (word, flag) in [
                    (&b"obsolete"[..], flags::OBSOLETE),
                    (b"hidden", flags::HIDDEN),
                    (b"store_first", flags::STORE_FIRST),
                ] {
                    if has(word) {
                        m.flags |= flag;
                    } else {
                        m.flags &= !flag;
                    }
                }
            }
            None => m.flags &= !flags::STORE_FIRST,
        });
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
        dim.update_meta(|m| {
            m.flags &= !dim_flags::DONT_DETECT_RESETS_OR_OVERFLOWS;
            let mut hidden = false;
            match options.filter(|o| !o.is_empty()) {
                Some(o) => {
                    let has = |what: &[u8]| o.windows(what.len()).any(|x| x == what);
                    if has(b"obsolete") {
                        m.flags |= dim_flags::OBSOLETE;
                    } else {
                        m.flags &= !dim_flags::OBSOLETE;
                    }
                    hidden = has(b"hidden");
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
                }
                None => m.flags &= !dim_flags::OBSOLETE,
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
            (self.log)(
                LogLevel::Error,
                "Ignoring malformed or empty CHART LABEL command.",
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
            Err(message) => (self.log)(LogLevel::Error, &message),
        }
        Ok(())
    }

    /// `pluginsd_clabel_commit()`.
    fn clabel_commit(&mut self) -> Rc {
        let chart = self.require_scope("CLABEL_COMMIT", "BEGIN")?;
        if self.clabel_count == 0 {
            let hostname = self.host.hostname();
            (self.log)(
                LogLevel::Error,
                &format!(
                    "PLUGINSD: 'host:{hostname}' got CLABEL_COMMIT, without a CHART or BEGIN. Ignoring it."
                ),
            );
            return refuse();
        }
        let changed = chart.update_meta(|m| m.labels.remove_all_unmarked_and_changed())
            || self.clabel_changed;
        if changed {
            chart.update_meta(|m| m.flags |= flags::METADATA_UPDATE);
        }
        self.clabel_count = 0;
        self.clabel_changed = false;
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
        chart.update_meta(|m| m.flags &= !flags::OBSOLETE);
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
        let is_float = dim.update_meta(|m| {
            m.flags &= !dim_flags::OBSOLETE;
            m.flags & dim_flags::FLOAT != 0
        });
        let sender_sent_float = is_float && self.config.capabilities & CAP_FLOAT_BASELINE != 0;
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
        if let Some(ring) = dim.ring() {
            ring.store(end_time as u64 * 1_000_000, value, sn_flags);
        }
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
            None => (self.config.now)(),
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
        let now = (self.config.now)();
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
        self.out.extend_from_slice(
            format!(
                "REPLAY_CHART \"{}\" \"{}\" {after} {before}\n",
                chart.id(),
                if start_streaming { "true" } else { "false" }
            )
            .as_bytes(),
        );
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
                wall = (self.config.now)();
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
            (self.log)(
                LogLevel::Error,
                &format!(
                    "PLUGINSD REPLAY ERROR: 'host:{hostname}/chart:{}' got a RBEGIN from {start} to {end}, but timestamps are invalid (now is {wall} [{}], tolerance {tolerance}). Ignoring RSET",
                    chart.id(),
                    if child_now_s.is_some() && wall > 0 {
                        "child wall clock"
                    } else {
                        "parent wall clock"
                    }
                ),
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
            let hostname = self.host.hostname();
            (self.log)(
                LogLevel::Error,
                &format!(
                    "PLUGINSD REPLAY ERROR: 'host:{hostname}/chart:{}' got a RSET but it is disabled by RBEGIN errors",
                    chart.id()
                ),
            );
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
        if let Some(ring) = dim.ring() {
            ring.store(end as u64 * 1_000_000, value, sn_flags);
        }
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
        let sender_sent_float = is_float && self.config.capabilities & CAP_FLOAT_BASELINE != 0;
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
            (self.log)(LogLevel::Error, "REPLAY: malformed REND command");
            if let Some(chart) = &self.scope {
                chart.receiver().replication_empty_response_count = 0;
            }
            return refuse();
        }
        let number = |i: usize| w.get(i).map_or(0, |v| str2ull_encoded(v) as i64);
        let update_every_child = number(1);
        let first_entry_child = number(2);
        let last_entry_child = number(3);
        let start_streaming = parse_enable_streaming(w.get(4), &mut self.log);
        let first_requested = number(5);
        let last_requested = number(6);
        let child_world_time = match w.get(7).filter(|v| !v.is_empty()) {
            Some(v) => str2ull_encoded(v) as i64,
            None => (self.config.now)(),
        };
        let chart = self.require_scope("REND", "RBEGIN")?;
        self.data_collections_count += 1;
        if self.replay.rset_enabled {
            chart.receiver().replication_empty_response_count = 0;
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
                (self.log)(
                    LogLevel::Info,
                    &format!(
                        "PLUGINSD REPLAY ERROR: 'host:{hostname}/chart:{}' got a REND with enable_streaming = true, but there was no replication in progress for this chart.",
                        chart.id()
                    ),
                );
            }
            self.clear_scope();
            return Ok(());
        }
        let (_, local_last) = retention_for_collected_chart(&chart, (self.config.now)());
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
            (self.log)(
                LogLevel::Info,
                &format!(
                    "PLUGINSD REPLAY: 'host:{hostname}/chart:{}' detected stuck replication loop. Parent last entry: {local_last}, Child last entry: {last_entry_child}, Gap: 0 seconds, Empty responses: {}. Forcing replication to finish.",
                    chart.id(),
                    chart.receiver().replication_empty_response_count
                ),
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
