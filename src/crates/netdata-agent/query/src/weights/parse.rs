//! The request of the weights endpoints as its handler reads it (`web_client_api_request_weights()`,
//! `src/web/api/v2/api_v2_weights.c`): `/api/v1/weights`, `/api/v1/metric_correlations`, `/api/v2/weights` and
//! `/api/v3/weights` share one parser, which reads some names for version 1 alone and others from version 2 on.

use netdata_agent_text::parse::{str2l, str2ul};

use super::Method;
use super::methods::Texts;
use crate::request::{GroupByPass, pairs};
use crate::tables::{Aggregation, TimeGrouping, group_by, options, parse_group_by, parse_options};

/// `WEIGHTS_FORMAT`: no parameter chooses it; each route has its own, and the MCP tool has the fourth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Charts,
    Contexts,
    Multinode,
    Mcp,
}

/// The body of the parser's one refusal (a 400 with the JSON content type), byte for byte.
pub const LIMIT_ERROR: &str =
    r#"{"error":"Weights limits must be nonnegative integers within the supported range."}"#;

/// A `limit` or `cardinality_limit` that is no number: the answer is [`LIMIT_ERROR`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitError;

/// `QUERY_WEIGHTS_REQUEST` as the handler fills it. No host: C gives a version-1 request none, and reads a later
/// version's only where the version is 1.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightsRequest {
    pub version: u8,
    pub texts: Texts,
    pub group_by: GroupByPass,
    pub method: Method,
    pub format: Format,
    pub time_group: TimeGrouping,
    pub time_group_options: Option<Vec<u8>>,
    pub baseline_after: i64,
    pub baseline_before: i64,
    pub after: i64,
    pub before: i64,
    pub points: u64,
    pub options: u64,
    pub tier: u64,
    pub timeout_ms: i64,
    /// `cardinality_limit` when it was given (a 0 too), else `limit`.
    pub cardinality_limit: u64,
}

/// `weights_parse_limit()`: decimal digits alone, at least one, and a number that fits.
fn parse_limit(value: &[u8]) -> Option<u64> {
    if value.is_empty() {
        return None;
    }
    let mut n: u64 = 0;
    for &c in value {
        if !c.is_ascii_digit() {
            return None;
        }
        let digit = u64::from(c - b'0');
        if n > (u64::MAX - digit) / 10 {
            return None;
        }
        n = n * 10 + digit;
    }
    Some(n)
}

/// The handler's loop over the query and what it does after it. `method` and `format` are the route's; a `method`
/// parameter replaces the first (a name that is no method is `ks2`, whatever the route's was). C leaves the loop
/// at once for a limit that is no number.
/// - An item without a name or without a value is passed; a later value of a name replaces an earlier one, except
///   `options`, which add up.
/// - Version 1 reads `group`, `group_options` and `context` or `contexts` (the scope of contexts); later versions
///   read `time_group`, `time_group_options`, the eleven selectors and the first group-by pass.
/// - `tier` selects a tier the agent has; any other number is tier 0, not selected.
/// - No option at all (a `tier` that selects counts as one): unaligned, null-to-zero and nonzero; otherwise
///   unaligned and null-to-zero are added. `percentage` adds `absolute`; `debug` removes `minify`.
pub fn parse(
    query: &[u8],
    version: u8,
    method: Method,
    format: Format,
    storage_tiers: u64,
) -> Result<WeightsRequest, LimitError> {
    let mut req = WeightsRequest {
        version,
        texts: Texts::default(),
        group_by: GroupByPass { group_by: group_by::NONE, label: None, aggregation: Aggregation::Average },
        method,
        format,
        time_group: TimeGrouping::Average,
        time_group_options: None,
        baseline_after: 0,
        baseline_before: 0,
        after: 0,
        before: 0,
        points: 0,
        options: 0,
        tier: 0,
        timeout_ms: 0,
        cardinality_limit: 0,
    };
    let (mut cardinality_limit, mut limit) = (None, 0);
    let text = |value: &[u8]| Some(value.to_vec());
    let (v1, later) = (version == 1, version >= 2);
    for (name, value) in pairs(query) {
        match name {
            b"baseline_after" => req.baseline_after = str2l(value),
            b"baseline_before" => req.baseline_before = str2l(value),
            b"after" | b"highlight_after" => req.after = str2l(value),
            b"before" | b"highlight_before" => req.before = str2l(value),
            b"points" | b"max_points" => req.points = str2ul(value),
            b"timeout" => req.timeout_ms = str2l(value),
            b"cardinality_limit" => cardinality_limit = Some(parse_limit(value).ok_or(LimitError)?),
            b"limit" => limit = parse_limit(value).ok_or(LimitError)?,
            b"group" if v1 => req.time_group = TimeGrouping::parse(value),
            b"time_group" if later => req.time_group = TimeGrouping::parse(value),
            b"group_options" if v1 => req.time_group_options = text(value),
            b"time_group_options" if later => req.time_group_options = text(value),
            b"options" => req.options |= parse_options(value),
            b"method" => req.method = Method::parse(value),
            b"context" | b"contexts" if v1 => req.texts.scope_contexts = text(value),
            b"scope_nodes" if later => req.texts.scope_nodes = text(value),
            b"scope_contexts" if later => req.texts.scope_contexts = text(value),
            b"scope_instances" if later => req.texts.scope_instances = text(value),
            b"scope_labels" if later => req.texts.scope_labels = text(value),
            b"scope_dimensions" if later => req.texts.scope_dimensions = text(value),
            b"nodes" if later => req.texts.nodes = text(value),
            b"contexts" if later => req.texts.contexts = text(value),
            b"instances" if later => req.texts.instances = text(value),
            b"dimensions" if later => req.texts.dimensions = text(value),
            b"labels" if later => req.texts.labels = text(value),
            b"alerts" if later => req.texts.alerts = text(value),
            b"group_by" | b"group_by[0]" if later => req.group_by.group_by = parse_group_by(value),
            b"group_by_label" | b"group_by_label[0]" if later => req.group_by.label = text(value),
            b"aggregation" | b"aggregation[0]" if later => req.group_by.aggregation = Aggregation::parse(value),
            b"tier" => {
                req.tier = str2ul(value);
                if req.tier < storage_tiers {
                    req.options |= options::SELECTED_TIER;
                } else {
                    req.tier = 0;
                }
            }
            _ => {}
        }
    }
    if req.options == 0 {
        req.options = options::NOT_ALIGNED | options::NULL2ZERO | options::NONZERO;
    } else {
        req.options |= options::NOT_ALIGNED | options::NULL2ZERO;
    }
    if req.options & options::PERCENTAGE != 0 {
        req.options |= options::ABSOLUTE;
    }
    if req.options & options::DEBUG != 0 {
        req.options &= !options::MINIFY;
    }
    req.cardinality_limit = cardinality_limit.unwrap_or(limit);
    Ok(req)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULTS: u64 = options::NOT_ALIGNED | options::NULL2ZERO | options::NONZERO;
    const ADDED: u64 = options::NOT_ALIGNED | options::NULL2ZERO;

    fn v1(query: &str) -> WeightsRequest {
        parse(query.as_bytes(), 1, Method::AnomalyRate, Format::Contexts, 3).expect("a request")
    }

    fn v2(query: &str) -> WeightsRequest {
        parse(query.as_bytes(), 2, Method::Value, Format::Multinode, 3).expect("a request")
    }

    fn some(text: &str) -> Option<Vec<u8>> {
        Some(text.as_bytes().to_vec())
    }

    /// The route's method and format, zeros, the average, no group-by, and the three default options.
    #[test]
    fn an_empty_query_is_the_route_s_request() {
        let expected = WeightsRequest {
            version: 1,
            texts: Texts::default(),
            group_by: GroupByPass { group_by: group_by::NONE, label: None, aggregation: Aggregation::Average },
            method: Method::AnomalyRate,
            format: Format::Contexts,
            time_group: TimeGrouping::Average,
            time_group_options: None,
            baseline_after: 0,
            baseline_before: 0,
            after: 0,
            before: 0,
            points: 0,
            options: DEFAULTS,
            tier: 0,
            timeout_ms: 0,
            cardinality_limit: 0,
        };
        assert_eq!(v1(""), expected);
        // an item without a value, a name without a value, an unknown name
        assert_eq!(v1("&&after=&=5&limit=&nope=1&after"), expected);
        let later = v2("");
        let route = (later.version, later.method, later.format, later.options);
        assert_eq!(route, (2, Method::Value, Format::Multinode, DEFAULTS));
    }

    /// The numbers, their second names, the last value of a name, and separators that repeat.
    #[test]
    fn the_windows_the_points_and_the_timeout_are_numbers() {
        let r = v1("after=-600&before=-10&baseline_after=-1200&baseline_before=-600&points=30&timeout=2500");
        assert_eq!((r.after, r.before, r.baseline_after, r.baseline_before), (-600, -10, -1200, -600));
        assert_eq!((r.points, r.timeout_ms), (30, 2500));
        let r = v2("highlight_after=5&highlight_before=9&max_points=7&after=6");
        assert_eq!((r.after, r.before, r.points), (6, 9, 7));
        // an item is cut at its first `=`: `a==b` gives a the value `=b`, which is no number; an empty name before
        // the first `=` is passed over, so `=a=b` is a's value b
        let r = v1("after==42&&&=before=43&points==7");
        assert_eq!((r.after, r.before, r.points), (0, 43, 0));
    }

    /// `cardinality_limit` beats `limit` whenever it is given, a 0 too; either must be digits alone that fit.
    #[test]
    fn the_limit_is_digits_alone_and_cardinality_limit_wins() {
        assert_eq!(v1("limit=5").cardinality_limit, 5);
        assert_eq!(v1("limit=5&limit=7").cardinality_limit, 7);
        assert_eq!(v1("cardinality_limit=3&limit=5").cardinality_limit, 3);
        assert_eq!(v1("limit=2&cardinality_limit=0").cardinality_limit, 0);
        assert_eq!(v2("cardinality_limit=18446744073709551615").cardinality_limit, u64::MAX);
        assert_eq!(v2("limit=007").cardinality_limit, 7);
        for text in ["-1", "+1", "1.5", "abc", "1e3", "%201", "1x", "18446744073709551616", "99999999999999999999"] {
            for name in ["limit", "cardinality_limit"] {
                let query = format!("after=-60&{name}={text}&options=raw");
                let refused = parse(query.as_bytes(), 2, Method::Value, Format::Multinode, 3);
                assert_eq!(refused, Err(LimitError), "{query}");
            }
        }
        // an empty limit is no value at all: the item is passed
        assert_eq!(v1("limit=&cardinality_limit=").cardinality_limit, 0);
        assert!(LIMIT_ERROR.starts_with(r#"{"error":"Weights limits must be"#), "{LIMIT_ERROR}");
        assert!(LIMIT_ERROR.ends_with(r#"range."}"#), "{LIMIT_ERROR}");
    }

    /// Options add up; none at all gets the three defaults; any gets two of them; `percentage` adds `absolute`;
    /// `debug` removes `minify`; a tier the agent has is an option too.
    #[test]
    fn the_options_get_their_defaults() {
        assert_eq!(v1("options=bogus").options, DEFAULTS);
        assert_eq!(v1("options=raw").options, options::RETURN_RAW | ADDED);
        let both = options::RETURN_RAW | options::ANOMALY_BIT | ADDED;
        assert_eq!(v1("options=raw&options=anomaly-bit").options, both);
        assert_eq!(v1("options=nonzero").options, DEFAULTS);
        assert_eq!(v1("options=percentage").options, options::PERCENTAGE | options::ABSOLUTE | ADDED);
        assert_eq!(v1("options=debug,minify").options, options::DEBUG | ADDED);
        assert_eq!(v1("options=minify").options, options::MINIFY | ADDED);
        // the tier
        let selected = v1("tier=0");
        assert_eq!((selected.tier, selected.options), (0, options::SELECTED_TIER | ADDED));
        let second = v1("tier=2");
        assert_eq!((second.tier, second.options), (2, options::SELECTED_TIER | ADDED));
        // a tier the agent does not have is tier 0, not selected
        for query in ["tier=3", "tier=99"] {
            let none = v1(query);
            assert_eq!((none.tier, none.options), (0, DEFAULTS), "{query}");
        }
        // a text is 0, which the agent has; a later tier it lacks resets the number and leaves the option
        let text = v1("tier=abc&tier=3");
        assert_eq!((text.tier, text.options), (0, options::SELECTED_TIER | ADDED));
    }

    /// A `method` that is none of the four is ks2, whatever the route's method.
    #[test]
    fn a_method_replaces_the_route_s() {
        assert_eq!(v1("method=volume").method, Method::Volume);
        assert_eq!(v2("method=anomaly-rate").method, Method::AnomalyRate);
        assert_eq!(v2("method=anomaly_rate").method, Method::Ks2);
        assert_eq!(v1("method=VALUE").method, Method::Ks2);
        assert_eq!(v1("method=value&method=ks2").method, Method::Ks2);
    }

    /// What each version reads: version 1 the time grouping as `group` and one scope of contexts; later versions
    /// `time_group`, the eleven selectors and the first group-by pass. Each ignores the other's names.
    #[test]
    fn each_version_reads_its_own_names() {
        let r = v1("group=max&group_options=o1&context=a.b&contexts=c.d");
        assert_eq!((r.time_group, r.time_group_options.clone()), (TimeGrouping::Max, some("o1")));
        assert_eq!(r.texts, Texts { scope_contexts: some("c.d"), ..Texts::default() });
        let later_names = concat!(
            "time_group=max&time_group_options=o&scope_nodes=n&nodes=n&instances=i&dimensions=d&labels=l&alerts=a",
            "&scope_contexts=s&scope_instances=s&scope_labels=s&scope_dimensions=s&group_by=node&aggregation=sum",
        );
        let r = v1(later_names);
        assert_eq!((r.time_group, r.time_group_options.clone()), (TimeGrouping::Average, None));
        assert_eq!(r.texts, Texts::default());
        assert_eq!((r.group_by.group_by, r.group_by.aggregation), (group_by::NONE, Aggregation::Average));

        let query = concat!(
            "scope_nodes=sn&scope_contexts=sc&scope_instances=si&scope_labels=sl&scope_dimensions=sd",
            "&nodes=n&contexts=c&instances=i&dimensions=d&labels=l&alerts=a&time_group=min&time_group_options=tgo",
        );
        let r = v2(query);
        let texts = Texts {
            scope_nodes: some("sn"),
            scope_contexts: some("sc"),
            scope_instances: some("si"),
            scope_labels: some("sl"),
            scope_dimensions: some("sd"),
            nodes: some("n"),
            contexts: some("c"),
            instances: some("i"),
            dimensions: some("d"),
            labels: some("l"),
            alerts: some("a"),
        };
        assert_eq!(r.texts, texts);
        assert_eq!((r.time_group, r.time_group_options.clone()), (TimeGrouping::Min, some("tgo")));
        let r = v2("group=max&group_options=o&context=a.b");
        assert_eq!((r.time_group, r.time_group_options.clone()), (TimeGrouping::Average, None));
        assert_eq!(r.texts, Texts::default());
        // the first group-by pass, by either name; the second pass's names are not read
        let pass = |r: &WeightsRequest| (r.group_by.group_by, r.group_by.label.clone(), r.group_by.aggregation);
        let r = v2("group_by=node&group_by_label=role&aggregation=sum");
        assert_eq!(pass(&r), (parse_group_by(b"node"), some("role"), Aggregation::parse(b"sum")));
        let r = v2("group_by[0]=context&group_by_label[0]=x&aggregation[0]=max&group_by[1]=node&aggregation[1]=min");
        assert_eq!(pass(&r), (parse_group_by(b"context"), some("x"), Aggregation::parse(b"max")));
        assert_ne!(parse_group_by(b"node"), group_by::NONE);
    }
}
