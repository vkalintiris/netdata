//! SFST → Arrow schema mapping (Stage A, decision D4: flat dotted-path columns).
//!
//! One Arrow column per derived field path, plus a leading `timestamp` column
//! (the frozen per-record `Record.ts`, which carries the table's time order).
//! Multi-valued (`[]`) paths become `List<Utf8>`; scalar types come from the
//! typed `SchemaTree`. See the SOW for the recorded forks.

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use sfst::{IndexReader, ValueKind};

/// The synthetic timestamp column. SFST rows are stored time-sorted, so this
/// column is the table's output ordering (advertised to DataFusion later).
pub const TS_COLUMN: &str = "timestamp";

/// How an attribute column's string values pivot into a typed Arrow array.
///
/// Multi-valued (`[]`) paths are `List` regardless of element kind — values
/// arrive from `materialize_rows` as strings and typed list elements are a
/// later-stage refinement (D4). `Bytes` coalesces to `Str` for the same
/// reason: avoid guessing a byte encoding in Stage A.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColKind {
    Bool,
    Int,
    Double,
    Str,
    List,
}

impl ColKind {
    fn arrow_type(self) -> DataType {
        match self {
            ColKind::Bool => DataType::Boolean,
            ColKind::Int => DataType::Int64,
            ColKind::Double => DataType::Float64,
            ColKind::Str => DataType::Utf8,
            ColKind::List => DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
        }
    }
}

/// One attribute column. `name` matches the interned `key=` byte-for-byte
/// (including any `[]` array marker), so the pivot keys directly off it.
#[derive(Clone, Debug)]
pub struct ColumnSpec {
    pub name: String,
    pub kind: ColKind,
}

/// The table's Arrow schema plus the per-attribute pivot specs. Schema field 0
/// is `timestamp` (no spec); field `i + 1` is described by `specs[i]`.
pub struct SfstSchema {
    pub schema: SchemaRef,
    pub specs: Vec<ColumnSpec>,
}

impl SfstSchema {
    pub fn build(reader: &IndexReader) -> Self {
        // path -> coalesced scalar kind (containers / Null are absent here).
        let kinds: HashMap<String, ValueKind> =
            reader.tree().derive_scalar_kinds().into_iter().collect();

        let mut fields = Vec::new();
        fields.push(Field::new(
            TS_COLUMN,
            DataType::Timestamp(TimeUnit::Nanosecond, None),
            false,
        ));

        let mut specs = Vec::new();
        for fe in reader.field_table().iter() {
            // A `[]` path is array-collapsed → multiple values per row → List.
            let kind = if fe.name.contains("[]") {
                ColKind::List
            } else {
                match kinds.get(&fe.name).copied() {
                    Some(ValueKind::Bool) => ColKind::Bool,
                    Some(ValueKind::Int) => ColKind::Int,
                    Some(ValueKind::Double) => ColKind::Double,
                    // Str, Bytes, or a path with no coalesced scalar → Utf8.
                    _ => ColKind::Str,
                }
            };
            // Attributes are sparse across rows → nullable.
            fields.push(Field::new(&fe.name, kind.arrow_type(), true));
            specs.push(ColumnSpec {
                name: fe.name.clone(),
                kind,
            });
        }

        SfstSchema {
            schema: Arc::new(Schema::new(fields)),
            specs,
        }
    }
}
