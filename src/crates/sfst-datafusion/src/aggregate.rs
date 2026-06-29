//! Aggregation pushdown (Stage D): answer `SELECT <field>, COUNT(*) FROM logs
//! [WHERE …] GROUP BY <field>` from SFST's precomputed facet bitmaps instead of
//! scanning rows.
//!
//! Pipeline: an [`OptimizerRule`] recognises the exact pattern over an
//! [`SfstTable`] and rewrites it to a leaf `Extension(SfstFacetNode)`; an
//! [`ExtensionPlanner`] turns that into [`SfstFacetExec`], which calls
//! `IndexReader::facets`. Anything that doesn't match — high-card or list group
//! column, a non-`COUNT(*)` aggregate, an untranslatable `WHERE`, or a `WHERE`
//! that constrains the group column — is left untouched and runs the normal
//! (already-fast) plan. Correctness via fallback is absolute.

use std::any::Any;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use datafusion::arrow::array::{
    ArrayRef, BooleanBuilder, Float64Builder, Int64Array, Int64Builder, RecordBatch, StringBuilder,
};
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::common::tree_node::Transformed;
use datafusion::common::{DFSchemaRef, DataFusionError, Result};
use datafusion::datasource::source_as_provider;
use datafusion::execution::context::QueryPlanner;
use datafusion::execution::{SessionState, TaskContext};
use datafusion::logical_expr::expr::AggregateFunction;
use datafusion::logical_expr::utils::split_conjunction;
use datafusion::logical_expr::{
    Aggregate, Expr, Extension, LogicalPlan, UserDefinedLogicalNodeCore,
};
use datafusion::optimizer::{ApplyOrder, OptimizerConfig, OptimizerRule};
use datafusion::physical_expr::EquivalenceProperties;
use datafusion::physical_plan::execution_plan::{Boundedness, EmissionType};
use datafusion::physical_plan::memory::MemoryStream;
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
    SendableRecordBatchStream,
};
use datafusion::physical_planner::{
    DefaultPhysicalPlanner, ExtensionPlanner, PhysicalPlanner,
};

use sfst::{Filter, FieldTier, IndexReader};

use crate::pushdown::{self, Pushable};
use crate::schema::ColKind;
use crate::table::SfstTable;

fn exec_err(e: impl fmt::Display) -> DataFusionError {
    DataFusionError::Execution(format!("sfst: {e}"))
}

// ── Optimizer rule ──────────────────────────────────────────────────────────

/// Rewrites an eligible `COUNT(*) GROUP BY <field>` over an `SfstTable` into a
/// facet-bitmap plan; leaves everything else untouched.
#[derive(Debug, Default)]
pub struct SfstFacetRule;

impl OptimizerRule for SfstFacetRule {
    fn name(&self) -> &str {
        "sfst_facet_pushdown"
    }

    /// Visit every node so the rule sees the `Aggregate` (not just the plan
    /// root, which is usually a `Projection`).
    fn apply_order(&self) -> Option<ApplyOrder> {
        Some(ApplyOrder::BottomUp)
    }

    fn rewrite(
        &self,
        plan: LogicalPlan,
        _config: &dyn OptimizerConfig,
    ) -> Result<Transformed<LogicalPlan>> {
        match try_build_node(&plan) {
            Some(node) => Ok(Transformed::yes(LogicalPlan::Extension(Extension {
                node: Arc::new(node),
            }))),
            None => Ok(Transformed::no(plan)),
        }
    }
}

/// Recognise the pushdown pattern and build the facet node, or `None` to fall
/// back to the normal plan.
fn try_build_node(plan: &LogicalPlan) -> Option<SfstFacetNode> {
    let LogicalPlan::Aggregate(Aggregate {
        input,
        group_expr,
        aggr_expr,
        schema,
        ..
    }) = plan
    else {
        return None;
    };

    // Exactly one group column and exactly one COUNT(*) aggregate.
    let [group] = group_expr.as_slice() else {
        return None;
    };
    let [agg] = aggr_expr.as_slice() else {
        return None;
    };
    let group_field = column_name(group)?;
    if !is_count_star(agg) {
        return None;
    }

    // Peel an optional WHERE Filter, then require a TableScan over an SfstTable.
    // Predicates can live on the Filter node and/or already pushed into the
    // scan; gather both so translation sees the complete WHERE.
    let mut node_input = input.as_ref();
    let mut predicates: Vec<Expr> = Vec::new();
    if let LogicalPlan::Filter(filter) = node_input {
        predicates.extend(split_conjunction(&filter.predicate).into_iter().cloned());
        node_input = filter.input.as_ref();
    }
    let LogicalPlan::TableScan(scan) = node_input else {
        return None;
    };
    for f in &scan.filters {
        predicates.extend(split_conjunction(f).into_iter().cloned());
    }

    let provider = source_as_provider(&scan.source).ok()?;
    let table = (provider.as_ref() as &dyn Any).downcast_ref::<SfstTable>()?;
    let sfst_schema = table.sfst_schema();

    // The group column must be a low/mid scalar attribute: `facets()` errors on
    // high-card fields, and a list column's GROUP BY is not a per-value facet.
    let spec = sfst_schema.specs.iter().find(|s| s.name == group_field)?;
    if spec.kind == ColKind::List || spec.tier == FieldTier::High {
        return None;
    }

    // Every WHERE conjunct must translate to an SFST predicate, and none may
    // constrain the group column (facets exclude their own selection).
    let mut eq_filter: Vec<(String, String)> = Vec::new();
    let mut lo = i64::MIN;
    let mut hi = i64::MAX;
    for pred in &predicates {
        match pushdown::classify(pred, sfst_schema) {
            Some(Pushable::Equals { field, value }) => {
                if field == group_field {
                    return None;
                }
                eq_filter.push((field, value));
            }
            Some(Pushable::TimeLo(v)) => lo = lo.max(v),
            Some(Pushable::TimeHi(v)) => hi = hi.min(v),
            None => return None,
        }
    }

    Some(SfstFacetNode {
        data: table.data(),
        field: group_field,
        kind: spec.kind,
        eq_filter,
        lo,
        hi,
        schema: schema.clone(),
    })
}

/// The bare column name of a group expression (unwrapping an alias), or `None`.
fn column_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Column(c) => Some(c.name.clone()),
        Expr::Alias(a) => column_name(&a.expr),
        _ => None,
    }
}

/// Whether `expr` is `COUNT(*)` — `count` over no column (empty args or a single
/// literal), not distinct, unfiltered.
fn is_count_star(expr: &Expr) -> bool {
    let inner = match expr {
        Expr::Alias(a) => a.expr.as_ref(),
        other => other,
    };
    let Expr::AggregateFunction(AggregateFunction { func, params }) = inner else {
        return false;
    };
    func.name() == "count"
        && !params.distinct
        && params.filter.is_none()
        && match params.args.as_slice() {
            [] => true,
            [arg] => matches!(arg, Expr::Literal(_, _)),
            _ => false,
        }
}

// ── Logical node ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct SfstFacetNode {
    data: Arc<Vec<u8>>,
    field: String,
    kind: ColKind,
    eq_filter: Vec<(String, String)>,
    lo: i64,
    hi: i64,
    /// Output schema — exactly the rewritten Aggregate's schema, so the rest of
    /// the plan sees identical column names and types.
    schema: DFSchemaRef,
}

impl fmt::Debug for SfstFacetNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_for_explain(f)
    }
}

impl PartialEq for SfstFacetNode {
    fn eq(&self, o: &Self) -> bool {
        Arc::ptr_eq(&self.data, &o.data)
            && self.field == o.field
            && self.kind == o.kind
            && self.eq_filter == o.eq_filter
            && self.lo == o.lo
            && self.hi == o.hi
    }
}
impl Eq for SfstFacetNode {}

impl Hash for SfstFacetNode {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.data) as *const () as usize).hash(state);
        self.field.hash(state);
        self.eq_filter.hash(state);
        self.lo.hash(state);
        self.hi.hash(state);
    }
}

// `UserDefinedLogicalNodeCore` requires `PartialOrd`; order by the same fields
// `Eq`/`Hash` use (the data pointer stands in for table identity).
impl PartialOrd for SfstFacetNode {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        (
            Arc::as_ptr(&self.data) as *const () as usize,
            &self.field,
            &self.eq_filter,
            self.lo,
            self.hi,
        )
            .partial_cmp(&(
                Arc::as_ptr(&o.data) as *const () as usize,
                &o.field,
                &o.eq_filter,
                o.lo,
                o.hi,
            ))
    }
}

impl UserDefinedLogicalNodeCore for SfstFacetNode {
    fn name(&self) -> &str {
        "SfstFacet"
    }

    fn inputs(&self) -> Vec<&LogicalPlan> {
        vec![]
    }

    fn schema(&self) -> &DFSchemaRef {
        &self.schema
    }

    fn expressions(&self) -> Vec<Expr> {
        vec![]
    }

    fn fmt_for_explain(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "SfstFacet: group=\"{}\", count(*) via facet bitmaps",
            self.field
        )
    }

    fn with_exprs_and_inputs(
        &self,
        _exprs: Vec<Expr>,
        _inputs: Vec<LogicalPlan>,
    ) -> Result<Self> {
        // Leaf node with everything internalised — nothing to rebuild.
        Ok(self.clone())
    }
}

// ── Physical planner + exec ─────────────────────────────────────────────────

/// Turns an `SfstFacetNode` into `SfstFacetExec`.
#[derive(Debug)]
struct SfstExtensionPlanner;

#[async_trait::async_trait]
impl ExtensionPlanner for SfstExtensionPlanner {
    async fn plan_extension(
        &self,
        _planner: &dyn PhysicalPlanner,
        node: &dyn datafusion::logical_expr::UserDefinedLogicalNode,
        _logical_inputs: &[&LogicalPlan],
        _physical_inputs: &[Arc<dyn ExecutionPlan>],
        _session: &SessionState,
    ) -> Result<Option<Arc<dyn ExecutionPlan>>> {
        let Some(node) = node.as_any().downcast_ref::<SfstFacetNode>() else {
            return Ok(None);
        };
        let arrow_schema: SchemaRef = Arc::new(node.schema.as_arrow().clone());
        Ok(Some(Arc::new(SfstFacetExec::new(
            node.data.clone(),
            node.field.clone(),
            node.kind,
            node.eq_filter.clone(),
            node.lo..node.hi,
            arrow_schema,
        ))))
    }
}

struct SfstFacetExec {
    data: Arc<Vec<u8>>,
    field: String,
    kind: ColKind,
    eq_filter: Vec<(String, String)>,
    window: std::ops::Range<i64>,
    schema: SchemaRef,
    cache: Arc<PlanProperties>,
}

impl SfstFacetExec {
    fn new(
        data: Arc<Vec<u8>>,
        field: String,
        kind: ColKind,
        eq_filter: Vec<(String, String)>,
        window: std::ops::Range<i64>,
        schema: SchemaRef,
    ) -> Self {
        let cache = Arc::new(PlanProperties::new(
            EquivalenceProperties::new(schema.clone()),
            Partitioning::UnknownPartitioning(1),
            EmissionType::Incremental,
            Boundedness::Bounded,
        ));
        Self {
            data,
            field,
            kind,
            eq_filter,
            window,
            schema,
            cache,
        }
    }

    fn build_batch(&self) -> Result<RecordBatch> {
        let reader = IndexReader::open(&self.data).map_err(exec_err)?;

        let mut filter = Filter::new();
        for (k, v) in &self.eq_filter {
            filter = filter.select(k.clone(), v.clone());
        }
        let compiled = reader.compile_filter(&filter, None).map_err(exec_err)?;

        let facets = reader
            .facets(&[self.field.as_str()], &compiled, self.window.clone())
            .map_err(exec_err)?;
        let values = &facets[0].values; // exactly one requested field

        // Rows where the field is absent form SQL's NULL group. For a scalar
        // field each present row contributes one facet count, so the absent
        // count is the matched total minus the facet sum (exact; not valid for
        // multi-valued fields, which are excluded from pushdown).
        let matched = reader
            .matched_count(&compiled, self.window.clone())
            .map_err(exec_err)?;
        let present: u64 = values.iter().map(|(_, c)| *c as u64).sum();
        let null_count = matched.saturating_sub(present);

        // Group-key column (typed per the field) + the COUNT(*) column (Int64).
        let group = build_group_column(self.kind, values, null_count > 0);
        let mut counts: Vec<i64> = values.iter().map(|(_, c)| *c as i64).collect();
        if null_count > 0 {
            counts.push(null_count as i64);
        }
        let count_col: ArrayRef = Arc::new(Int64Array::from(counts));

        RecordBatch::try_new(self.schema.clone(), vec![group, count_col]).map_err(Into::into)
    }
}

/// Build the group-key array from facet `(value, count)` pairs, appending a
/// trailing NULL key when there is an absent-field group.
fn build_group_column(kind: ColKind, values: &[(String, u32)], null_group: bool) -> ArrayRef {
    match kind {
        ColKind::Str => {
            let mut b = StringBuilder::new();
            for (v, _) in values {
                b.append_value(v);
            }
            if null_group {
                b.append_null();
            }
            Arc::new(b.finish())
        }
        ColKind::Int => {
            let mut b = Int64Builder::new();
            for (v, _) in values {
                b.append_option(v.parse::<i64>().ok());
            }
            if null_group {
                b.append_null();
            }
            Arc::new(b.finish())
        }
        ColKind::Double => {
            let mut b = Float64Builder::new();
            for (v, _) in values {
                b.append_option(v.parse::<f64>().ok());
            }
            if null_group {
                b.append_null();
            }
            Arc::new(b.finish())
        }
        ColKind::Bool => {
            let mut b = BooleanBuilder::new();
            for (v, _) in values {
                b.append_option(v.parse::<bool>().ok());
            }
            if null_group {
                b.append_null();
            }
            Arc::new(b.finish())
        }
        // List is excluded from pushdown; never reached.
        ColKind::List => {
            let mut b = StringBuilder::new();
            if null_group {
                b.append_null();
            }
            Arc::new(b.finish())
        }
    }
}

impl fmt::Debug for SfstFacetExec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SfstFacetExec")
    }
}

impl DisplayAs for SfstFacetExec {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SfstFacetExec: group=\"{}\" (facet bitmaps)", self.field)
    }
}

impl ExecutionPlan for SfstFacetExec {
    fn name(&self) -> &str {
        "SfstFacetExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.cache
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![]
    }

    fn with_new_children(
        self: Arc<Self>,
        _children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        Ok(self)
    }

    fn execute(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let batch = self.build_batch()?;
        Ok(Box::pin(MemoryStream::try_new(
            vec![batch],
            self.schema.clone(),
            None,
        )?))
    }
}

// ── Query planner (injects the extension planner) ─────────────────────────────

/// A `QueryPlanner` that adds [`SfstExtensionPlanner`] to the default planner so
/// `SfstFacetNode` becomes `SfstFacetExec`.
#[derive(Debug)]
pub struct SfstQueryPlanner;

#[async_trait::async_trait]
impl QueryPlanner for SfstQueryPlanner {
    async fn create_physical_plan(
        &self,
        logical_plan: &LogicalPlan,
        session_state: &SessionState,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let planner =
            DefaultPhysicalPlanner::with_extension_planners(vec![Arc::new(SfstExtensionPlanner)]);
        planner
            .create_physical_plan(logical_plan, session_state)
            .await
    }
}
