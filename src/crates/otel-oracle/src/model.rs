//! Spans as the plugin stores them, rebuilt from OTLP and the documented
//! storage rules (not from the plugin's code):
//!
//! - ids of the wrong length, or all zero, are unset; an unset parent makes a
//!   root;
//! - duration = end − start, 0 when the end is unset or before the start,
//!   saturating;
//! - every stored field is a `path=value` pair: span fields at the top level,
//!   `attributes.*`, `resource.attributes.*`, `scope.name`, `scope.version`,
//!   `scope.attributes.*`; nested maps join keys with `.`, array elements
//!   collapse to `[]`; values render strings raw, ints and doubles in decimal
//!   (Rust `{}`), bools `true`/`false`, bytes lowercase hex, an empty array
//!   `[]`, an empty map `{}`; a key that is empty becomes `_` and `=` in a key
//!   becomes `_`;
//! - enum fields store a label (`kind`, `status_code`) and the raw int
//!   (`_kind`, `_status_code`); UNSPECIFIED and UNSET store nothing;
//! - two derived fields: `_role` and `_duration_band`.

use std::collections::{BTreeMap, BTreeSet};

use opentelemetry_proto::tonic::{
    collector::trace::v1::ExportTraceServiceRequest,
    common::v1::{AnyValue, KeyValue, any_value},
    trace::v1::Span,
};

pub const ROLE_FIELD: &str = "_role";
pub const DURATION_BAND_FIELD: &str = "_duration_band";
pub const STATUS_FIELD: &str = "status_code";
pub const SERVICE_FIELD: &str = "resource.attributes.service.name";

/// Band labels, fastest first, and the lower edges (ns) of bands 1.., each
/// edge inclusive: <1ms, 1-10ms, 10-100ms, 100ms-1s, 1-10s, >10s.
pub const DURATION_BANDS: [&str; 6] = ["<1ms", "1-10ms", "10-100ms", "100ms-1s", "1-10s", ">10s"];
const BAND_EDGES_NS: [i64; 5] = [
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
    10_000_000_000,
];

/// One stored span row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleSpan {
    pub trace_id: Option<[u8; 16]>,
    pub span_id: Option<[u8; 8]>,
    pub parent_span_id: Option<[u8; 8]>,
    /// The row timestamp: the span start.
    pub start_ns: i64,
    pub duration_ns: i64,
    /// Every stored field and its values (several for array elements).
    pub fields: BTreeMap<String, BTreeSet<String>>,
    /// Which stored unit the row landed in (a sealed file, or the live WAL);
    /// set by whoever knows the membership.
    pub unit: usize,
}

impl OracleSpan {
    /// Whether the row has `field=value`.
    pub fn has(&self, field: &str, value: &str) -> bool {
        self.fields
            .get(field)
            .is_some_and(|values| values.contains(value))
    }

    pub fn is_error(&self) -> bool {
        self.has(STATUS_FIELD, "ERROR")
    }
}

/// The rows a request produces, in request order, all in `unit`.
pub fn spans_of_request(request: &ExportTraceServiceRequest, unit: usize) -> Vec<OracleSpan> {
    let mut out = Vec::new();
    for rs in &request.resource_spans {
        let mut context: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        if let Some(resource) = &rs.resource {
            for kv in &resource.attributes {
                render_kv("resource.attributes", kv, &mut context);
            }
        }
        for ss in &rs.scope_spans {
            let mut scoped = context.clone();
            if let Some(scope) = &ss.scope {
                if !scope.name.is_empty() {
                    add(&mut scoped, "scope.name", scope.name.clone());
                }
                if !scope.version.is_empty() {
                    add(&mut scoped, "scope.version", scope.version.clone());
                }
                for kv in &scope.attributes {
                    render_kv("scope.attributes", kv, &mut scoped);
                }
            }
            for span in &ss.spans {
                out.push(span_row(span, &scoped, unit));
            }
        }
    }
    out
}

fn span_row(span: &Span, context: &BTreeMap<String, BTreeSet<String>>, unit: usize) -> OracleSpan {
    let parent_span_id = id::<8>(&span.parent_span_id);
    let duration_ns =
        if span.end_time_unix_nano == 0 || span.end_time_unix_nano < span.start_time_unix_nano {
            0
        } else {
            i64::try_from(span.end_time_unix_nano - span.start_time_unix_nano).unwrap_or(i64::MAX)
        };

    let mut fields = context.clone();
    if !span.name.is_empty() {
        add(&mut fields, "name", span.name.clone());
    }
    if span.kind != 0 {
        if let Some(label) = kind_label(span.kind) {
            add(&mut fields, "kind", label.to_string());
        }
        add(&mut fields, "_kind", span.kind.to_string());
    }
    if !span.trace_state.is_empty() {
        add(&mut fields, "trace_state", span.trace_state.clone());
    }
    if let Some(status) = &span.status {
        if status.code != 0 {
            if let Some(label) = status_label(status.code) {
                add(&mut fields, STATUS_FIELD, label.to_string());
            }
            add(&mut fields, "_status_code", status.code.to_string());
        }
        if !status.message.is_empty() {
            add(&mut fields, "status_message", status.message.clone());
        }
    }
    for kv in &span.attributes {
        render_kv("attributes", kv, &mut fields);
    }
    // An event's name (even an empty one) and attributes, and a link's
    // attributes, are fields of the span's row; event times, link ids and link
    // trace state are not.
    for event in &span.events {
        add(&mut fields, "events.name", event.name.clone());
        for kv in &event.attributes {
            render_kv("events.attributes", kv, &mut fields);
        }
    }
    for link in &span.links {
        for kv in &link.attributes {
            render_kv("links.attributes", kv, &mut fields);
        }
    }
    add(
        &mut fields,
        ROLE_FIELD,
        role(parent_span_id.is_some(), span.kind).to_string(),
    );
    add(
        &mut fields,
        DURATION_BAND_FIELD,
        DURATION_BANDS[band(duration_ns)].to_string(),
    );

    OracleSpan {
        trace_id: id::<16>(&span.trace_id),
        span_id: id::<8>(&span.span_id),
        parent_span_id,
        start_ns: i64::try_from(span.start_time_unix_nano).unwrap_or(i64::MAX),
        duration_ns,
        fields,
        unit,
    }
}

/// `root` without a parent; otherwise by kind.
pub fn role(has_parent: bool, kind: i32) -> &'static str {
    if !has_parent {
        return "root";
    }
    match kind {
        2 | 5 => "inbound",
        3 | 4 => "outbound",
        _ => "internal",
    }
}

/// Index into [`DURATION_BANDS`].
pub fn band(duration_ns: i64) -> usize {
    let mut index = 0;
    for edge in BAND_EDGES_NS {
        if duration_ns >= edge {
            index += 1;
        }
    }
    index
}

fn id<const N: usize>(bytes: &[u8]) -> Option<[u8; N]> {
    let array: [u8; N] = bytes.try_into().ok()?;
    if array == [0u8; N] { None } else { Some(array) }
}

fn kind_label(kind: i32) -> Option<&'static str> {
    match kind {
        1 => Some("INTERNAL"),
        2 => Some("SERVER"),
        3 => Some("CLIENT"),
        4 => Some("PRODUCER"),
        5 => Some("CONSUMER"),
        _ => None,
    }
}

fn status_label(code: i32) -> Option<&'static str> {
    match code {
        1 => Some("OK"),
        2 => Some("ERROR"),
        _ => None,
    }
}

fn add(fields: &mut BTreeMap<String, BTreeSet<String>>, path: &str, value: String) {
    fields.entry(path.to_string()).or_default().insert(value);
}

fn render_kv(prefix: &str, kv: &KeyValue, out: &mut BTreeMap<String, BTreeSet<String>>) {
    let key = if kv.key.is_empty() {
        "_".to_string()
    } else {
        kv.key.replace('=', "_")
    };
    render_value(&format!("{prefix}.{key}"), kv.value.as_ref(), out);
}

fn render_value(
    path: &str,
    value: Option<&AnyValue>,
    out: &mut BTreeMap<String, BTreeSet<String>>,
) {
    let Some(inner) = value.and_then(|v| v.value.as_ref()) else {
        add(out, path, String::new());
        return;
    };
    match inner {
        any_value::Value::StringValue(s) => add(out, path, s.clone()),
        any_value::Value::BoolValue(b) => add(out, path, b.to_string()),
        any_value::Value::IntValue(i) => add(out, path, i.to_string()),
        any_value::Value::DoubleValue(d) => add(out, path, format!("{d}")),
        any_value::Value::BytesValue(bytes) => {
            let mut hex = String::with_capacity(bytes.len() * 2);
            for byte in bytes {
                hex.push_str(&format!("{byte:02x}"));
            }
            add(out, path, hex);
        }
        any_value::Value::ArrayValue(array) => {
            if array.values.is_empty() {
                add(out, path, "[]".to_string());
            } else {
                let element_path = format!("{path}[]");
                for element in &array.values {
                    render_value(&element_path, Some(element), out);
                }
            }
        }
        any_value::Value::KvlistValue(list) => {
            if list.values.is_empty() {
                add(out, path, "{}".to_string());
            } else {
                for kv in &list.values {
                    render_kv(path, kv, out);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::{
        common::v1::{ArrayValue, KeyValueList},
        resource::v1::Resource,
        trace::v1::{ResourceSpans, ScopeSpans, Status},
    };

    fn value(v: any_value::Value) -> Option<AnyValue> {
        Some(AnyValue { value: Some(v) })
    }

    fn kv(key: &str, v: any_value::Value) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: value(v),
        }
    }

    fn one_span(span: Span) -> OracleSpan {
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![kv(
                        "service.name",
                        any_value::Value::StringValue("api".into()),
                    )],
                    ..Default::default()
                }),
                scope_spans: vec![ScopeSpans {
                    spans: vec![span],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        spans_of_request(&request, 3).remove(0)
    }

    fn values(span: &OracleSpan, field: &str) -> Vec<String> {
        span.fields
            .get(field)
            .map(|v| v.iter().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn renders_fields_like_the_storage_rules() {
        use any_value::Value as V;
        let span = one_span(Span {
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            parent_span_id: vec![3; 5],
            name: "op".into(),
            kind: 3,
            start_time_unix_nano: 1_000,
            end_time_unix_nano: 2_500_000,
            status: Some(Status {
                code: 2,
                message: "boom".into(),
            }),
            attributes: vec![
                kv("s", V::StringValue("x".into())),
                kv("b", V::BoolValue(true)),
                kv("i", V::IntValue(-7)),
                kv("d", V::DoubleValue(1.5)),
                kv("bytes", V::BytesValue(vec![0xab, 0x01])),
                kv(
                    "arr",
                    V::ArrayValue(ArrayValue {
                        values: vec![
                            AnyValue {
                                value: Some(V::StringValue("a".into())),
                            },
                            AnyValue {
                                value: Some(V::StringValue("a".into())),
                            },
                        ],
                    }),
                ),
                kv("empty", V::ArrayValue(ArrayValue { values: vec![] })),
                kv(
                    "map",
                    V::KvlistValue(KeyValueList {
                        values: vec![kv("k", V::IntValue(1))],
                    }),
                ),
                kv("a=b", V::StringValue("eq".into())),
                kv("", V::StringValue("blank".into())),
            ],
            ..Default::default()
        });
        assert_eq!(span.unit, 3);
        assert_eq!(span.parent_span_id, None, "a wrong-length parent is unset");
        assert_eq!(span.duration_ns, 2_499_000);
        let want: [(&str, &str); 18] = [
            ("resource.attributes.service.name", "api"),
            ("name", "op"),
            ("kind", "CLIENT"),
            ("_kind", "3"),
            ("status_code", "ERROR"),
            ("_status_code", "2"),
            ("status_message", "boom"),
            ("attributes.s", "x"),
            ("attributes.b", "true"),
            ("attributes.i", "-7"),
            ("attributes.d", "1.5"),
            ("attributes.bytes", "ab01"),
            ("attributes.arr[]", "a"),
            ("attributes.empty", "[]"),
            ("attributes.map.k", "1"),
            ("attributes.a_b", "eq"),
            ("_role", "root"),
            ("_duration_band", "1-10ms"),
        ];
        for (field, value) in want {
            assert_eq!(values(&span, field), [value], "{field}");
        }
        assert_eq!(values(&span, "attributes._"), ["blank"]);
        assert!(span.is_error());
    }

    #[test]
    fn unset_enums_and_empty_names_store_nothing() {
        let span = one_span(Span {
            parent_span_id: vec![9; 8],
            status: Some(Status {
                code: 0,
                message: String::new(),
            }),
            ..Default::default()
        });
        for field in [
            "name",
            "kind",
            "_kind",
            "status_code",
            "_status_code",
            "status_message",
        ] {
            assert!(!span.fields.contains_key(field), "{field}");
        }
        assert_eq!(values(&span, ROLE_FIELD), ["internal"]);
    }

    #[test]
    fn bands_include_their_lower_edge() {
        let cases: [(i64, &str); 8] = [
            (0, "<1ms"),
            (999_999, "<1ms"),
            (1_000_000, "1-10ms"),
            (10_000_000, "10-100ms"),
            (100_000_000, "100ms-1s"),
            (1_000_000_000, "1-10s"),
            (10_000_000_000, ">10s"),
            (i64::MAX, ">10s"),
        ];
        for (duration, want) in cases {
            assert_eq!(DURATION_BANDS[band(duration)], want, "{duration}");
        }
    }

    #[test]
    fn roles_follow_parent_then_kind() {
        let cases: [(bool, i32, &str); 8] = [
            (false, 3, "root"),
            (true, 2, "inbound"),
            (true, 5, "inbound"),
            (true, 3, "outbound"),
            (true, 4, "outbound"),
            (true, 1, "internal"),
            (true, 0, "internal"),
            (true, 42, "internal"),
        ];
        for (has_parent, kind, want) in cases {
            assert_eq!(role(has_parent, kind), want, "{has_parent} {kind}");
        }
    }
}
