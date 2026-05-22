//! AST → Plan lowering.
//!
//! Translates an [`nlogql::ast::Expr`] into a [`crate::plan::Plan`].
//! The lowering layer is also the home for the semantic checks
//! that the parser intentionally defers (see `EXPECTED_FAILS.md`
//! in the `nlogql` crate):
//!
//! - `topk` / `bottomk` / `approx_topk` require a numeric first arg.
//! - Other vector ops reject a numeric first arg.
//! - `quantile_over_time` requires its quantile in `[0, 1]`.
//! - Other range ops reject a numeric first arg.

use nlogql::ast::{
    Expr, LogRangeExpr, PipelineExpr, PipelineStage, RangeAggregationExpr, RangeOp, UnwrapExpr,
    VectorAggregationExpr, VectorOp,
};

use crate::error::LowerError;
use crate::plan::{
    BinaryPlan, LabelReplacePlan, LogPlan, LogRange, LogStage, MetricPlan, Plan, RangeAggPlan,
    Unwrap, VectorAggPlan,
};

/// Lower a parsed LogQL AST into an executable [`Plan`].
pub fn lower(expr: &Expr) -> Result<Plan, LowerError> {
    match expr {
        Expr::Selector(s) => Ok(Plan::Log(LogPlan {
            matchers: s.matchers.clone(),
            stages: Vec::new(),
        })),
        Expr::Pipeline(p) => lower_pipeline(p).map(Plan::Log),
        // Everything else is a metric-path expression.
        _ => lower_metric(expr).map(Plan::Metric),
    }
}

fn lower_pipeline(p: &PipelineExpr) -> Result<LogPlan, LowerError> {
    let mut stages = Vec::with_capacity(p.stages.len());
    for stage in &p.stages {
        stages.push(lower_stage(stage)?);
    }
    Ok(LogPlan {
        matchers: p.selector.matchers.clone(),
        stages,
    })
}

fn lower_stage(stage: &PipelineStage) -> Result<LogStage, LowerError> {
    match stage {
        PipelineStage::LineFilter(f) => Ok(LogStage::LineFilter(f.clone())),
        PipelineStage::Parser(p) => Ok(LogStage::Parser(p.clone())),
        PipelineStage::LabelFilter(f) => Ok(LogStage::LabelFilter(f.clone())),
        PipelineStage::Decolorize(_) => Ok(LogStage::Decolorize),
        PipelineStage::DropLabels(l) => Ok(LogStage::DropLabels(l.clone())),
        PipelineStage::KeepLabels(l) => Ok(LogStage::KeepLabels(l.clone())),
        PipelineStage::LineFormat(s) => Err(LowerError::DeferredStage {
            stage: "line_format",
            span: s.span,
        }),
        PipelineStage::LabelFormat(s) => Err(LowerError::DeferredStage {
            stage: "label_format",
            span: s.span,
        }),
    }
}

// -- Metric path -------------------------------------------------

fn lower_metric(expr: &Expr) -> Result<MetricPlan, LowerError> {
    match expr {
        Expr::Literal(l) => Ok(MetricPlan::Literal(l.value)),
        Expr::Vector(v) => Ok(MetricPlan::Vector(v.value)),
        Expr::RangeAggregation(r) => lower_range_agg(r),
        Expr::VectorAggregation(v) => lower_vector_agg(v),
        Expr::Binary(b) => Ok(MetricPlan::Binary(BinaryPlan {
            op: b.op,
            lhs: Box::new(lower_metric(&b.lhs)?),
            rhs: Box::new(lower_metric(&b.rhs)?),
            modifier: b.modifier.clone(),
        })),
        Expr::LabelReplace(lr) => Ok(MetricPlan::LabelReplace(LabelReplacePlan {
            inner: Box::new(lower_metric(&lr.expr)?),
            dst_label: lr.dst_label.clone(),
            replacement: lr.replacement.clone(),
            src_label: lr.src_label.clone(),
            regex: lr.regex.clone(),
        })),
        Expr::Selector(_) | Expr::Pipeline(_) => Err(LowerError::LogInMetricPosition {
            span: expr.span(),
        }),
    }
}

fn lower_range_agg(r: &RangeAggregationExpr) -> Result<MetricPlan, LowerError> {
    let op_name = range_op_name(r.op);
    match (r.op, r.parameter) {
        (RangeOp::QuantileOverTime, None) => {
            return Err(LowerError::MissingParameter {
                op: op_name,
                span: r.span,
            });
        }
        (RangeOp::QuantileOverTime, Some(q)) if !(0.0..=1.0).contains(&q) => {
            return Err(LowerError::QuantileOutOfRange {
                value: q,
                span: r.span,
            });
        }
        (op, Some(_)) if op != RangeOp::QuantileOverTime => {
            return Err(LowerError::UnexpectedParameter {
                op: op_name,
                span: r.span,
            });
        }
        _ => {}
    }
    Ok(MetricPlan::RangeAgg(RangeAggPlan {
        op: r.op,
        log: lower_log_range(&r.log_range)?,
        parameter: r.parameter,
        grouping: r.grouping.clone(),
    }))
}

fn lower_vector_agg(v: &VectorAggregationExpr) -> Result<MetricPlan, LowerError> {
    let op_name = vector_op_name(v.op);
    let needs_param = matches!(
        v.op,
        VectorOp::TopK | VectorOp::BottomK | VectorOp::ApproxTopK,
    );
    match (needs_param, v.parameter) {
        (true, None) => {
            return Err(LowerError::MissingParameter {
                op: op_name,
                span: v.span,
            });
        }
        (false, Some(_)) => {
            return Err(LowerError::UnexpectedParameter {
                op: op_name,
                span: v.span,
            });
        }
        _ => {}
    }
    Ok(MetricPlan::VectorAgg(VectorAggPlan {
        op: v.op,
        parameter: v.parameter,
        grouping: v.grouping.clone(),
        inner: Box::new(lower_metric(&v.expr)?),
    }))
}

fn lower_log_range(lr: &LogRangeExpr) -> Result<LogRange, LowerError> {
    let mut stages = Vec::with_capacity(lr.stages.len());
    for s in &lr.stages {
        stages.push(lower_stage(s)?);
    }
    Ok(LogRange {
        matchers: lr.selector.matchers.clone(),
        stages,
        unwrap: lr.unwrap.as_ref().map(lower_unwrap),
        range_ns: lr.range_ns,
        offset_ns: lr.offset_ns,
    })
}

fn lower_unwrap(u: &UnwrapExpr) -> Unwrap {
    Unwrap {
        conv_op: u.conv_op,
        identifier: u.identifier.clone(),
        post_filters: u.post_filters.clone(),
    }
}

// Stable string names for ops, used in error messages. Reuses the
// AST Display impls, but copies into &'static str slots that fit
// LowerError's variant shape.
fn range_op_name(op: RangeOp) -> &'static str {
    match op {
        RangeOp::AbsentOverTime => "absent_over_time",
        RangeOp::AvgOverTime => "avg_over_time",
        RangeOp::BytesOverTime => "bytes_over_time",
        RangeOp::BytesRate => "bytes_rate",
        RangeOp::CountOverTime => "count_over_time",
        RangeOp::FirstOverTime => "first_over_time",
        RangeOp::LastOverTime => "last_over_time",
        RangeOp::MaxOverTime => "max_over_time",
        RangeOp::MinOverTime => "min_over_time",
        RangeOp::QuantileOverTime => "quantile_over_time",
        RangeOp::Rate => "rate",
        RangeOp::RateCounter => "rate_counter",
        RangeOp::StddevOverTime => "stddev_over_time",
        RangeOp::StdvarOverTime => "stdvar_over_time",
        RangeOp::SumOverTime => "sum_over_time",
    }
}

fn vector_op_name(op: VectorOp) -> &'static str {
    match op {
        VectorOp::Sum => "sum",
        VectorOp::Avg => "avg",
        VectorOp::Min => "min",
        VectorOp::Max => "max",
        VectorOp::Stddev => "stddev",
        VectorOp::Stdvar => "stdvar",
        VectorOp::Count => "count",
        VectorOp::BottomK => "bottomk",
        VectorOp::TopK => "topk",
        VectorOp::Sort => "sort",
        VectorOp::SortDesc => "sort_desc",
        VectorOp::ApproxTopK => "approx_topk",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nlogql::parse;

    fn lower_str(s: &str) -> Result<Plan, LowerError> {
        let expr = parse(s).expect("parse");
        lower(&expr)
    }

    fn expect_log(s: &str) -> LogPlan {
        match lower_str(s).unwrap_or_else(|e| panic!("lower failed for {s:?}: {e}")) {
            Plan::Log(p) => p,
            Plan::Metric(_) => panic!("expected Log, got Metric for {s:?}"),
        }
    }

    fn expect_metric(s: &str) -> MetricPlan {
        match lower_str(s).unwrap_or_else(|e| panic!("lower failed for {s:?}: {e}")) {
            Plan::Metric(p) => p,
            Plan::Log(_) => panic!("expected Metric, got Log for {s:?}"),
        }
    }

    // ---- log path (carried over from SOW-D2) -----------------------

    #[test]
    fn bare_selector() {
        let p = expect_log(r#"{app="foo"}"#);
        assert_eq!(p.matchers.len(), 1);
        assert!(p.stages.is_empty());
    }

    #[test]
    fn multi_stage_pipeline() {
        let p = expect_log(
            r#"{app="foo"} |= "x" | logfmt | latency > 1s | decolorize | drop trace_id"#,
        );
        assert_eq!(p.stages.len(), 5);
    }

    #[test]
    fn line_format_is_deferred() {
        let err = lower_str(r#"{app="foo"} | line_format "{{.x}}""#).unwrap_err();
        assert!(matches!(err, LowerError::DeferredStage { stage: "line_format", .. }));
    }

    // ---- metric path -----------------------------------------------

    #[test]
    fn literal_lowers() {
        let m = expect_metric("42");
        assert_eq!(m, MetricPlan::Literal(42.0));
    }

    #[test]
    fn vector_n_lowers() {
        let m = expect_metric("vector(3.14)");
        assert_eq!(m, MetricPlan::Vector(3.14));
    }

    #[test]
    fn rate_lowers() {
        let m = expect_metric(r#"rate({app="foo"}[5m])"#);
        match m {
            MetricPlan::RangeAgg(p) => {
                assert_eq!(p.op, RangeOp::Rate);
                assert_eq!(p.log.range_ns, 5 * 60 * 1_000_000_000);
                assert!(p.parameter.is_none());
                assert!(p.grouping.is_none());
            }
            other => panic!("expected RangeAgg, got {other:?}"),
        }
    }

    #[test]
    fn quantile_over_time_with_valid_q() {
        let m = expect_metric(r#"quantile_over_time(0.99, {app="foo"} | unwrap latency [5m])"#);
        match m {
            MetricPlan::RangeAgg(p) => {
                assert_eq!(p.op, RangeOp::QuantileOverTime);
                assert_eq!(p.parameter, Some(0.99));
                assert!(p.log.unwrap.is_some());
            }
            other => panic!("expected RangeAgg, got {other:?}"),
        }
    }

    #[test]
    fn quantile_without_param_rejected() {
        let err = lower_str(r#"quantile_over_time({app="foo"}[5m])"#).unwrap_err();
        assert!(matches!(
            err,
            LowerError::MissingParameter { op: "quantile_over_time", .. },
        ));
    }

    #[test]
    fn quantile_out_of_range_rejected() {
        // Loki's NUMBER token is unsigned, so a negative quantile
        // can't be expressed in valid LogQL — `-0.1` lexes as SUB
        // then NUMBER and fails to parse. Out-of-range here means
        // strictly > 1.
        for bad in ["1.5", "2", "100"] {
            let q = format!(r#"quantile_over_time({bad}, {{app="foo"}}[5m])"#);
            let err = lower_str(&q).unwrap_err();
            assert!(
                matches!(err, LowerError::QuantileOutOfRange { .. }),
                "expected QuantileOutOfRange for q={bad}, got {err:?}",
            );
        }
    }

    #[test]
    fn count_over_time_with_param_rejected() {
        // Closes the parser leniency: only quantile_over_time takes
        // a parameter; the others must reject one at lower time.
        let err = lower_str(r#"count_over_time(5, {app="foo"}[5m])"#).unwrap_err();
        assert!(matches!(
            err,
            LowerError::UnexpectedParameter { op: "count_over_time", .. },
        ));
    }

    #[test]
    fn sum_of_rate_lowers() {
        let m = expect_metric(r#"sum(rate({app="foo"}[5m]))"#);
        match m {
            MetricPlan::VectorAgg(p) => {
                assert_eq!(p.op, VectorOp::Sum);
                assert!(p.parameter.is_none());
                assert!(matches!(*p.inner, MetricPlan::RangeAgg(_)));
            }
            other => panic!("expected VectorAgg, got {other:?}"),
        }
    }

    #[test]
    fn topk_with_param() {
        let m = expect_metric(r#"topk(5, rate({app="foo"}[5m]))"#);
        match m {
            MetricPlan::VectorAgg(p) => {
                assert_eq!(p.op, VectorOp::TopK);
                assert_eq!(p.parameter, Some(5.0));
            }
            other => panic!("expected VectorAgg, got {other:?}"),
        }
    }

    #[test]
    fn topk_without_param_rejected() {
        // Closes the second parser leniency from EXPECTED_FAILS.md.
        let err = lower_str(r#"topk(rate({app="foo"}[5m]))"#).unwrap_err();
        assert!(matches!(
            err,
            LowerError::MissingParameter { op: "topk", .. },
        ));
    }

    #[test]
    fn sum_with_param_rejected() {
        // Closes the first parser leniency: sum doesn't take a
        // parameter, but the parser accepts `sum(N, expr)`.
        let err = lower_str(r#"sum(3, rate({app="foo"}[5m]))"#).unwrap_err();
        assert!(matches!(
            err,
            LowerError::UnexpectedParameter { op: "sum", .. },
        ));
    }

    #[test]
    fn binop_literal_plus_literal() {
        let m = expect_metric("1 + 2");
        match m {
            MetricPlan::Binary(b) => {
                assert_eq!(*b.lhs, MetricPlan::Literal(1.0));
                assert_eq!(*b.rhs, MetricPlan::Literal(2.0));
            }
            other => panic!("expected Binary, got {other:?}"),
        }
    }

    #[test]
    fn binop_nested_precedence_preserved() {
        // `1 + 2 * 3` → Add(1, Mul(2, 3))
        let m = expect_metric("1 + 2 * 3");
        match m {
            MetricPlan::Binary(outer) => {
                assert_eq!(*outer.lhs, MetricPlan::Literal(1.0));
                match *outer.rhs {
                    MetricPlan::Binary(inner) => {
                        assert_eq!(*inner.lhs, MetricPlan::Literal(2.0));
                        assert_eq!(*inner.rhs, MetricPlan::Literal(3.0));
                    }
                    other => panic!("expected Mul on rhs, got {other:?}"),
                }
            }
            other => panic!("expected Binary, got {other:?}"),
        }
    }

    #[test]
    fn label_replace_lowers() {
        let m = expect_metric(
            r#"label_replace(rate({app="foo"}[5m]), "dst", "$1", "src", "(.+)")"#,
        );
        match m {
            MetricPlan::LabelReplace(lr) => {
                assert_eq!(lr.dst_label, "dst");
                assert_eq!(lr.regex, "(.+)");
                assert!(matches!(*lr.inner, MetricPlan::RangeAgg(_)));
            }
            other => panic!("expected LabelReplace, got {other:?}"),
        }
    }

    #[test]
    fn vector_in_binop() {
        let m = expect_metric(r#"vector(0) + rate({a="b"}[5m])"#);
        match m {
            MetricPlan::Binary(b) => {
                assert_eq!(*b.lhs, MetricPlan::Vector(0.0));
                assert!(matches!(*b.rhs, MetricPlan::RangeAgg(_)));
            }
            other => panic!("expected Binary, got {other:?}"),
        }
    }

    // ---- error span coverage --------------------------------------

    #[test]
    fn error_carries_span() {
        // The span points at the failing AST node (`topk(...)` here).
        let err = lower_str(r#"topk(rate({app="foo"}[5m]))"#).unwrap_err();
        assert!(matches!(err, LowerError::MissingParameter { .. }));
        let span = err.span();
        // The whole topk call covers the entire input.
        assert_eq!(span.start, 0);
        assert!(span.end > 0);
    }

    #[test]
    fn error_message_includes_byte_range() {
        let err = lower_str(r#"sum(3, rate({app="foo"}[5m]))"#).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("byte ") && msg.contains("sum"),
            "expected byte-range + op name in: {msg}",
        );
    }

    #[test]
    fn quantile_out_of_range_displays_value() {
        let err = lower_str(r#"quantile_over_time(2, {app="foo"}[5m])"#).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("got 2"), "expected `got 2` in: {msg}");
    }

    // ---- LogInMetricPosition via manual AST -----------------------

    #[test]
    fn log_in_metric_position_via_manual_ast() {
        // The parser doesn't construct this (binop atoms come from
        // metric_expr only), so we hand-build the AST.
        use nlogql::ast::{
            BinaryExpr, BinaryModifier, BinaryOp, Expr, LiteralExpr, StreamSelector,
        };
        use nlogql::span::Span;

        let sel = Expr::Selector(StreamSelector {
            matchers: Vec::new(),
            span: Span::new(0, 2),
        });
        let lit = Expr::Literal(LiteralExpr {
            value: 1.0,
            span: Span::new(5, 6),
        });
        let bin = Expr::Binary(BinaryExpr {
            op: BinaryOp::Add,
            lhs: Box::new(sel),
            rhs: Box::new(lit),
            modifier: BinaryModifier::default(),
            span: Span::new(0, 6),
        });

        let err = lower(&bin).unwrap_err();
        assert!(matches!(err, LowerError::LogInMetricPosition { .. }));
        // Span points at the offending lhs (the selector).
        assert_eq!(err.span().start, 0);
        assert_eq!(err.span().end, 2);
    }

    // ---- LowerError Display sanity --------------------------------

    #[test]
    fn each_error_variant_displays() {
        // Exercise every variant's Display arm so changes there
        // don't break the format silently.
        let cases: &[&str] = &[
            r#"{a="b"} | line_format "x""#,                  // DeferredStage
            r#"quantile_over_time({a="b"}[5m])"#,            // MissingParameter
            r#"sum(3, rate({a="b"}[5m]))"#,                  // UnexpectedParameter
            r#"quantile_over_time(2, {a="b"}[5m])"#,         // QuantileOutOfRange
        ];
        for q in cases {
            let err = lower_str(q).unwrap_err();
            let msg = err.to_string();
            assert!(msg.starts_with("lower at byte "), "{q:?} -> {msg}");
        }
    }
}
