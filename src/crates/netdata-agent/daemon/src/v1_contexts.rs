//! `/api/v1/contexts` and `/api/v1/context`, ported from `src/web/api/v1/api_v1_contexts.c`, `api_v1_context.c`,
//! `rrdcontext_to_json_parse_options()` in `src/web/api/web_api.c` and `src/database/contexts/api_v1_contexts.c`.

use netdata_agent_rrd::contexts::{self, Context, Flags, Instance, Metric, REASONS, flags};
use netdata_agent_rrd::host::Host;
use netdata_agent_text::c::strsep_skip;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::parse::str2l;
use netdata_agent_text::print::print_uuid_lower;
use netdata_agent_text::simple_pattern::{Separators, SimplePattern, SimplePatternMode};
use netdata_agent_text::time_window::relative_window_to_absolute_query;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::status;

use crate::server::Reply;

/// `RRDCONTEXT_TO_JSON_OPTIONS`.
mod options {
    pub const SHOW_METRICS: u32 = 1 << 0;
    pub const SHOW_INSTANCES: u32 = 1 << 1;
    pub const SHOW_LABELS: u32 = 1 << 2;
    pub const SHOW_QUEUED: u32 = 1 << 3;
    pub const SHOW_FLAGS: u32 = 1 << 4;
    pub const SHOW_DELETED: u32 = 1 << 5;
    pub const DEEPSCAN: u32 = 1 << 6;
    pub const SHOW_UUIDS: u32 = 1 << 7;
    pub const SHOW_HIDDEN: u32 = 1 << 8;
    pub const RFC3339: u32 = 1 << 9;
    pub const SKIP_ID: u32 = 1 << 31;
    pub const ALL: u32 = SHOW_METRICS
        | SHOW_INSTANCES
        | SHOW_LABELS
        | SHOW_QUEUED
        | SHOW_FLAGS
        | SHOW_DELETED
        | SHOW_UUIDS
        | SHOW_HIDDEN;
}

/// `rrdcontext_to_json_parse_options()`.
fn parse_options(o: &[u8]) -> u32 {
    let mut result = 0;
    let mut rest = Some(o);
    while rest.is_some_and(|r| !r.is_empty()) {
        result |= match strsep_skip(&mut rest, b", |") {
            b"full" | b"all" => options::ALL,
            b"charts" | b"instances" => options::SHOW_INSTANCES,
            b"dimensions" | b"metrics" => options::SHOW_METRICS,
            b"queue" => options::SHOW_QUEUED,
            b"flags" => options::SHOW_FLAGS,
            b"uuids" => options::SHOW_UUIDS,
            b"deleted" => options::SHOW_DELETED,
            b"labels" => options::SHOW_LABELS,
            b"deepscan" => options::DEEPSCAN,
            b"hidden" => options::SHOW_HIDDEN,
            b"rfc3339" => options::RFC3339,
            _ => 0,
        };
    }
    result
}

/// The parameters both endpoints read.
struct Params<'a> {
    context: Option<&'a [u8]>,
    after: i64,
    before: i64,
    options: u32,
    chart_label_key: Option<SimplePattern>,
    chart_labels_filter: Option<SimplePattern>,
    chart_dimensions: Option<SimplePattern>,
}

fn pattern(text: &[u8]) -> SimplePattern {
    SimplePattern::new(
        text,
        Separators::Bytes(b",|\t\r\n\x0c\x0b"),
        SimplePatternMode::Exact,
        true,
    )
}

/// The query loop of `api_v1_contexts()` and `api_v1_context()`: `name=value` pairs, empty ones skipped.
fn parse_params(query: &[u8]) -> Params<'_> {
    let mut p = Params {
        context: None,
        after: 0,
        before: 0,
        options: 0,
        chart_label_key: None,
        chart_labels_filter: None,
        chart_dimensions: None,
    };
    let (mut key, mut filter, mut dimensions) = (None, None, None::<Vec<u8>>);
    let mut url = Some(query);
    while url.is_some() {
        let mut pair = Some(strsep_skip(&mut url, b"&"));
        if pair.is_some_and(<[u8]>::is_empty) {
            continue;
        }
        let name = strsep_skip(&mut pair, b"=");
        let value = pair.unwrap_or(b"");
        if name.is_empty() || value.is_empty() {
            continue;
        }
        match name {
            b"context" | b"ctx" => p.context = Some(value),
            b"after" => p.after = str2l(value),
            b"before" => p.before = str2l(value),
            b"options" => p.options = parse_options(value),
            b"chart_label_key" => key = Some(value),
            b"chart_labels_filter" => filter = Some(value),
            b"dimension" | b"dim" | b"dimensions" | b"dims" => {
                let d = dimensions.get_or_insert_with(Vec::new);
                d.push(b'|');
                d.extend_from_slice(value);
            }
            _ => {}
        }
    }
    p.chart_label_key = key.map(pattern);
    p.chart_labels_filter = filter.map(pattern);
    p.chart_dimensions = dimensions.as_deref().map(pattern);
    p
}

/// `struct rrdcontext_to_json`: the request, plus what the written children combine to.
struct ToJson<'a> {
    p: &'a Params<'a>,
    options: u32,
    after: i64,
    before: i64,
    now: i64,
}

#[derive(Default)]
struct Combined {
    written: usize,
    first_time_s: i64,
    last_time_s: i64,
    flags: u32,
}

impl Combined {
    fn add(&mut self, first: i64, last: i64, flags: u32) {
        if self.written != 0 {
            self.first_time_s = self.first_time_s.min(first);
            self.last_time_s = self.last_time_s.max(last);
            self.flags |= flags;
        } else {
            self.first_time_s = first;
            self.last_time_s = last;
            self.flags = flags;
        }
    }
}

impl ToJson<'_> {
    fn has_filter(&self) -> bool {
        self.p.chart_label_key.is_some()
            || self.p.chart_labels_filter.is_some()
            || self.p.chart_dimensions.is_some()
    }

    fn rfc3339(&self) -> bool {
        self.options & options::RFC3339 != 0
    }

    /// Whether `[first, last]` misses the requested window.
    fn outside(&self, first: i64, last: i64) -> bool {
        (self.after != 0 && (last == 0 || self.after > last))
            || (self.before != 0 && (first == 0 || self.before < first))
    }

    fn uuid(&self, w: &mut JsonWriter, uuid: &[u8; 16]) {
        if self.options & options::SHOW_UUIDS != 0 {
            let mut text = Vec::new();
            print_uuid_lower(&mut text, uuid);
            w.member_add_string(b"uuid", &text);
        }
    }

    /// `first_time_t`, `last_time_t` (now while collected), `collected`, and optionally `deleted` and `flags`.
    fn times_and_state(
        &self,
        w: &mut JsonWriter,
        first: i64,
        last: i64,
        combined: u32,
        own: &Flags,
    ) {
        let collected = combined & flags::COLLECTED != 0;
        w.member_add_time_t_formatted(b"first_time_t", first, self.rfc3339());
        w.member_add_time_t_formatted(
            b"last_time_t",
            if collected { self.now } else { last },
            self.rfc3339(),
        );
        w.member_add_boolean(b"collected", collected);
        if self.options & options::SHOW_DELETED != 0 {
            w.member_add_boolean(b"deleted", own.is_deleted());
        }
        if self.options & options::SHOW_FLAGS != 0 {
            w.member_add_array(Some(b"flags"));
            flags_to_json(w, own.get());
            w.array_close();
        }
    }

    /// `rrdmetric_to_json_callback()`.
    fn metric(&self, w: &mut JsonWriter, total: &mut Combined, rm: &Metric) -> bool {
        let state = rm.state();
        if rm.flags.is_deleted() && self.options & options::SHOW_DELETED == 0 {
            return false;
        }
        if self.outside(state.first_time_s, state.last_time_s) {
            return false;
        }
        if let Some(dims) = &self.p.chart_dimensions
            && !dims.matches(rm.id().as_bytes())
            && state.name != rm.id()
            && !dims.matches(state.name.as_bytes())
        {
            return false;
        }
        total.add(state.first_time_s, state.last_time_s, rm.flags.get());
        w.member_add_object(rm.id());
        self.uuid(w, &state.uuid);
        w.member_add_string(b"name", &state.name);
        let own = rm.flags.get();
        self.times_and_state(w, state.first_time_s, state.last_time_s, own, &rm.flags);
        w.object_close();
        total.written += 1;
        true
    }

    /// `rrdinstance_to_json_callback()`.
    fn instance(
        &self,
        w: &mut JsonWriter,
        total: &mut Combined,
        rc: &Context,
        ri: &Instance,
    ) -> bool {
        if ri.flags.is_deleted() && self.options & options::SHOW_DELETED == 0 {
            return false;
        }
        let state = ri.state();
        if self.outside(state.first_time_s, state.last_time_s) {
            return false;
        }
        let labels = ri.labels();
        if let Some(key) = &self.p.chart_label_key
            && !labels.match_simple_pattern_parsed(key, 0).is_positive()
        {
            return false;
        }
        if let Some(filter) = &self.p.chart_labels_filter
            && !labels
                .match_simple_pattern_parsed(filter, b':')
                .is_positive()
        {
            return false;
        }
        let (mut first, mut last, mut combined) =
            (state.first_time_s, state.last_time_s, ri.flags.get());
        let mut metrics_json = None;
        if self.options & options::SHOW_METRICS != 0 || self.p.chart_dimensions.is_some() {
            let mut sub =
                JsonWriter::with_quotes(b"\"", b"\"", w.depth() + 2, false, JsonOptions::DEFAULT);
            let mut metrics = Combined::default();
            for rm in ri.metrics() {
                self.metric(&mut sub, &mut metrics, &rm);
            }
            if self.has_filter() && metrics.written == 0 {
                return false;
            }
            (first, last, combined) = (metrics.first_time_s, metrics.last_time_s, metrics.flags);
            metrics_json = Some(sub);
        }
        total.add(first, last, combined);
        w.member_add_object(ri.id());
        self.uuid(w, &state.uuid);
        w.member_add_string(b"name", &state.name);
        w.member_add_string(b"context", rc.id());
        w.member_add_string(b"title", &state.title);
        w.member_add_string(b"units", &state.units);
        w.member_add_string(b"family", &state.family);
        w.member_add_string(b"chart_type", state.chart_type.name());
        w.member_add_uint64(b"priority", u64::from(state.priority));
        w.member_add_time_t(b"update_every", i64::from(state.update_every_s));
        self.times_and_state(w, first, last, combined, &ri.flags);
        if self.options & options::SHOW_LABELS != 0 && !labels.is_empty() {
            w.member_add_object(b"labels");
            labels.to_json_members(w);
            w.object_close();
        }
        if let Some(sub) = metrics_json {
            w.member_add_object(b"dimensions");
            w.raw(sub.as_bytes());
            w.object_close();
        }
        w.object_close();
        total.written += 1;
        true
    }

    /// `rrdcontext_to_json_callback()`.
    fn context(&self, w: &mut JsonWriter, total: &mut Combined, rc: &Context) -> bool {
        if rc.flags.check(flags::HIDDEN) && self.options & options::SHOW_HIDDEN == 0 {
            return false;
        }
        if rc.flags.is_deleted() && self.options & options::SHOW_DELETED == 0 {
            return false;
        }
        if self.options & options::DEEPSCAN != 0 {
            contexts::recalculate_context_retention(rc, 0);
        }
        let state = rc.state();
        if self.outside(state.first_time_s, state.last_time_s) {
            return false;
        }
        let (mut first, mut last, mut combined) =
            (state.first_time_s, state.last_time_s, rc.flags.get());
        let mut instances_json = None;
        if self.options & (options::SHOW_LABELS | options::SHOW_INSTANCES | options::SHOW_METRICS)
            != 0
            || self.has_filter()
        {
            let mut sub =
                JsonWriter::with_quotes(b"\"", b"\"", w.depth() + 2, false, JsonOptions::DEFAULT);
            let mut instances = Combined::default();
            for ri in rc.instances() {
                self.instance(&mut sub, &mut instances, rc, &ri);
            }
            if self.has_filter() && instances.written == 0 {
                return false;
            }
            (first, last, combined) = (
                instances.first_time_s,
                instances.last_time_s,
                instances.flags,
            );
            instances_json = Some(sub);
        }
        let skip_id = self.options & options::SKIP_ID != 0;
        if !skip_id {
            w.member_add_object(rc.id());
        }
        w.member_add_string(b"title", &state.title);
        w.member_add_string(b"units", &state.units);
        w.member_add_string(b"family", &state.family);
        w.member_add_string(b"chart_type", state.chart_type.name());
        w.member_add_uint64(b"priority", u64::from(state.priority));
        self.times_and_state(w, first, last, combined, &rc.flags);
        if self.options & options::SHOW_QUEUED != 0 {
            let pp = rc.pp();
            // The hub queue comes with claiming: its fields are what an unclaimed agent reports.
            w.member_add_array(Some(b"queued_reasons"));
            w.array_close();
            for key in [&b"last_queued"[..], b"scheduled_dispatch", b"last_dequeued"] {
                w.member_add_time_t_formatted(key, 0, self.rfc3339());
            }
            w.member_add_uint64(b"dispatches", 0);
            w.member_add_uint64(b"hub_version", state.hub.version);
            w.member_add_uint64(b"version", state.version);
            w.member_add_array(Some(b"pp_reasons"));
            reasons_to_json(w, pp.queued_flags);
            w.array_close();
            w.member_add_time_t_formatted(
                b"pp_last_queued",
                (pp.queued_ut / 1_000_000) as i64,
                self.rfc3339(),
            );
            w.member_add_time_t_formatted(
                b"pp_last_dequeued",
                (pp.dequeued_ut / 1_000_000) as i64,
                self.rfc3339(),
            );
            w.member_add_uint64(b"pp_executed", pp.executions as u64);
        }
        if let Some(sub) = instances_json {
            w.member_add_object(b"charts");
            w.raw(sub.as_bytes());
            w.object_close();
        }
        if !skip_id {
            w.object_close();
        }
        total.written += 1;
        true
    }
}

/// `rrd_flags_to_buffer_json_array_items()`.
fn flags_to_json(w: &mut JsonWriter, f: u32) {
    for (flag, name) in [
        (flags::QUEUED_FOR_HUB, "QUEUED"),
        (flags::DELETED, "DELETED"),
        (flags::COLLECTED, "COLLECTED"),
        (flags::UPDATED, "UPDATED"),
        (flags::ARCHIVED, "ARCHIVED"),
        (flags::OWN_LABELS, "OWN_LABELS"),
        (flags::LIVE_RETENTION, "LIVE_RETENTION"),
        (flags::HIDDEN, "HIDDEN"),
        (flags::QUEUED_FOR_PP, "PENDING_UPDATES"),
    ] {
        if f & flag != 0 {
            w.add_array_item_string(name);
        }
    }
}

/// `rrd_reasons_to_buffer_json_array_items()`.
fn reasons_to_json(w: &mut JsonWriter, f: u32) {
    for (flag, name, _) in REASONS {
        if f & flag != 0 {
            w.add_array_item_string(name);
        }
    }
}

fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Both endpoints convert a window only when both ends are given.
fn window(p: &Params) -> (i64, i64) {
    if p.after != 0 && p.before != 0 {
        let (after, before, _) = relative_window_to_absolute_query(p.after, p.before, now_s());
        (after, before)
    } else {
        (p.after, p.before)
    }
}

fn json_reply(code: u16, w: JsonWriter) -> Reply {
    Reply {
        code,
        content_type: ContentType::ApplicationJson,
        body: w.into_bytes(),
        ..Reply::default()
    }
}

/// `api_v1_contexts()` and `rrdcontexts_to_json()`.
pub fn contexts(host: &Host, query: &[u8]) -> Reply {
    let p = parse_params(query);
    let (after, before) = window(&p);
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_string(b"hostname", host.hostname());
    w.member_add_string(b"machine_guid", host.machine_guid());
    let node_id = host.node_id();
    let mut text = Vec::new();
    if node_id != [0; 16] {
        print_uuid_lower(&mut text, &node_id);
    }
    w.member_add_string(b"node_id", &text);
    text.clear();
    if let Some(claim_id) = host.claim_id() {
        print_uuid_lower(&mut text, &claim_id);
    }
    w.member_add_string(b"claim_id", &text);
    if p.options & options::SHOW_LABELS != 0 {
        w.member_add_object(b"host_labels");
        host.labels().to_json_members(&mut w);
        w.object_close();
    }
    w.member_add_object(b"contexts");
    let t = ToJson {
        p: &p,
        options: p.options,
        after,
        before,
        now: now_s(),
    };
    let mut total = Combined::default();
    for rc in host.contexts().all() {
        t.context(&mut w, &mut total, &rc);
    }
    w.object_close();
    w.finalize();
    json_reply(status::OK, w)
}

/// `api_v1_context()` and `rrdcontext_to_json()`.
pub fn context(host: &Host, query: &[u8]) -> Reply {
    let p = parse_params(query);
    let Some(id) = p.context.filter(|c| !c.is_empty()) else {
        return Reply::text(status::BAD_REQUEST, "No context is given at the request.");
    };
    let Some(rc) = host.contexts().get(&String::from_utf8_lossy(id)) else {
        return Reply {
            code: status::NOT_FOUND,
            content_type: ContentType::ApplicationJson,
            ..Reply::default()
        };
    };
    let (after, before) = window(&p);
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    let t = ToJson {
        p: &p,
        options: p.options | options::SKIP_ID,
        after,
        before,
        now: now_s(),
    };
    let mut total = Combined::default();
    t.context(&mut w, &mut total, &rc);
    w.finalize();
    json_reply(
        if total.written == 0 {
            status::NOT_FOUND
        } else {
            status::OK
        },
        w,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_params() {
        assert_eq!(
            parse_options(b"charts, dimensions|bogus"),
            options::SHOW_INSTANCES | options::SHOW_METRICS
        );
        assert_eq!(parse_options(b"full"), options::ALL);
        let p = parse_params(b"&ctx=a.b&&dims=x&dimension=y&after=-60&empty=&before=0");
        assert_eq!(p.context, Some(&b"a.b"[..]));
        assert_eq!((p.after, p.before), (-60, 0));
        let dims = p.chart_dimensions.unwrap();
        assert!(dims.matches(b"x") && dims.matches(b"y") && !dims.matches(b"z"));
    }
}
