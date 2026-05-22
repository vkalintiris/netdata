//! AST → Plan lowering.
//!
//! Translates an [`nlogql::ast::Expr`] into a [`crate::plan::Plan`],
//! performing the semantic checks that the parser intentionally
//! defers (see `EXPECTED_FAILS.md` in the `nlogql` crate):
//!
//! - `topk` / `bottomk` / `approx_topk` require a numeric first arg.
//! - Other vector ops reject a numeric first arg.
//! - `quantile_over_time` requires its quantile in `[0, 1]`.
//!
//! Today only the log path is lowered (SOW-D2). Metric expressions
//! return `LowerError::Unimplemented` until SOW-D3 lands.

use nlogql::ast::{Expr, PipelineExpr, PipelineStage};

use crate::error::LowerError;
use crate::plan::{LogPlan, LogStage, Plan};

/// Lower a parsed LogQL AST into an executable [`Plan`].
pub fn lower(expr: &Expr) -> Result<Plan, LowerError> {
    match expr {
        Expr::Selector(s) => Ok(Plan::Log(LogPlan {
            matchers: s.matchers.clone(),
            stages: Vec::new(),
        })),
        Expr::Pipeline(p) => lower_pipeline(p).map(Plan::Log),
        Expr::RangeAggregation(_)
        | Expr::VectorAggregation(_)
        | Expr::Binary(_)
        | Expr::Literal(_)
        | Expr::LabelReplace(_)
        | Expr::Vector(_) => Err(LowerError::Unimplemented {
            what: "metric path lowering (SOW-D3)",
        }),
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
        // The two template stages need a Go-template engine. Per
        // the evaluator plan they're deferred to a follow-up plan;
        // for now we reject at lower time so the Plan type never
        // carries something we can't execute.
        PipelineStage::LineFormat(_) => Err(LowerError::DeferredStage {
            stage: "line_format",
        }),
        PipelineStage::LabelFormat(_) => Err(LowerError::DeferredStage {
            stage: "label_format",
        }),
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
        }
    }

    #[test]
    fn bare_selector() {
        let p = expect_log(r#"{app="foo"}"#);
        assert_eq!(p.matchers.len(), 1);
        assert!(p.stages.is_empty());
    }

    #[test]
    fn empty_selector() {
        let p = expect_log("{}");
        assert!(p.matchers.is_empty());
        assert!(p.stages.is_empty());
    }

    #[test]
    fn selector_with_line_filter() {
        let p = expect_log(r#"{app="foo"} |= "error""#);
        assert_eq!(p.stages.len(), 1);
        assert!(matches!(p.stages[0], LogStage::LineFilter(_)));
    }

    #[test]
    fn multi_stage_pipeline() {
        let p = expect_log(
            r#"{app="foo"} |= "x" | logfmt | latency > 1s | decolorize | drop trace_id"#,
        );
        assert_eq!(p.stages.len(), 5);
        assert!(matches!(p.stages[0], LogStage::LineFilter(_)));
        assert!(matches!(p.stages[1], LogStage::Parser(_)));
        assert!(matches!(p.stages[2], LogStage::LabelFilter(_)));
        assert!(matches!(p.stages[3], LogStage::Decolorize));
        assert!(matches!(p.stages[4], LogStage::DropLabels(_)));
    }

    #[test]
    fn keep_labels_lowers() {
        let p = expect_log(r#"{app="foo"} | logfmt | keep status, latency"#);
        assert!(matches!(p.stages.last(), Some(LogStage::KeepLabels(_))));
    }

    #[test]
    fn line_format_is_deferred() {
        let err = lower_str(r#"{app="foo"} | line_format "{{.x}}""#).unwrap_err();
        match err {
            LowerError::DeferredStage { stage } => assert_eq!(stage, "line_format"),
            other => panic!("expected DeferredStage, got {other:?}"),
        }
    }

    #[test]
    fn label_format_is_deferred() {
        let err = lower_str(r#"{app="foo"} | label_format new=old"#).unwrap_err();
        match err {
            LowerError::DeferredStage { stage } => assert_eq!(stage, "label_format"),
            other => panic!("expected DeferredStage, got {other:?}"),
        }
    }

    #[test]
    fn metric_query_unimplemented_for_now() {
        // SOW-D3 will lower these; today they're rejected.
        let err = lower_str(r#"rate({app="foo"}[5m])"#).unwrap_err();
        assert!(matches!(err, LowerError::Unimplemented { .. }));

        let err = lower_str(r#"sum(rate({app="foo"}[5m]))"#).unwrap_err();
        assert!(matches!(err, LowerError::Unimplemented { .. }));

        let err = lower_str(r#"1 + 2"#).unwrap_err();
        assert!(matches!(err, LowerError::Unimplemented { .. }));
    }
}
