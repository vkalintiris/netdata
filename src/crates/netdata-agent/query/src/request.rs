//! The data handlers' parameter parsing, ported from `api_v1_data()` (`src/web/api/v1/api_v1_data.c`) and
//! `api_v23_data_internal()` (`src/web/api/v2/api_v2_data.c`). Spec §2.2-2.5.

use netdata_agent_text::c::strsep_skip;
use netdata_agent_text::parse::{str2i, str2l, str2u, str2ul, strtoul0};

use crate::tables::{
    Aggregation, Format, TimeGrouping, group_by, options, parse_group_by, parse_options,
};

/// One pass of `group_by` (`struct group_by_pass`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupByPass {
    pub group_by: u32,
    pub label: Option<Vec<u8>>,
    pub aggregation: Aggregation,
}

impl Default for GroupByPass {
    fn default() -> Self {
        GroupByPass {
            group_by: group_by::NONE,
            label: None,
            aggregation: Aggregation::Average,
        }
    }
}

/// The Google visualization parameters (`tqx`, `callback`, `filename`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Google {
    pub version: Vec<u8>,
    pub req_id: Vec<u8>,
    pub sig: Vec<u8>,
    pub out: Vec<u8>,
    pub response_handler: Option<Vec<u8>>,
    pub out_file_name: Option<Vec<u8>>,
    /// `strtoul(sig, NULL, 0)` as `time_t`.
    pub timestamp: i64,
}

impl Default for Google {
    fn default() -> Self {
        Google {
            version: b"0.6".to_vec(),
            req_id: b"0".to_vec(),
            sig: b"0".to_vec(),
            out: b"json".to_vec(),
            response_handler: None,
            out_file_name: None,
            timestamp: 0,
        }
    }
}

/// `fix_google_param()`: every byte other than ASCII alphanumerics, `.`, `_` and `-` becomes `_`.
fn fix_google_param(v: &mut [u8]) {
    for b in v {
        if !(b.is_ascii_alphanumeric() || matches!(*b, b'.' | b'_' | b'-')) {
            *b = b'_';
        }
    }
}

/// What both data handlers pass to the query target (`QUERY_TARGET_REQUEST`) and the formatters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataRequest {
    /// 1, 2 or 3.
    pub version: u8,
    pub scope_nodes: Option<Vec<u8>>,
    pub scope_contexts: Option<Vec<u8>>,
    pub scope_instances: Option<Vec<u8>>,
    pub scope_labels: Option<Vec<u8>>,
    pub scope_dimensions: Option<Vec<u8>>,
    pub nodes: Option<Vec<u8>>,
    pub contexts: Option<Vec<u8>>,
    pub instances: Option<Vec<u8>>,
    pub dimensions: Option<Vec<u8>>,
    pub chart_label_key: Option<Vec<u8>>,
    pub labels: Option<Vec<u8>>,
    pub alerts: Option<Vec<u8>>,
    pub after: i64,
    pub before: i64,
    /// `size_t points`: v1 parses an int, so a negative value sign-extends.
    pub points: u64,
    pub timeout_ms: i32,
    pub tier: u64,
    pub format: Format,
    pub options: u64,
    pub time_group: TimeGrouping,
    pub time_group_options: Option<Vec<u8>>,
    pub resampling_time: i64,
    pub group_by: [GroupByPass; 2],
    pub cardinality_limit: u64,
    pub google: Google,
}

impl DataRequest {
    fn new(version: u8) -> Self {
        DataRequest {
            version,
            scope_nodes: None,
            scope_contexts: None,
            scope_instances: None,
            scope_labels: None,
            scope_dimensions: None,
            nodes: None,
            contexts: None,
            instances: None,
            dimensions: None,
            chart_label_key: None,
            labels: None,
            alerts: None,
            after: 0,
            before: 0,
            points: 0,
            timeout_ms: 0,
            tier: 0,
            format: Format::Json,
            options: 0,
            time_group: TimeGrouping::Average,
            time_group_options: None,
            resampling_time: 0,
            group_by: [GroupByPass::default(), GroupByPass::default()],
            cardinality_limit: 0,
            google: Google::default(),
        }
    }

    /// `fix_google_param()` on the six Google parameters, after the loop.
    fn fix_google_params(&mut self) {
        let g = &mut self.google;
        for v in [&mut g.out, &mut g.sig, &mut g.req_id, &mut g.version] {
            fix_google_param(v);
        }
        for v in [&mut g.response_handler, &mut g.out_file_name]
            .into_iter()
            .flatten()
        {
            fix_google_param(v);
        }
    }
}

/// `name=value` pairs of a query string, with C's skipping: empty items, empty names and empty values are dropped.
pub fn pairs(query: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut url = Some(query);
    std::iter::from_fn(move || {
        while url.is_some() {
            let mut value = Some(strsep_skip(&mut url, b"&"));
            if value.is_some_and(<[u8]>::is_empty) {
                continue;
            }
            let name = strsep_skip(&mut value, b"=");
            let value = value.unwrap_or(b"");
            if name.is_empty() || value.is_empty() {
                continue;
            }
            return Some((name, value));
        }
        None
    })
}

/// The `tqx` parameter: `;`-separated `name:value` pairs.
fn parse_tqx(g: &mut Google, format: &mut Format, tqx: &[u8]) {
    let mut rest = Some(tqx);
    while rest.is_some() {
        let mut item = Some(strsep_skip(&mut rest, b";"));
        if item.is_some_and(<[u8]>::is_empty) {
            continue;
        }
        let name = strsep_skip(&mut item, b":");
        let value = item.unwrap_or(b"");
        if name.is_empty() || value.is_empty() {
            continue;
        }
        match name {
            b"version" => g.version = value.to_vec(),
            b"reqId" => g.req_id = value.to_vec(),
            b"sig" => {
                g.sig = value.to_vec();
                g.timestamp = strtoul0(value).0 as i64;
            }
            b"out" => {
                g.out = value.to_vec();
                *format = Format::from_google(value);
            }
            b"responseHandler" => g.response_handler = Some(value.to_vec()),
            b"outFileName" => g.out_file_name = Some(value.to_vec()),
            _ => {}
        }
    }
}

/// `tier=`: a tier below the configured count selects it (the bit is never cleared), else tier 0.
fn apply_tier(req: &mut DataRequest, value: &[u8], storage_tiers: u64) {
    let tier = str2ul(value);
    if tier < storage_tiers {
        req.tier = tier;
        req.options |= options::SELECTED_TIER;
    } else {
        req.tier = 0;
    }
}

/// `cardinality_limit=` (even 0), else a non-zero `limit=` plus one.
fn cardinality(cardinality_limit: Option<&[u8]>, limit: Option<&[u8]>) -> u64 {
    match (cardinality_limit, limit) {
        (Some(c), _) => str2ul(c),
        (None, Some(l)) if str2ul(l) != 0 => str2ul(l) + 1,
        _ => 0,
    }
}

/// The v1 parameters (`api_v1_data()`), before the chart lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V1Params {
    pub request: DataRequest,
    /// `chart=`: looked up by id then name before the query target is built.
    pub chart: Option<Vec<u8>>,
}

/// `is_valid_sp()`: set, non-empty and not exactly `*`.
pub fn is_valid_sp(v: Option<&[u8]>) -> bool {
    v.is_some_and(|v| !v.is_empty() && v != b"*")
}

/// The loop and the numeric conversions of `api_v1_data()` (steps 1, 4 and 5 of spec §2.3).
pub fn parse_v1(query: &[u8], storage_tiers: u64) -> V1Params {
    let mut req = DataRequest::new(1);
    req.after = -600;
    let mut chart = None;
    let mut dims: Option<Vec<u8>> = None;
    let (mut after, mut before, mut points, mut timeout, mut gtime) =
        (None, None, None, None, None);
    let (mut group_options, mut cardinality_limit, mut limit) = (None, None, None);
    for (name, value) in pairs(query) {
        match name {
            b"context" => req.contexts = Some(value.to_vec()),
            b"chart_label_key" => req.chart_label_key = Some(value.to_vec()),
            b"chart_labels_filter" => req.labels = Some(value.to_vec()),
            b"chart" => chart = Some(value.to_vec()),
            b"dimension" | b"dim" | b"dimensions" | b"dims" => {
                let d = dims.get_or_insert_with(Vec::new);
                d.push(b'|');
                d.extend_from_slice(value);
            }
            b"show_dimensions" => req.options |= options::ALL_DIMENSIONS,
            b"after" => after = Some(value),
            b"before" => before = Some(value),
            b"points" => points = Some(value),
            b"timeout" => timeout = Some(value),
            b"gtime" => gtime = Some(value),
            b"group_options" => group_options = Some(value),
            b"cardinality_limit" => cardinality_limit = Some(value),
            b"limit" => limit = Some(value),
            b"group" => req.time_group = TimeGrouping::parse(value),
            b"format" => req.format = Format::parse(value),
            b"options" => req.options |= parse_options(value),
            b"callback" => req.google.response_handler = Some(value.to_vec()),
            b"filename" => req.google.out_file_name = Some(value.to_vec()),
            b"tqx" => parse_tqx(&mut req.google, &mut req.format, value),
            b"tier" => apply_tier(&mut req, value, storage_tiers),
            _ => {}
        }
    }
    req.fix_google_params();
    if let Some(v) = before {
        req.before = str2l(v);
    }
    if let Some(v) = after {
        req.after = str2l(v);
    }
    if let Some(v) = points {
        req.points = i64::from(str2i(v)) as u64;
    }
    if let Some(v) = timeout {
        req.timeout_ms = str2i(v);
    }
    if let Some(v) = gtime {
        req.resampling_time = str2l(v);
    }
    req.time_group_options = group_options.map(<[u8]>::to_vec);
    req.cardinality_limit = cardinality(cardinality_limit, limit);
    req.instances = chart.clone();
    req.dimensions = dims;
    V1Params {
        request: req,
        chart,
    }
}

/// The loop and the post-processing of `api_v23_data_internal()` (steps 1-7 of spec §2.4).
pub fn parse_v2(query: &[u8], version: u8, storage_tiers: u64) -> DataRequest {
    let mut req = DataRequest::new(version);
    req.after = -600;
    req.format = Format::Json2;
    req.options = options::VIRTUAL_POINTS | options::JSON_WRAP | options::RETURN_JWAR;
    let (mut gb_idx, mut gbl_idx, mut agg_idx) = (0usize, 0usize, 0usize);
    let (mut after, mut before, mut points, mut timeout, mut resampling) =
        (None, None, None, None, None);
    let (mut tier, mut cardinality_limit, mut limit) = (None, None, None);
    let next = |idx: &mut usize| {
        let i = *idx;
        *idx = (*idx + 1).min(1);
        i
    };
    for (name, value) in pairs(query) {
        match name {
            b"scope_nodes" => req.scope_nodes = Some(value.to_vec()),
            b"scope_contexts" => req.scope_contexts = Some(value.to_vec()),
            b"scope_instances" => req.scope_instances = Some(value.to_vec()),
            b"scope_labels" => req.scope_labels = Some(value.to_vec()),
            b"scope_dimensions" => req.scope_dimensions = Some(value.to_vec()),
            b"nodes" => req.nodes = Some(value.to_vec()),
            b"contexts" => req.contexts = Some(value.to_vec()),
            b"instances" => req.instances = Some(value.to_vec()),
            b"dimensions" => req.dimensions = Some(value.to_vec()),
            b"labels" => req.labels = Some(value.to_vec()),
            b"alerts" => req.alerts = Some(value.to_vec()),
            b"after" => after = Some(value),
            b"before" => before = Some(value),
            b"points" => points = Some(value),
            b"timeout" => timeout = Some(value),
            b"time_resampling" => resampling = Some(value),
            b"tier" => tier = Some(value),
            b"cardinality_limit" => cardinality_limit = Some(value),
            b"limit" => limit = Some(value),
            b"group_by" => req.group_by[next(&mut gb_idx)].group_by = parse_group_by(value),
            b"group_by_label" => req.group_by[next(&mut gbl_idx)].label = Some(value.to_vec()),
            b"aggregation" => {
                req.group_by[next(&mut agg_idx)].aggregation = Aggregation::parse(value)
            }
            b"group_by[0]" => req.group_by[0].group_by = parse_group_by(value),
            b"group_by[1]" => req.group_by[1].group_by = parse_group_by(value),
            b"group_by_label[0]" => req.group_by[0].label = Some(value.to_vec()),
            b"group_by_label[1]" => req.group_by[1].label = Some(value.to_vec()),
            b"aggregation[0]" => req.group_by[0].aggregation = Aggregation::parse(value),
            b"aggregation[1]" => req.group_by[1].aggregation = Aggregation::parse(value),
            b"format" => req.format = Format::parse(value),
            b"options" => req.options |= parse_options(value),
            b"time_group" => req.time_group = TimeGrouping::parse(value),
            b"time_group_options" => req.time_group_options = Some(value.to_vec()),
            b"callback" => req.google.response_handler = Some(value.to_vec()),
            b"filename" => req.google.out_file_name = Some(value.to_vec()),
            b"tqx" => parse_tqx(&mut req.google, &mut req.format, value),
            _ => {}
        }
    }
    req.fix_google_params();
    for pass in &mut req.group_by {
        if pass.label.as_ref().is_some_and(|l| !l.is_empty()) {
            pass.group_by |= group_by::LABEL;
        }
    }
    if req.group_by[0].group_by == group_by::NONE {
        req.group_by[0].group_by = group_by::DIMENSION;
    }
    for pass in &req.group_by {
        let g = pass.group_by;
        if (g != group_by::NONE && g & group_by::DIMENSION == 0)
            || req.options & options::PERCENTAGE != 0
        {
            req.options |= options::ABSOLUTE;
            break;
        }
    }
    if req.options & options::DEBUG != 0 {
        req.options &= !options::MINIFY;
    }
    if let Some(v) = tier {
        apply_tier(&mut req, v, storage_tiers);
    }
    req.cardinality_limit = cardinality(cardinality_limit, limit);
    if let Some(v) = before {
        req.before = str2l(v);
    }
    if let Some(v) = after {
        req.after = str2l(v);
    }
    if let Some(v) = points {
        req.points = u64::from(str2u(v));
    }
    if let Some(v) = timeout {
        req.timeout_ms = str2i(v);
    }
    if let Some(v) = resampling {
        req.resampling_time = str2l(v);
    }
    req
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_parameters_follow_c() {
        let p = parse_v1(
            b"chart=system.cpu&dims=user&dimension=system&after=-60&points=-5&a==b&x=&group=max\
              &format=csv&options=abs|reversed&options=nonzero&tqx=reqId:7;sig:0x10;out:html&callback=f(1)\
              &limit=3&tier=2",
            1,
        );
        let r = &p.request;
        assert_eq!(p.chart.as_deref(), Some(&b"system.cpu"[..]));
        assert_eq!(r.instances.as_deref(), Some(&b"system.cpu"[..]));
        assert_eq!(r.dimensions.as_deref(), Some(&b"|user|system"[..]));
        assert_eq!((r.after, r.before), (-60, 0));
        assert_eq!(r.points, (-5i64) as u64, "an int sign-extends into size_t");
        assert_eq!(r.time_group, TimeGrouping::Max);
        assert_eq!(r.format, Format::Html, "tqx out overrides format");
        assert_eq!(
            r.options,
            options::ABSOLUTE | options::REVERSED | options::NONZERO
        );
        assert_eq!(r.google.req_id, b"7");
        assert_eq!(r.google.timestamp, 16);
        assert_eq!(r.google.response_handler.as_deref(), Some(&b"f_1_"[..]));
        assert_eq!(r.cardinality_limit, 4);
        assert_eq!(r.tier, 0, "tier 2 is not below 1 tier");
        assert!(is_valid_sp(p.chart.as_deref()) && !is_valid_sp(Some(b"*")));
    }

    #[test]
    fn v1_defaults() {
        let r = parse_v1(b"", 3).request;
        assert_eq!((r.after, r.before, r.points), (-600, 0, 0));
        assert_eq!(
            (r.format, r.time_group, r.options),
            (Format::Json, TimeGrouping::Average, 0)
        );
        assert_eq!(r.google, Google::default());
    }

    #[test]
    fn v2_parameters_follow_c() {
        let r = parse_v2(
            b"scope_nodes=a&group_by=node&group_by=instance&group_by=label&group_by_label=k\
              &aggregation[1]=sum&points=-5&options=debug,minify&cardinality_limit=0&limit=9",
            3,
            1,
        );
        assert_eq!(r.version, 3);
        assert_eq!(r.group_by[0].group_by, group_by::NODE | group_by::LABEL);
        assert_eq!(
            r.group_by[1].group_by,
            group_by::LABEL,
            "later group_by values land in pass 1"
        );
        assert_eq!(r.group_by[1].aggregation, Aggregation::Sum);
        assert_eq!(r.points, 0, "str2u gives 0 for a negative value");
        assert_ne!(
            r.options & options::ABSOLUTE,
            0,
            "grouping without dimension forces absolute"
        );
        assert_eq!(r.options & options::MINIFY, 0, "debug clears minify");
        assert_ne!(r.options & options::DEBUG, 0);
        assert_eq!(r.cardinality_limit, 0);
        let d = parse_v2(b"", 2, 1);
        assert_eq!(d.group_by[0].group_by, group_by::DIMENSION);
        assert_eq!(d.format, Format::Json2);
        assert_eq!(
            d.options,
            options::VIRTUAL_POINTS | options::JSON_WRAP | options::RETURN_JWAR
        );
    }
}
