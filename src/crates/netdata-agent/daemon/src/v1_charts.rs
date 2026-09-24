//! `/api/v1/charts` and `/api/v1/chart`, ported from `src/web/api/v1/api_v1_charts.c`,
//! `src/web/api/formatters/charts2json.c` and `rrdset2json.c`. Health is not ported: chart variables and alarms are
//! empty, `alarms_count` is 0 and `green`/`red` are unset (null).

use std::sync::Arc;

use netdata_agent_rrd::chart::{Chart, ID_LENGTH_MAX, dim_flags, flags as chart_flags};
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_rrd::mode::DbMode;
use netdata_agent_text::c::strsep_skip;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::print::html_escape;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::status;

use crate::server::Reply;

/// `get_release_channel()`: `RELEASE_CHANNEL` of `<user config>/.environment` (`stable` or `nightly`), else nightly
/// for a version with a `-`.
pub fn release_channel(user_config_dir: &str, version: &str) -> &'static str {
    let from_file = std::fs::read_to_string(format!("{user_config_dir}/.environment"))
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "RELEASE_CHANNEL").then(|| {
                    value
                        .trim()
                        .trim_matches(|c| c == '\'' || c == '"')
                        .to_string()
                })
            })
        });
    match from_file.as_deref() {
        Some("stable") => "stable",
        Some("nightly") => "nightly",
        _ if version.contains('-') => "nightly",
        _ => "stable",
    }
}

/// What `/api/v1/charts` reports besides the charts: the release channel and `[web] custom dashboard_info.js`.
#[derive(Debug, Clone, Default)]
pub struct ChartsInfo {
    pub release_channel: &'static str,
    pub custom_info: String,
}

/// `rrdset_is_available_for_viewers()`.
fn available_for_viewers(st: &Chart) -> bool {
    let flags = st.meta().flags;
    flags & (chart_flags::HIDDEN | chart_flags::OBSOLETE) == 0
        && st.dim_count() > 0
        && st.mode() != DbMode::None
}

/// `rrdset2json()`: adds the chart's members; returns its visible dimensions.
fn chart_json(w: &mut JsonWriter, st: &Chart) -> usize {
    let meta = st.meta();
    let name = meta.name.clone().unwrap_or_else(|| st.id().to_string());
    let (first, last) = st.tier0_retention();
    // snprintfz(buf, RRD_ID_LENGTH_MAX + 15, ...).
    let cut = |mut s: String| {
        if s.len() > ID_LENGTH_MAX + 15 {
            let mut end = ID_LENGTH_MAX + 15;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
        }
        s
    };
    w.member_add_string("id", st.id());
    w.member_add_string("name", &name);
    w.member_add_string("type", st.type_());
    w.member_add_string("family", &meta.family);
    w.member_add_string("context", &meta.context);
    w.member_add_string("title", cut(format!("{} ({name})", meta.title)));
    w.member_add_int64("priority", meta.priority);
    w.member_add_string("plugin", &meta.plugin);
    w.member_add_string("module", &meta.module);
    w.member_add_string("units", &meta.units);
    w.member_add_string("data_url", cut(format!("/api/v1/data?chart={name}")));
    w.member_add_string("chart_type", meta.chart_type.name());
    let update_every = i64::from(meta.update_every);
    w.member_add_int64("duration", last - first + update_every);
    w.member_add_int64("first_entry", first);
    w.member_add_int64("last_entry", last);
    w.member_add_int64("update_every", update_every);
    let mut dimensions = 0;
    w.member_add_object(b"dimensions");
    for rd in st.dims() {
        let dm = rd.meta();
        if dm.flags & (dim_flags::HIDDEN | dim_flags::OBSOLETE) != 0 {
            continue;
        }
        w.member_add_object(rd.id());
        w.member_add_string("name", &dm.name);
        w.object_close();
        dimensions += 1;
    }
    w.object_close();
    w.member_add_object(b"chart_variables");
    w.object_close();
    w.member_add_double("green", f64::NAN);
    w.member_add_double("red", f64::NAN);
    w.member_add_object(b"alarms");
    w.object_close();
    w.member_add_object(b"chart_labels");
    for label in meta.labels.iter() {
        w.member_add_string_or_empty(&label.name, Some(&label.value));
    }
    w.object_close();
    // Chart-scoped functions no longer exist; the key stays, always empty.
    w.member_add_object(b"functions");
    w.object_close();
    dimensions
}

/// `charts2json()`. `rrd_memory_bytes` counts this implementation's chart and ring memory, not C's structures.
pub fn charts(host: &Host, hosts: &Hosts, info: &ChartsInfo) -> Reply {
    let hi = host.info();
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_string("hostname", host.hostname());
    w.member_add_string("version", &hi.program_version);
    w.member_add_string("release_channel", info.release_channel);
    w.member_add_string("os", &hi.os);
    w.member_add_string("timezone", &hi.timezone);
    w.member_add_int64("update_every", i64::from(hi.update_every));
    w.member_add_int64("history", hi.history_entries);
    w.member_add_string("memory_mode", hi.db_mode.name());
    w.member_add_string("custom_info", &info.custom_info);
    let (mut count, mut dimensions, mut memory) = (0i64, 0i64, 0i64);
    w.member_add_object(b"charts");
    for st in host.charts().all() {
        if !available_for_viewers(&st) {
            continue;
        }
        w.member_add_object(st.id());
        dimensions += chart_json(&mut w, &st) as i64;
        w.object_close();
        memory += st
            .dims()
            .iter()
            .filter_map(|rd| rd.ring().map(|r| (r.entries() * 4) as i64))
            .sum::<i64>();
        count += 1;
    }
    w.object_close();
    w.member_add_int64("charts_count", count);
    w.member_add_int64("dimensions_count", dimensions);
    w.member_add_int64("alarms_count", 0);
    w.member_add_int64("rrd_memory_bytes", memory);
    let all = hosts.all();
    w.member_add_int64("hosts_count", all.len() as i64);
    w.member_add_array(Some(b"hosts"));
    // Orphan hosts are not archived yet: every host is listed.
    for h in &all {
        w.add_array_item_object();
        w.member_add_string("hostname", h.hostname());
        w.object_close();
    }
    w.array_close();
    w.finalize();
    Reply {
        code: status::OK,
        content_type: ContentType::ApplicationJson,
        body: w.into_bytes(),
        ..Reply::default()
    }
}

/// `api_v1_single_chart_helper()` with `rrd_stats_api_v1_chart()`.
pub fn chart(host: &Arc<Host>, query: &[u8]) -> Reply {
    let mut chart = None;
    let mut rest = Some(query);
    while rest.is_some() {
        let mut value = Some(strsep_skip(&mut rest, b"&"));
        let name = strsep_skip(&mut value, b"=");
        let value = value.unwrap_or(b"");
        if name.is_empty() || value.is_empty() {
            continue;
        }
        if name == b"chart" {
            chart = Some(value);
        }
    }
    let Some(chart) = chart else {
        return Reply::text(status::BAD_REQUEST, "No chart id is given at the request.");
    };
    let Some(st) = crate::data::find_chart(host, chart) else {
        // The body is HTML-escaped but stays text/plain.
        let mut reply = Reply::text(status::NOT_FOUND, "Chart is not found: ");
        html_escape(&mut reply.body, chart);
        return reply;
    };
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    chart_json(&mut w, &st);
    w.finalize();
    Reply {
        code: status::OK,
        content_type: ContentType::ApplicationJson,
        body: w.into_bytes(),
        ..Reply::default()
    }
}
