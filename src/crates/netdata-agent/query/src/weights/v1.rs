//! The two formats of the version-1 weights endpoints (`src/web/api/queries/weights.c`): `charts`
//! (`registered_results_to_json_charts()`, of `/api/v1/metric_correlations`) and `contexts`
//! (`registered_results_to_json_contexts()`, of `/api/v1/weights`), with the header they share.

use std::sync::Arc;

use netdata_agent_text::json::{JsonOptions, JsonWriter};

use super::Method;
use super::engine::Finished;
use crate::tables::{options, options_to_json_array};

/// The writer of every format: minified when the request says so.
pub(super) fn writer(finished: &Finished) -> JsonWriter {
    let minify = finished.request.options & options::MINIFY != 0;
    JsonWriter::new(if minify { JsonOptions::MINIFY } else { JsonOptions::DEFAULT })
}

/// `results_header_to_json()`: the windows, the points, what the queries read, the time grouping, the method and
/// the options.
fn header(w: &mut JsonWriter, finished: &Finished, storage_tiers: usize) {
    let req = &finished.request;
    let rfc3339 = req.options & options::RFC3339 != 0;
    w.member_add_time_t_formatted("after", req.after, rfc3339);
    w.member_add_time_t_formatted("before", req.before, rfc3339);
    w.member_add_time_t("duration", req.before - req.after);
    w.member_add_uint64("points", req.points);
    if matches!(req.method, Method::Ks2 | Method::Volume) {
        w.member_add_time_t_formatted("baseline_after", req.baseline_after, rfc3339);
        w.member_add_time_t_formatted("baseline_before", req.baseline_before, rfc3339);
        w.member_add_time_t("baseline_duration", req.baseline_before - req.baseline_after);
        w.member_add_uint64("baseline_points", req.points.checked_shl(finished.shifts).unwrap_or(0));
    }
    w.member_add_object("statistics");
    w.member_add_double("query_time_ms", finished.duration_us as f64 / 1000.0);
    db_counters(w, finished, storage_tiers);
    w.object_close();
    w.member_add_string("group", req.time_group.name());
    w.member_add_string("method", req.method.name());
    options_to_json_array(w, b"options", req.options);
}

/// What the queries read: the members of version 1's `statistics` after its time, and all of the later
/// versions' `db`.
pub(super) fn db_counters(w: &mut JsonWriter, finished: &Finished, storage_tiers: usize) {
    let stats = &finished.stats;
    w.member_add_uint64("db_queries", stats.db_queries as u64);
    w.member_add_uint64("query_result_points", stats.result_points as u64);
    w.member_add_uint64("binary_searches", stats.binary_searches as u64);
    w.member_add_uint64("db_points_read", stats.db_points as u64);
    w.member_add_array(Some(b"db_points_per_tier"));
    for points in stats.db_points_per_tier.iter().take(storage_tiers) {
        w.add_array_item_uint64(*points as u64);
    }
    w.array_close();
}

/// What every format ends with: how many results there are and how many metrics were examined, then
/// `weights_result_limit_to_json()` when the request has a limit: how many of `total` (dimensions, or groups:
/// the `unit`) were returned.
pub(super) fn footer(w: &mut JsonWriter, finished: &Finished, unit: &str, total: usize, returned: usize) {
    w.member_add_uint64("correlated_dimensions", finished.results.len() as u64);
    w.member_add_uint64("total_dimensions_count", finished.examined as u64);
    let limit = finished.request.cardinality_limit;
    if limit != 0 {
        w.member_add_object("result_limit");
        w.member_add_uint64("limit", limit);
        w.member_add_uint64("total", total as u64);
        w.member_add_uint64("returned", returned as u64);
        w.member_add_string("unit", unit);
        w.member_add_boolean("truncated", total > returned);
        w.member_add_string("summary_scope", "all");
        w.object_close();
    }
}

/// `registered_results_to_json_charts()`: the results by instance, a new object whenever the instance is another
/// than the result's before it (by identity: another host's instance of the same id repeats the key), each
/// dimension by the metric's name; with a limit only the selected results. Returns the body and how many
/// dimensions it holds.
pub fn charts(finished: &Finished, storage_tiers: usize) -> (Vec<u8>, usize) {
    let mut w = writer(finished);
    header(&mut w, finished, storage_tiers);
    w.member_add_object("correlated_charts");
    let limit = finished.request.cardinality_limit != 0;
    let mut dimensions = 0;
    let mut last = None;
    for t in &finished.results {
        if limit && !t.selected {
            continue;
        }
        if last.is_none_or(|last| !Arc::ptr_eq(last, &t.instance)) {
            if last.is_some() {
                w.object_close();
                w.object_close();
            }
            last = Some(&t.instance);
            w.member_add_object(t.instance.id());
            w.member_add_string("context", t.context.id());
            w.member_add_object("dimensions");
        }
        w.member_add_double(t.metric.state().name, t.value);
        dimensions += 1;
    }
    if dimensions != 0 {
        w.object_close();
        w.object_close();
    }
    w.object_close();
    footer(&mut w, finished, "dimensions", finished.results.len(), dimensions);
    w.finalize();
    (w.into_bytes(), dimensions)
}

/// `registered_results_to_json_contexts()`: the results by context and instance (each by identity, the instance
/// anew in every context), with two means: a chart's weight over its results in the context, a context's over
/// all its results. With a limit: only a context that holds a selected result, in it only an instance that
/// holds one, and of it only the selected dimensions are printed, while the means still count every result of
/// what is shown. Returns the body and how many dimensions it holds.
pub fn contexts(finished: &Finished, storage_tiers: usize) -> (Vec<u8>, usize) {
    let mut w = writer(finished);
    header(&mut w, finished, storage_tiers);
    w.member_add_object("contexts");
    let limit = finished.request.cardinality_limit != 0;
    let (mut contexts, mut charts, mut dimensions) = (0, 0, 0);
    let (mut context_dims, mut chart_dims) = (0usize, 0usize);
    let (mut context_weight, mut chart_weight) = (0.0, 0.0);
    let (mut last_context, mut last_instance) = (None, None);
    for t in &finished.results {
        if limit && !t.context_selected {
            continue;
        }
        if last_context.is_none_or(|last| !Arc::ptr_eq(last, &t.context)) {
            last_context = Some(&t.context);
            if contexts != 0 {
                w.object_close();
                w.member_add_double("weight", chart_weight / chart_dims as f64);
                w.object_close();
                w.object_close();
                w.member_add_double("weight", context_weight / context_dims as f64);
                w.object_close();
            }
            w.member_add_object(t.context.id());
            w.member_add_object("charts");
            contexts += 1;
            charts = 0;
            context_dims = 0;
            context_weight = 0.0;
            last_instance = None;
        }
        context_weight += t.value;
        context_dims += 1;
        if limit && !t.instance_selected {
            continue;
        }
        if last_instance.is_none_or(|last| !Arc::ptr_eq(last, &t.instance)) {
            last_instance = Some(&t.instance);
            if charts != 0 {
                w.object_close();
                w.member_add_double("weight", chart_weight / chart_dims as f64);
                w.object_close();
            }
            w.member_add_object(t.instance.id());
            w.member_add_object("dimensions");
            charts += 1;
            chart_dims = 0;
            chart_weight = 0.0;
        }
        chart_weight += t.value;
        chart_dims += 1;
        if !limit || t.selected {
            w.member_add_double(t.metric.state().name, t.value);
            dimensions += 1;
        }
    }
    if dimensions != 0 {
        w.object_close();
        w.member_add_double("weight", chart_weight / chart_dims as f64);
        w.object_close();
        w.object_close();
        w.member_add_double("weight", context_weight / context_dims as f64);
        w.object_close();
    }
    w.object_close();
    footer(&mut w, finished, "dimensions", finished.results.len(), dimensions);
    w.finalize();
    (w.into_bytes(), dimensions)
}

#[cfg(test)]
mod tests {
    use super::super::methods::Stats;
    use super::super::parse::{Format, parse};
    use super::super::results::{Found, Of, Registered, register, select};
    use super::*;
    use crate::target::Versions;
    use crate::testing::weights_host_as;
    use std::time::Instant;

    /// A finished request of `method` with results of the fixture's metrics on two hosts: (host, metric, value).
    fn finished(method: Method, query: &str, values: &[(usize, &str, f64)]) -> Finished {
        let hosts = [weights_host_as("guid-1", "one"), weights_host_as("guid-2", "two")];
        let (mut results, mut stats): (Vec<Registered>, Stats) = (Vec::new(), Stats::default());
        for &(host, dimension, value) in values {
            let h = &hosts[host];
            let rc = h.contexts().get("ctx.w").expect("the fixture's context");
            let ri = rc.instances().into_iter().next().expect("its instance");
            let rm = ri.metric(dimension).expect("the metric");
            let of = Of { host: h, hostname: "h", context: &rc, instance: &ri, metric: &rm };
            let found = Found { value, flags: 0, highlighted: None, baseline: None, duration_us: 0 };
            register(&mut results, &mut stats, true, &of, found);
        }
        let mut request = parse(query.as_bytes(), 1, method, Format::Contexts, 1).expect("a request");
        // the windows as the engine leaves them, and no option but what the query gave
        (request.after, request.before, request.baseline_after, request.baseline_before) = (1000, 1060, 760, 1000);
        request.options &= options::MINIFY | options::RFC3339;
        if request.cardinality_limit != 0 {
            let limit = usize::try_from(request.cardinality_limit).unwrap();
            select(&mut results, limit, false);
        }
        stats = Stats { db_points: 484, result_points: 4, db_queries: 4, ..Stats::default() };
        stats.db_points_per_tier[0] = 484;
        let (received, versions) = (Instant::now(), Versions::default());
        Finished { request, shifts: 2, results, stats, examined: 10, received, duration_us: 2500, versions }
    }

    fn text((body, dimensions): (Vec<u8>, usize)) -> (String, usize) {
        (String::from_utf8(body).unwrap(), dimensions)
    }

    const THREE: [(usize, &str, f64); 3] = [(0, "a", 0.5), (0, "b", 0.25), (1, "a", 0.75)];
    const STATISTICS: &str = concat!(
        r#""statistics":{"query_time_ms":2.5,"db_queries":4,"query_result_points":4,"binary_searches":0,"#,
        r#""db_points_read":484,"db_points_per_tier":[484]},"#
    );

    /// The header: the windows and their durations, the points, the statistics, the grouping, the method and the
    /// options; the baseline's four members for the two methods that have one.
    #[test]
    fn the_header_has_the_windows_the_statistics_and_the_request() {
        let value = text(charts(&finished(Method::Value, "options=minify&points=7", &THREE), 1)).0;
        let head = format!(
            r#"{{"after":1000,"before":1060,"duration":60,"points":7,{STATISTICS}"group":"average","method":"value","#
        );
        assert!(value.starts_with(&head), "{value}");
        let ks2 = text(charts(&finished(Method::Ks2, "options=minify&points=20&group=max", &THREE), 1)).0;
        let head = format!(
            concat!(
                r#"{{"after":1000,"before":1060,"duration":60,"points":20,"baseline_after":760,"#,
                r#""baseline_before":1000,"baseline_duration":240,"baseline_points":80,{}"group":"max","method":"ks2","#
            ),
            STATISTICS
        );
        assert!(ks2.starts_with(&head), "{ks2}");
        // the options as the options' own writer prints them, and a storage tier more
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        options_to_json_array(&mut w, b"options", options::MINIFY);
        w.finalize();
        let options_member = String::from_utf8(w.into_bytes()).unwrap();
        let after_method = &value[value.find(r#""method":"value","#).unwrap() + 17..];
        assert!(after_method.starts_with(&options_member[1..options_member.len() - 1]), "{value}");
        let two_tiers = text(charts(&finished(Method::Value, "options=minify", &THREE), 2)).0;
        assert!(two_tiers.contains(r#""db_points_per_tier":[484,0]}"#), "{two_tiers}");
        // not minified without the option
        assert!(text(charts(&finished(Method::Value, "", &THREE), 1)).0.contains('\n'));
    }

    /// The charts format: an object per run of results of one instance, the same id again for another host's
    /// instance; with a limit only the selected results, and the limit's own object.
    #[test]
    fn the_charts_format_lists_the_dimensions_by_instance() {
        let (body, dimensions) = text(charts(&finished(Method::Value, "options=minify", &THREE), 1));
        let tail = concat!(
            r#""correlated_charts":{"t.w":{"context":"ctx.w","dimensions":{"a":0.5,"b":0.25}},"#,
            r#""t.w":{"context":"ctx.w","dimensions":{"a":0.75}}},"correlated_dimensions":3,"#,
            r#""total_dimensions_count":10}"#
        );
        assert!(body.ends_with(tail), "{body}");
        assert_eq!(dimensions, 3);

        let (body, dimensions) = text(charts(&finished(Method::Value, "options=minify&limit=1", &THREE), 1));
        let tail = concat!(
            r#""correlated_charts":{"t.w":{"context":"ctx.w","dimensions":{"a":0.75}}},"correlated_dimensions":3,"#,
            r#""total_dimensions_count":10,"result_limit":{"limit":1,"total":3,"returned":1,"unit":"dimensions","#,
            r#""truncated":true,"summary_scope":"all"}}"#
        );
        assert!(body.ends_with(tail), "{body}");
        assert_eq!(dimensions, 1);

        // no result: an empty object, and nothing printed
        let (body, dimensions) = text(charts(&finished(Method::Value, "options=minify", &[]), 1));
        let none = r#""correlated_charts":{},"correlated_dimensions":0,"total_dimensions_count":10}"#;
        assert!(body.ends_with(none), "{body}");
        assert_eq!(dimensions, 0);
    }

    /// The contexts format: contexts, their charts and the two means; with a limit a context's and a chart's
    /// mean still count the results that are not printed.
    #[test]
    fn the_contexts_format_has_the_means_of_what_it_shows() {
        let (body, dimensions) = text(contexts(&finished(Method::Value, "options=minify", &THREE), 1));
        let tail = concat!(
            r#""contexts":{"ctx.w":{"charts":{"t.w":{"dimensions":{"a":0.5,"b":0.25},"weight":0.375}},"#,
            r#""weight":0.375},"ctx.w":{"charts":{"t.w":{"dimensions":{"a":0.75},"weight":0.75}},"weight":0.75}},"#,
            r#""correlated_dimensions":3,"total_dimensions_count":10}"#
        );
        assert!(body.ends_with(tail), "{body}");
        assert_eq!(dimensions, 3);

        // one of two results of a chart is selected: the other still counts in both means
        let two = [(0, "a", 0.5), (0, "b", 0.75), (1, "a", 0.25)];
        let (body, dimensions) = text(contexts(&finished(Method::Value, "options=minify&limit=1", &two), 1));
        let tail = concat!(
            r#""contexts":{"ctx.w":{"charts":{"t.w":{"dimensions":{"b":0.75},"weight":0.625}},"weight":0.625}},"#,
            r#""correlated_dimensions":3,"total_dimensions_count":10,"result_limit":{"limit":1,"total":3,"#,
            r#""returned":1,"unit":"dimensions","truncated":true,"summary_scope":"all"}}"#
        );
        assert!(body.ends_with(tail), "{body}");
        assert_eq!(dimensions, 1);

        let (body, dimensions) = text(contexts(&finished(Method::Value, "options=minify", &[]), 1));
        assert!(body.ends_with(r#""contexts":{},"correlated_dimensions":0,"total_dimensions_count":10}"#), "{body}");
        assert_eq!(dimensions, 0);
    }
}
