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
    TimestampNanosecondArray,
};
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::common::tree_node::Transformed;
use datafusion::common::{DFSchemaRef, DataFusionError, Result, ScalarValue};
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

use sfst::{FieldTier, Filter, Grid, IndexReader};

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
        if let Some(node) = try_build_facet_node(&plan) {
            return Ok(Transformed::yes(LogicalPlan::Extension(Extension {
                node: Arc::new(node),
            })));
        }
        if let Some(node) = try_build_timeline_node(&plan) {
            return Ok(Transformed::yes(LogicalPlan::Extension(Extension {
                node: Arc::new(node),
            })));
        }
        Ok(Transformed::no(plan))
    }
}

/// Peel an optional WHERE `Filter` off `input`, require a `TableScan` over an
/// `SfstTable`, and return the table plus the complete set of WHERE conjuncts
/// (gathered from the Filter node and any predicates already pushed into the
/// scan). The borrowed `&SfstTable` is valid for the returned `provider`'s
/// lifetime, so the caller keeps `provider` alive.
fn scan_table<'a>(
    input: &'a LogicalPlan,
    provider: &'a mut Option<Arc<dyn datafusion::catalog::TableProvider>>,
) -> Option<(&'a SfstTable, Vec<Expr>)> {
    let mut node = input;
    let mut predicates: Vec<Expr> = Vec::new();
    if let LogicalPlan::Filter(filter) = node {
        predicates.extend(split_conjunction(&filter.predicate).into_iter().cloned());
        node = filter.input.as_ref();
    }
    let LogicalPlan::TableScan(scan) = node else {
        return None;
    };
    for f in &scan.filters {
        predicates.extend(split_conjunction(f).into_iter().cloned());
    }
    *provider = Some(source_as_provider(&scan.source).ok()?);
    let table = (provider.as_ref().unwrap().as_ref() as &dyn Any).downcast_ref::<SfstTable>()?;
    Some((table, predicates))
}

/// Recognise the pushdown pattern and build the facet node, or `None` to fall
/// back to the normal plan.
fn try_build_facet_node(plan: &LogicalPlan) -> Option<SfstFacetNode> {
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

    let mut provider = None;
    let (table, predicates) = scan_table(input.as_ref(), &mut provider)?;
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

/// Largest grid this rule will build; beyond it (e.g. 1-second buckets over a
/// year) we fall back rather than allocate an enormous bucket vector.
const MAX_TIMELINE_BUCKETS: usize = 1_000_000;

/// Recognise `COUNT(*) GROUP BY date_bin(timestamp) [, <field>]` and build the
/// timeline node, or `None` to fall back.
fn try_build_timeline_node(plan: &LogicalPlan) -> Option<SfstTimelineNode> {
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
    let [agg] = aggr_expr.as_slice() else {
        return None;
    };
    if !is_count_star(agg) || group_expr.is_empty() || group_expr.len() > 2 {
        return None;
    }

    // Identify the single date_bin group expr and an optional scalar value
    // group expr, tracking their output-column positions (group columns come
    // first in the Aggregate schema, in group_expr order).
    let mut date_bin: Option<(usize, i64, i64)> = None; // (pos, stride_ns, origin_ns)
    let mut value: Option<(usize, String)> = None; // (pos, field)
    for (i, g) in group_expr.iter().enumerate() {
        if let Some((stride, origin)) = parse_date_bin(g) {
            if date_bin.is_some() {
                return None;
            }
            date_bin = Some((i, stride, origin));
        } else if let Some(name) = column_name(g) {
            if value.is_some() {
                return None;
            }
            value = Some((i, name));
        } else {
            return None;
        }
    }
    let (time_pos, stride_ns, origin_ns) = date_bin?;

    let mut provider = None;
    let (table, predicates) = scan_table(input.as_ref(), &mut provider)?;
    let sfst_schema = table.sfst_schema();

    // Optional value field: low/mid scalar only (timeline errors on high-card,
    // and a list column's grouping is not a per-value timeline).
    let value_field = match &value {
        Some((_, name)) => {
            let spec = sfst_schema.specs.iter().find(|s| &s.name == name)?;
            if spec.kind == ColKind::List || spec.tier == FieldTier::High {
                return None;
            }
            Some((name.clone(), spec.kind))
        }
        None => None,
    };

    // Translate WHERE: equality predicates (not on the value field, which
    // timeline excludes from its own histogram) + a time window clamping the grid.
    let mut eq_filter: Vec<(String, String)> = Vec::new();
    let mut lo = i64::MIN;
    let mut hi = i64::MAX;
    for pred in &predicates {
        match pushdown::classify(pred, sfst_schema) {
            Some(Pushable::Equals { field, value: v }) => {
                if value.as_ref().is_some_and(|(_, f)| f == &field) {
                    return None;
                }
                eq_filter.push((field, v));
            }
            Some(Pushable::TimeLo(v)) => lo = lo.max(v),
            Some(Pushable::TimeHi(v)) => hi = hi.min(v),
            None => return None,
        }
    }

    // Size the grid to the data range (clamped by the WHERE window) and align it
    // to date_bin's boundaries: bucket k starts at origin + k*stride.
    let (min_ts, max_ts) = table.ts_bounds();
    let range_start = min_ts.max(lo);
    let range_end = max_ts.saturating_add(1).min(hi);
    let (bucket_start_ns, num_buckets) = if range_end <= range_start {
        (origin_ns, 0)
    } else {
        let start = origin_ns + (range_start - origin_ns).div_euclid(stride_ns) * stride_ns;
        let n = (((range_end - start) as i128 + stride_ns as i128 - 1) / stride_ns as i128) as usize;
        (start, n)
    };
    if num_buckets > MAX_TIMELINE_BUCKETS {
        return None;
    }

    Some(SfstTimelineNode {
        data: table.data(),
        stride_ns,
        bucket_start_ns,
        num_buckets,
        value_field,
        eq_filter,
        time_pos,
        value_pos: value.as_ref().map(|(i, _)| *i),
        group_cols: group_expr.len(),
        schema: schema.clone(),
    })
}

/// Parse `date_bin(<interval>, <timestamp-col>[, <origin>])` over the timestamp
/// column into `(stride_ns, origin_ns)`. Rejects month-based (variable-width)
/// strides and any non-timestamp source.
fn parse_date_bin(expr: &Expr) -> Option<(i64, i64)> {
    let inner = match expr {
        Expr::Alias(a) => a.expr.as_ref(),
        e => e,
    };
    let Expr::ScalarFunction(sf) = inner else {
        return None;
    };
    if sf.func.name() != "date_bin" {
        return None;
    }
    if column_name(sf.args.get(1)?).as_deref() != Some(crate::schema::TS_COLUMN) {
        return None;
    }
    let stride_ns = match sf.args.first()? {
        Expr::Literal(ScalarValue::IntervalMonthDayNano(Some(v)), _) => {
            if v.months != 0 {
                return None; // calendar-month buckets are not fixed-width
            }
            (v.days as i64)
                .checked_mul(86_400_000_000_000)?
                .checked_add(v.nanoseconds)?
        }
        _ => return None,
    };
    if stride_ns <= 0 {
        return None;
    }
    let origin_ns = match sf.args.get(2) {
        None => 0,
        Some(Expr::Literal(ScalarValue::TimestampNanosecond(Some(o), _), _)) => *o,
        Some(_) => return None,
    };
    Some((stride_ns, origin_ns))
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
        if let Some(node) = node.as_any().downcast_ref::<SfstFacetNode>() {
            let arrow_schema: SchemaRef = Arc::new(node.schema.as_arrow().clone());
            return Ok(Some(Arc::new(SfstFacetExec::new(
                node.data.clone(),
                node.field.clone(),
                node.kind,
                node.eq_filter.clone(),
                node.lo..node.hi,
                arrow_schema,
            ))));
        }
        if let Some(node) = node.as_any().downcast_ref::<SfstTimelineNode>() {
            let arrow_schema: SchemaRef = Arc::new(node.schema.as_arrow().clone());
            return Ok(Some(Arc::new(SfstTimelineExec::new(
                node.data.clone(),
                node.stride_ns,
                node.bucket_start_ns,
                node.num_buckets,
                node.value_field.clone(),
                node.eq_filter.clone(),
                node.time_pos,
                node.value_pos,
                node.group_cols,
                arrow_schema,
            ))));
        }
        Ok(None)
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

// ── Timeline node + exec (date_bin GROUP BY) ────────────────────────────────

#[derive(Clone)]
struct SfstTimelineNode {
    data: Arc<Vec<u8>>,
    stride_ns: i64,
    bucket_start_ns: i64,
    num_buckets: usize,
    /// `Some((field, kind))` for the 2-D time×value grid; `None` for a plain
    /// per-bucket total.
    value_field: Option<(String, ColKind)>,
    eq_filter: Vec<(String, String)>,
    /// Output-column index of the `date_bin` (time) column.
    time_pos: usize,
    /// Output-column index of the value column (2-D only).
    value_pos: Option<usize>,
    /// Number of group columns (count column follows them).
    group_cols: usize,
    schema: DFSchemaRef,
}

impl SfstTimelineNode {
    fn value_name(&self) -> Option<&String> {
        self.value_field.as_ref().map(|(n, _)| n)
    }
}

impl fmt::Debug for SfstTimelineNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_for_explain(f)
    }
}

impl PartialEq for SfstTimelineNode {
    fn eq(&self, o: &Self) -> bool {
        Arc::ptr_eq(&self.data, &o.data)
            && self.stride_ns == o.stride_ns
            && self.bucket_start_ns == o.bucket_start_ns
            && self.num_buckets == o.num_buckets
            && self.value_name() == o.value_name()
            && self.eq_filter == o.eq_filter
            && self.time_pos == o.time_pos
            && self.value_pos == o.value_pos
            && self.group_cols == o.group_cols
    }
}
impl Eq for SfstTimelineNode {}

impl Hash for SfstTimelineNode {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.data) as *const () as usize).hash(state);
        self.stride_ns.hash(state);
        self.bucket_start_ns.hash(state);
        self.num_buckets.hash(state);
        self.value_name().hash(state);
        self.eq_filter.hash(state);
        self.time_pos.hash(state);
        self.value_pos.hash(state);
        self.group_cols.hash(state);
    }
}

impl PartialOrd for SfstTimelineNode {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        (
            Arc::as_ptr(&self.data) as *const () as usize,
            self.stride_ns,
            self.bucket_start_ns,
            self.num_buckets,
            self.value_name(),
            &self.eq_filter,
            self.time_pos,
            self.value_pos,
            self.group_cols,
        )
            .partial_cmp(&(
                Arc::as_ptr(&o.data) as *const () as usize,
                o.stride_ns,
                o.bucket_start_ns,
                o.num_buckets,
                o.value_name(),
                &o.eq_filter,
                o.time_pos,
                o.value_pos,
                o.group_cols,
            ))
    }
}

impl UserDefinedLogicalNodeCore for SfstTimelineNode {
    fn name(&self) -> &str {
        "SfstTimeline"
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
        match self.value_name() {
            Some(field) => write!(
                f,
                "SfstTimeline: date_bin({}ns) x \"{}\", count(*) via timeline bitmaps",
                self.stride_ns, field
            ),
            None => write!(
                f,
                "SfstTimeline: date_bin({}ns), count(*) via timeline bitmaps",
                self.stride_ns
            ),
        }
    }
    fn with_exprs_and_inputs(&self, _exprs: Vec<Expr>, _inputs: Vec<LogicalPlan>) -> Result<Self> {
        Ok(self.clone())
    }
}

struct SfstTimelineExec {
    data: Arc<Vec<u8>>,
    stride_ns: i64,
    bucket_start_ns: i64,
    num_buckets: usize,
    value_field: Option<(String, ColKind)>,
    eq_filter: Vec<(String, String)>,
    time_pos: usize,
    value_pos: Option<usize>,
    group_cols: usize,
    schema: SchemaRef,
    cache: Arc<PlanProperties>,
}

impl SfstTimelineExec {
    #[allow(clippy::too_many_arguments)]
    fn new(
        data: Arc<Vec<u8>>,
        stride_ns: i64,
        bucket_start_ns: i64,
        num_buckets: usize,
        value_field: Option<(String, ColKind)>,
        eq_filter: Vec<(String, String)>,
        time_pos: usize,
        value_pos: Option<usize>,
        group_cols: usize,
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
            stride_ns,
            bucket_start_ns,
            num_buckets,
            value_field,
            eq_filter,
            time_pos,
            value_pos,
            group_cols,
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
        let grid = Grid::new(self.bucket_start_ns, self.stride_ns, self.num_buckets);

        let mut times: Vec<i64> = Vec::new();
        let mut values: Vec<Option<String>> = Vec::new();
        let mut counts: Vec<i64> = Vec::new();

        match &self.value_field {
            Some((field, _)) => {
                // 2-D: emit one row per (bucket, value) with a positive count,
                // plus the absent-field (NULL) group from `unset`.
                let tl = reader
                    .timeline(field, &compiled, grid)
                    .map_err(exec_err)?;
                for (b, bucket) in tl.buckets.iter().enumerate() {
                    let ts = self.bucket_start_ns + b as i64 * self.stride_ns;
                    for (j, dim) in tl.dimensions.iter().enumerate() {
                        if bucket.counts[j] > 0 {
                            times.push(ts);
                            values.push(Some(dim.clone()));
                            counts.push(bucket.counts[j] as i64);
                        }
                    }
                    if bucket.unset > 0 {
                        times.push(ts);
                        values.push(None);
                        counts.push(bucket.unset as i64);
                    }
                }
            }
            None => {
                // 1-D: one row per non-empty bucket.
                let totals = reader.timeline_totals(&compiled, grid).map_err(exec_err)?;
                for (b, total) in totals.iter().enumerate() {
                    if *total > 0 {
                        times.push(self.bucket_start_ns + b as i64 * self.stride_ns);
                        counts.push(*total as i64);
                    }
                }
            }
        }

        // Assemble columns in the Aggregate's output order.
        let mut cols: Vec<Option<ArrayRef>> = (0..=self.group_cols).map(|_| None).collect();
        cols[self.time_pos] = Some(Arc::new(TimestampNanosecondArray::from(times)) as ArrayRef);
        cols[self.group_cols] = Some(Arc::new(Int64Array::from(counts)) as ArrayRef);
        if let (Some(vpos), Some((_, kind))) = (self.value_pos, &self.value_field) {
            cols[vpos] = Some(build_opt_column(*kind, &values));
        }
        let columns: Vec<ArrayRef> = cols.into_iter().map(Option::unwrap).collect();
        RecordBatch::try_new(self.schema.clone(), columns).map_err(Into::into)
    }
}

/// Build a typed value column from optional string values (NULL = absent-field
/// group). Mirrors the group-key typing used elsewhere.
fn build_opt_column(kind: ColKind, values: &[Option<String>]) -> ArrayRef {
    match kind {
        ColKind::Int => {
            let mut b = Int64Builder::new();
            for v in values {
                b.append_option(v.as_ref().and_then(|s| s.parse::<i64>().ok()));
            }
            Arc::new(b.finish())
        }
        ColKind::Double => {
            let mut b = Float64Builder::new();
            for v in values {
                b.append_option(v.as_ref().and_then(|s| s.parse::<f64>().ok()));
            }
            Arc::new(b.finish())
        }
        ColKind::Bool => {
            let mut b = BooleanBuilder::new();
            for v in values {
                b.append_option(v.as_ref().and_then(|s| s.parse::<bool>().ok()));
            }
            Arc::new(b.finish())
        }
        // Str, and List (excluded from pushdown) → Utf8.
        _ => {
            let mut b = StringBuilder::new();
            for v in values {
                match v {
                    Some(s) => b.append_value(s),
                    None => b.append_null(),
                }
            }
            Arc::new(b.finish())
        }
    }
}

impl fmt::Debug for SfstTimelineExec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SfstTimelineExec")
    }
}

impl DisplayAs for SfstTimelineExec {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SfstTimelineExec: {} bucket(s) (timeline bitmaps)", self.num_buckets)
    }
}

impl ExecutionPlan for SfstTimelineExec {
    fn name(&self) -> &str {
        "SfstTimelineExec"
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
