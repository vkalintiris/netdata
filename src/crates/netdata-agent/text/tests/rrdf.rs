//! A Function table's column (`buffer_rrdf_table_add_field()`) against the C golden vectors: every enum, sort and
//! option, NaN and real maxima, NULL and empty strings, minified and pretty.

mod common;

use common::{Row, check, text};
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::rrdf::{self, Field, FieldType, Filter, Summary, Transform, Visual};

const TYPES: [FieldType; 9] = [
    FieldType::None,
    FieldType::Integer,
    FieldType::Boolean,
    FieldType::String,
    FieldType::DetailString,
    FieldType::BarWithInteger,
    FieldType::Duration,
    FieldType::Timestamp,
    FieldType::Array,
];
const VISUALS: [Visual; 5] = [Visual::Value, Visual::Bar, Visual::Pill, Visual::Rich, Visual::RowOptions];
const TRANSFORMS: [Transform; 6] = [
    Transform::None,
    Transform::Number,
    Transform::DurationS,
    Transform::DatetimeMs,
    Transform::DatetimeUsec,
    Transform::Xml,
];
const SUMMARIES: [Summary; 7] =
    [Summary::UniqueCount, Summary::Sum, Summary::Min, Summary::Max, Summary::Mean, Summary::Median, Summary::Count];
const FILTERS: [Filter; 4] = [Filter::None, Filter::Range, Filter::MultiSelect, Filter::Facet];

/// A C string field: kind 0 is NULL.
fn opt(row: &Row, kind: usize) -> Option<&[u8]> {
    row.flag(kind).then(|| row.bytes(kind + 1))
}

fn render(row: &Row) -> String {
    let options = if row.flag(0) { JsonOptions::MINIFY } else { JsonOptions::DEFAULT };
    let mut w = JsonWriter::new(options);
    let field = Field {
        id: row.num(1),
        key: row.bytes(2),
        name: row.bytes(3),
        kind: TYPES[row.num::<usize>(4)],
        visual: VISUALS[row.num::<usize>(5)],
        transform: TRANSFORMS[row.num::<usize>(6)],
        decimal_points: row.num(7),
        units: opt(row, 8),
        max: f64::from_bits(row.bits(10)),
        sort: row.num(11),
        pointer_to: opt(row, 12),
        summary: SUMMARIES[row.num::<usize>(14)],
        filter: FILTERS[row.num::<usize>(15)],
        options: row.num(16),
        default_value: opt(row, 17),
    };
    rrdf::add_field(&mut w, &field);
    w.finalize();
    text(w.as_bytes())
}

#[test]
fn rrdf_fields_are_cs() {
    let checked = check("rrdf.tsv", |row| text(row.bytes(19)), render);
    assert_eq!(checked, 600);
}
