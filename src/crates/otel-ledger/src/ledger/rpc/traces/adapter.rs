//! Mapping between the `otel-traces` wire types and the wire-neutral
//! [`sfsq::traces`] engine — the traces analogue of the logs `adapter`.
//! Wire shapes live in [`super::wire`]; this module owns the
//! request-side parsing (hex ids, trace bounds, the explorer's windows and
//! terms) and the response-side conversion (engine data → wire, ids
//! rendered as W3C lowercase hex).

use sfsq::traces::{FieldKinds, TraceData};

use super::wire::{
    CoverageWire, EventWire, FieldKindsWire, LinkWire, SpanWire, StatusWire, TraceItems,
    TraceResult,
};

/// Parse a W3C text-form trace id: exactly 32 hex chars (16 bytes),
/// case-insensitive. The all-zero (unset) id parses here — the engine
/// rejects it with its own precise message.
pub(crate) fn parse_trace_id(s: &str) -> Result<sfst::TraceId, String> {
    let s = s.trim();
    if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "trace id must be 32 hex characters (16 bytes), got {s:?}"
        ));
    }
    let mut bytes = [0u8; 16];
    for (i, chunk) in s.as_bytes().chunks_exact(2).enumerate() {
        // Infallible: both bytes were checked hex above.
        bytes[i] = u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap();
    }
    Ok(sfst::TraceId::from(bytes))
}

/// Validate the `trace` sub-object's optional assembly bounds into a
/// capture range. Both-or-neither; `after < before` — violations are
/// client errors. Any width is accepted: the agent is relaxed on time ranges
/// and the response's `coverage` declares the range used. `None` =
/// full retention.
pub(crate) fn validate_trace_bounds(
    after: Option<u32>,
    before: Option<u32>,
) -> Result<Option<std::ops::Range<u32>>, String> {
    let (after, before) = match (after, before) {
        (None, None) => return Ok(None),
        (Some(a), Some(b)) => (a, b),
        _ => {
            return Err(
                "trace bounds require both 'after' and 'before' (or neither for full retention)"
                    .to_string(),
            );
        }
    };
    if after >= before {
        return Err(format!(
            "invalid trace bounds: after {after} >= before {before}"
        ));
    }
    Ok(Some(after..before))
}

/// Shape one assembled trace into the wire result. `trace_id` is echoed
/// back in canonical (lowercase) hex regardless of the request's casing.
/// `coverage` is the capture range assembly actually used.
pub(crate) fn to_trace_result(
    trace_id: &sfst::TraceId,
    data: TraceData,
    coverage: CoverageWire,
) -> TraceResult {
    let t = data.trace;
    let summary_root = t.summary_root();
    let family = data.family;
    let returned = t.spans.len();
    let mut spans = Vec::with_capacity(returned);
    for (index, span) in t.spans.into_iter().enumerate() {
        spans.push(span_wire(
            span,
            family.child_ns[index],
            family.error_origin[index],
        ));
    }
    let log_streams = log_streams(&spans);
    TraceResult {
        mode: "trace",
        version: 1,
        trace_id: trace_id.to_string(),
        coverage,
        status: StatusWire::from(&data.status),
        items: TraceItems { returned },
        summary_root,
        roots: t.roots,
        children: t.children,
        spans,
        field_kinds: field_kinds_wire(data.field_kinds),
        log_streams,
    }
}

/// The logs stream key of each service the spans name: the logs ingest keys
/// a stream by its resource's `service.namespace` and `service.name`.
fn log_streams(spans: &[SpanWire]) -> Vec<String> {
    let mut keys = std::collections::BTreeSet::new();
    for span in spans {
        let value = |key: &str| {
            span.fields
                .iter()
                .find(|(field, _)| field == key)
                .map_or("", |(_, value)| value.as_str())
        };
        let stream = otel_logs_identity::ServiceStream::new(
            value("resource.attributes.service.namespace"),
            value("resource.attributes.service.name"),
        );
        keys.insert(format!("{:016x}", otel_logs_identity::part_key(&stream)));
    }
    keys.into_iter().collect()
}

/// `child_ns` is the time the span's children cover and `error_origin` its
/// error-origin flag, both over the assembled trace.
fn span_wire(s: sfst::TraceSpan, child_ns: i64, error_origin: bool) -> SpanWire {
    SpanWire {
        span_id: s.span_id.to_string(),
        // OTel semantics: an unset parent means "root", rendered as an
        // absent field rather than the all-zero sentinel string.
        parent_span_id: (!s.parent_span_id.is_unset()).then(|| s.parent_span_id.to_string()),
        start_ns: s.start_ns,
        duration_ns: s.duration_ns,
        self_duration_ns: s.duration_ns - child_ns,
        error_origin,
        kind: s.kind,
        flags: s.flags,
        dropped_attributes_count: s.dropped_attributes_count,
        dropped_events_count: s.dropped_events_count,
        dropped_links_count: s.dropped_links_count,
        fields: s.fields,
        events: s.events.into_iter().map(event_wire).collect(),
        links: s.links.into_iter().map(link_wire).collect(),
    }
}

fn event_wire(e: sfst::TraceEvent) -> EventWire {
    EventWire {
        time_unix_nano: e.time_unix_nano,
        name: e.name,
        dropped_attributes_count: e.dropped_attributes_count,
        attributes: e.attributes,
    }
}

fn link_wire(l: sfst::TraceLink) -> LinkWire {
    LinkWire {
        trace_id: l.trace_id.to_string(),
        span_id: l.span_id.to_string(),
        trace_state: l.trace_state,
        flags: l.flags,
        dropped_attributes_count: l.dropped_attributes_count,
        attributes: l.attributes,
    }
}

fn field_kinds_wire(k: FieldKinds) -> FieldKindsWire {
    let section = |v: Vec<(String, sfst::ValueKind)>| -> Vec<(String, &'static str)> {
        v.into_iter().map(|(n, k)| (n, kind_word(k))).collect()
    };
    FieldKindsWire {
        fields: section(k.fields),
        event_attributes: section(k.event_attributes),
        link_attributes: section(k.link_attributes),
    }
}

/// The wire word for each schema value kind. Exhaustive on purpose: a
/// new engine kind fails compilation here until its wire name is decided.
fn kind_word(k: sfst::ValueKind) -> &'static str {
    match k {
        sfst::ValueKind::Null => "null",
        sfst::ValueKind::Bool => "bool",
        sfst::ValueKind::Int => "int",
        sfst::ValueKind::Double => "double",
        sfst::ValueKind::Str => "str",
        sfst::ValueKind::Bytes => "bytes",
        sfst::ValueKind::EmptyKvlist => "empty_kvlist",
        sfst::ValueKind::EmptyArray => "empty_array",
        sfst::ValueKind::Kvlist => "kvlist",
        sfst::ValueKind::Array => "array",
    }
}

/// The engine request for an explorer call received at `now_s`, plus the
/// window aligned outward to whole buckets (the range files are captured
/// for).
pub(crate) fn to_explore_query(
    params: &super::wire::ExploreParams,
    now_s: u32,
) -> (sfsq::traces::explore::ExploreQuery, u32, u32) {
    use sfsq::traces::explore::{
        ExploreQuery, ExploreScope, ExploreSelection, FacetSpec, HistogramSpec, Sections,
    };

    let (after, before) = params.window.resolve(now_s);
    // A relative window reaching before the epoch clamps to 0; keep at
    // least one second so the grid holds a bucket.
    let (grid, aligned_after, aligned_before) =
        super::super::grid::grid_for_window_s(after, before.max(after.saturating_add(1)));
    let filter_of = |chips: &super::wire::Chips| {
        let mut filter = sfst::Filter::new();
        for (field, values) in chips {
            for value in values {
                filter = match value {
                    Some(value) => filter.select(field.clone(), value.clone()),
                    None => filter.select_absent(field.clone()),
                };
            }
        }
        filter
    };
    let filter = filter_of(&params.filter);
    let selection = params.selection.as_ref().map(|selection| ExploreSelection {
        filter: filter_of(&selection.filter),
        duration: selection.duration,
        time_ns: selection.time_ns.clone(),
    });
    let query = ExploreQuery {
        grid,
        scope: ExploreScope {
            filter,
            text: params.text.as_deref().map(sfst::text::LiteralText::new),
            trace_ids: params.trace_ids.clone(),
            duration: params.duration,
        },
        selection,
        sections: Sections {
            histogram: params.histogram.as_ref().map(|h| HistogramSpec {
                stack: h.stack.clone(),
                percentiles: h.percentiles,
                durations: h.durations,
            }),
            facets: params.facets.as_ref().map(|f| FacetSpec {
                fields: f.fields.clone(),
            }),
            groups: params.groups,
            rows: params.rows.clone(),
            fields: params.fields,
        },
    };
    (query, aligned_after, aligned_before)
}

/// The engine request for a value-suggestion call received at `now_s`, plus the
/// window in unix seconds (the range files are captured for).
pub(crate) fn to_values_query(
    params: &super::wire::ValuesParams,
    now_s: u32,
) -> (sfsq::traces::explore::ValuesQuery, u32, u32) {
    let (after, before) = params.window.resolve(now_s);
    // A relative window reaching before the epoch clamps to 0; keep at least
    // one second.
    let before = before.max(after.saturating_add(1));
    let query = sfsq::traces::explore::ValuesQuery {
        window: i64::from(after) * 1_000_000_000..i64::from(before) * 1_000_000_000,
        field: params.field.clone(),
        prefix: params.prefix.clone(),
        limit: params.limit,
    };
    (query, after, before)
}

pub(crate) fn to_values_response(
    data: sfsq::traces::explore::ValuesData,
    field: String,
) -> super::wire::ValuesResponse {
    super::wire::ValuesResponse {
        mode: "values",
        version: 1,
        field,
        values: data.values,
        truncated: data.truncated,
        status: StatusWire::from(&data.status),
    }
}

/// The explorer's engine answer on the wire.
pub(crate) fn to_explore_response(
    data: sfsq::traces::explore::ExploreData,
    grid: sfst::Grid,
    after: u32,
    before: u32,
) -> super::wire::ExploreResponse {
    use super::wire::{
        BucketWire, ComparisonTotalsWire, ComparisonWire, DeltaOtherWire, DeltaRowWire,
        DeltaSideWire, ExploreDataWire, ExploreFacetValueWire, ExploreFacetWire, ExploreFacetsWire,
        ExploreResponse, FieldWire, FieldsWire, GridWire, GroupNumbersWire, GroupWire,
        GroupsDeltaWire, GroupsWire, HistogramWire, OtherGroupsWire, PercentileMethodWire,
        PercentilesWire, RowWire, RowsWire, TotalsWire, UnavailableFacetWire, WindowWire,
    };
    let numbers = |n: sfsq::traces::explore::GroupNumbers| GroupNumbersWire {
        spans: n.spans,
        errors: n.errors,
        errors_originated: n.errors_originated,
        p95_ns: n.p95_ns,
        self_ns: n.self_ns.to_string(),
    };
    let percentiles = |p: Option<sfsq::traces::explore::Percentiles>| {
        p.map(|p| PercentilesWire {
            p50_ns: p.p50_ns,
            p95_ns: p.p95_ns,
            p99_ns: p.p99_ns,
        })
    };

    let status = StatusWire::from(&data.status);
    let histogram = data.histogram.map(|h| {
        let mut buckets = Vec::with_capacity(h.buckets.len());
        for b in h.buckets {
            buckets.push(BucketWire {
                counts: b.counts,
                unset: b.unset,
                other: b.other,
                percentiles: percentiles(b.percentiles),
                durations: b.durations,
            });
        }
        HistogramWire {
            status: StatusWire::from(&h.status),
            stack: h.stack,
            dimensions: h.dimensions,
            buckets,
            totals: TotalsWire {
                count: h.totals.count,
                errors: h.totals.errors,
                percentiles: percentiles(h.totals.percentiles),
            },
            percentiles: h.percentiles.then_some(PercentileMethodWire {
                approximate: true,
                max_relative_error: sfsq::traces::duration_hist::MAX_RELATIVE_ERROR,
            }),
            duration_rows: h
                .durations
                .then(sfsq::traces::duration_hist::heatmap_row_bounds),
        }
    });
    let facets = data.facets.map(|f| {
        let mut fields = Vec::with_capacity(f.fields.len());
        for facet in f.fields {
            let mut values = Vec::with_capacity(facet.values.len());
            for v in facet.values {
                let compared = v.comparison;
                values.push(ExploreFacetValueWire {
                    value: v.value,
                    count: v.count,
                    selection: compared.as_ref().map(|c| c.selection),
                    baseline: compared.as_ref().map(|c| c.baseline),
                    eligible: compared.as_ref().map(|c| c.eligible),
                    rank: compared.as_ref().and_then(|c| c.rank),
                    diff: compared.as_ref().and_then(|c| c.diff).map(|d| d.to_f64()),
                });
            }
            let compared = facet.comparison;
            fields.push(ExploreFacetWire {
                field: facet.field,
                values,
                omitted_values: facet.omitted_values,
                omitted_rows: facet.omitted_rows,
                rank: compared.as_ref().and_then(|c| c.rank),
                best_diff: compared.as_ref().and_then(|c| c.best).map(|d| d.to_f64()),
                totals: compared.as_ref().map(|c| ComparisonTotalsWire {
                    scope: c.totals.scope,
                    selection: c.totals.selection,
                }),
                in_selection: facet.in_selection.then_some(true),
            });
        }
        let mut unavailable = Vec::with_capacity(f.unavailable.len());
        for (field, reason) in f.unavailable {
            unavailable.push(UnavailableFacetWire {
                field,
                reason: reason.into(),
            });
        }
        ExploreFacetsWire {
            status: StatusWire::from(&f.status),
            fields,
            unavailable,
            comparison: f.comparison.map(|c| ComparisonWire {
                scope: c.scope,
                selection: c.selection,
                min_support: sfsq::traces::explore::MIN_SUPPORT,
            }),
        }
    });
    let rows = data.rows.map(|r| {
        let mut items = Vec::with_capacity(r.items.len());
        for row in r.items {
            let mut columns = std::collections::BTreeMap::new();
            for (name, values) in r.columns.iter().zip(row.columns) {
                if !values.is_empty() {
                    columns.insert(name.clone(), values.join(", "));
                }
            }
            items.push(RowWire {
                cursor: row.key.encode(),
                start_ns: row.key.start_ns.to_string(),
                duration_ns: row.duration_ns,
                self_duration_ns: row.self_duration_ns,
                trace_id: row.key.trace_id.to_string(),
                span_id: row.key.span_id.to_string(),
                service: row.service,
                name: row.name,
                role: row.role,
                status: row.status,
                columns,
            });
        }
        RowsWire {
            status: StatusWire::from(&r.status),
            order: match r.order {
                sfsq::traces::explore::RowOrder::Newest { .. } => "newest",
                sfsq::traces::explore::RowOrder::Slowest => "slowest",
            },
            matched: r.matched,
            has_older: r.more.map(|m| m.older),
            has_newer: r.more.map(|m| m.newer),
            items,
        }
    });
    let fields = data.fields.map(|f| {
        let mut items = Vec::with_capacity(f.items.len());
        for field in f.items {
            items.push(FieldWire {
                tier: match field.tier {
                    sfst::FieldTier::Low => "low",
                    sfst::FieldTier::Mid => "mid",
                    sfst::FieldTier::High => "high",
                },
                name: field.name,
                chip: field.chip,
                facet: field.facet,
                stack: field.stack,
                text: field.text,
                column: field.column,
            });
        }
        FieldsWire {
            status: StatusWire::from(&f.status),
            items,
            columns: f.columns,
        }
    });
    let side = |n: sfsq::traces::explore::SideNumbers| DeltaSideWire {
        spans: n.spans,
        errors_originated: n.errors_originated,
        self_ns: n.self_ns.to_string(),
    };
    let groups = data.groups.map(|g| {
        let mut rows = Vec::with_capacity(g.rows.len());
        let mut delta_rows = Vec::new();
        for row in g.rows {
            if let Some(sides) = row.delta {
                delta_rows.push(DeltaRowWire {
                    service: row.key.service.clone(),
                    operation: row.key.operation.clone(),
                    selection: side(sides.selection),
                    baseline: side(sides.baseline),
                });
            }
            rows.push(GroupWire {
                service: row.key.service,
                operation: row.key.operation,
                numbers: numbers(row.numbers),
            });
        }
        let delta_other = g.other.as_ref().and_then(|other| {
            other.delta.map(|sides| DeltaOtherWire {
                groups: other.groups,
                selection: side(sides.selection),
                baseline: side(sides.baseline),
            })
        });
        GroupsWire {
            status: StatusWire::from(&g.status),
            window_s: g.window_s,
            self_ns_total: g.self_ns_total.to_string(),
            rows,
            other: g.other.map(|other| OtherGroupsWire {
                groups: other.groups,
                numbers: numbers(other.numbers),
            }),
            delta: g.delta.map(|delta| GroupsDeltaWire {
                selection_traces: delta.selection_traces,
                baseline_traces: delta.baseline_traces,
                selection_self_ns_total: delta.selection_self_ns_total.to_string(),
                baseline_self_ns_total: delta.baseline_self_ns_total.to_string(),
                rows: delta_rows,
                other: delta_other,
            }),
        }
    });
    ExploreResponse {
        status: 200,
        response_type: "traces",
        data: ExploreDataWire {
            mode: "explore",
            version: 1,
            window: WindowWire {
                after,
                before,
                grid: GridWire {
                    start_ns: grid.bucket_start_ns.to_string(),
                    bucket_ns: grid.bucket_width_ns,
                    buckets: grid.num_buckets,
                },
            },
            status,
            histogram,
            facets,
            groups,
            rows,
            fields,
        },
    }
}

#[cfg(test)]
mod tests;
