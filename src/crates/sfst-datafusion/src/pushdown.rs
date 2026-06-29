//! Predicate pushdown (Stage A, decision D5).
//!
//! We push two predicate shapes into the SFST index, both of which map exactly
//! to its semantics:
//!
//! - `<str-field> = '<literal>'` — exact term match via `Filter::select`.
//! - `timestamp <cmp> <literal>` — a half-open time window clip.
//!
//! Everything else stays with DataFusion (`Unsupported`). A field carrying two
//! equality predicates is reported `Inexact` (our index ORs same-field values,
//! but SQL ANDs them), so DataFusion re-checks and correctness holds.

use std::collections::HashMap;
use std::ops::Range;

use datafusion::common::ScalarValue;
use datafusion::logical_expr::{Expr, Operator, TableProviderFilterPushDown};

use crate::schema::{ColKind, SfstSchema, TS_COLUMN};

/// A single predicate we can hand to the SFST index.
pub enum Pushable {
    /// `field = value`, exact string match on a scalar Str column.
    Equals { field: String, value: String },
    /// Window lower bound (inclusive nanoseconds).
    TimeLo(i64),
    /// Window upper bound (exclusive nanoseconds).
    TimeHi(i64),
}

fn column_name(e: &Expr) -> Option<&str> {
    match e {
        Expr::Column(c) => Some(c.name.as_str()),
        _ => None,
    }
}

fn scalar(e: &Expr) -> Option<&ScalarValue> {
    match e {
        Expr::Literal(s, _) => Some(s),
        _ => None,
    }
}

/// Mirror a comparison operator when operands are swapped (`lit < col`).
fn flip(op: Operator) -> Option<Operator> {
    Some(match op {
        Operator::Gt => Operator::Lt,
        Operator::GtEq => Operator::LtEq,
        Operator::Lt => Operator::Gt,
        Operator::LtEq => Operator::GtEq,
        Operator::Eq => Operator::Eq,
        _ => return None,
    })
}

fn scalar_to_ns(s: &ScalarValue) -> Option<i64> {
    match s {
        ScalarValue::TimestampNanosecond(Some(v), _) => Some(*v),
        ScalarValue::TimestampMicrosecond(Some(v), _) => v.checked_mul(1_000),
        ScalarValue::TimestampMillisecond(Some(v), _) => v.checked_mul(1_000_000),
        ScalarValue::TimestampSecond(Some(v), _) => v.checked_mul(1_000_000_000),
        ScalarValue::Int64(Some(v)) => Some(*v),
        _ => None,
    }
}

fn scalar_to_utf8(s: &ScalarValue) -> Option<String> {
    match s {
        ScalarValue::Utf8(Some(v))
        | ScalarValue::LargeUtf8(Some(v))
        | ScalarValue::Utf8View(Some(v)) => Some(v.clone()),
        _ => None,
    }
}

/// Classify one predicate; `Some` means it maps to the SFST index.
pub fn classify(expr: &Expr, schema: &SfstSchema) -> Option<Pushable> {
    let Expr::BinaryExpr(be) = expr else {
        return None;
    };

    // Normalise to (column, op, literal), accepting either operand order.
    let (col, op, lit) = match (column_name(&be.left), scalar(&be.right)) {
        (Some(c), Some(l)) => (c, be.op, l),
        _ => match (scalar(&be.left), column_name(&be.right)) {
            (Some(l), Some(c)) => (c, flip(be.op)?, l),
            _ => return None,
        },
    };

    if col == TS_COLUMN {
        let v = scalar_to_ns(lit)?;
        return match op {
            Operator::GtEq => Some(Pushable::TimeLo(v)),
            Operator::Gt => Some(Pushable::TimeLo(v.checked_add(1)?)),
            Operator::Lt => Some(Pushable::TimeHi(v)),
            Operator::LtEq => Some(Pushable::TimeHi(v.checked_add(1)?)),
            _ => None,
        };
    }

    if op == Operator::Eq {
        // Only scalar Str attribute columns: typed/list columns risk a
        // semantic mismatch and stay with DataFusion (D5).
        let spec = schema.specs.iter().find(|s| s.name == col)?;
        if spec.kind == ColKind::Str {
            let value = scalar_to_utf8(lit)?;
            return Some(Pushable::Equals {
                field: col.to_string(),
                value,
            });
        }
    }

    None
}

/// Per-filter pushdown verdict. Equality on a field used by more than one
/// equality predicate is downgraded to `Inexact` (see module docs).
pub fn verdicts(filters: &[&Expr], schema: &SfstSchema) -> Vec<TableProviderFilterPushDown> {
    let classified: Vec<Option<Pushable>> = filters.iter().map(|f| classify(f, schema)).collect();

    let mut eq_counts: HashMap<&str, usize> = HashMap::new();
    for c in &classified {
        if let Some(Pushable::Equals { field, .. }) = c {
            *eq_counts.entry(field.as_str()).or_default() += 1;
        }
    }

    classified
        .iter()
        .map(|c| match c {
            None => TableProviderFilterPushDown::Unsupported,
            Some(Pushable::Equals { field, .. }) if eq_counts[field.as_str()] > 1 => {
                TableProviderFilterPushDown::Inexact
            }
            Some(_) => TableProviderFilterPushDown::Exact,
        })
        .collect()
}

/// Fold the pushable predicates DataFusion handed to `scan` into an SFST filter
/// plus a half-open time window. Same-field equalities OR together; time bounds
/// intersect (`max` lower, `min` upper).
pub fn plan(filters: &[Expr], schema: &SfstSchema) -> (sfst::Filter, Range<i64>) {
    let mut filter = sfst::Filter::new();
    let mut lo = i64::MIN;
    let mut hi = i64::MAX;

    for f in filters {
        match classify(f, schema) {
            Some(Pushable::Equals { field, value }) => filter = filter.select(field, value),
            Some(Pushable::TimeLo(v)) => lo = lo.max(v),
            Some(Pushable::TimeHi(v)) => hi = hi.min(v),
            None => {}
        }
    }

    (filter, lo..hi)
}
