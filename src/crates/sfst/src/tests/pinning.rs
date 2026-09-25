//! Pinned fields: a producer may pin fields so they are never High tier,
//! which keeps their facets and timelines available at any cardinality.

use bumpalo::Bump;

use crate::{
    DEFAULT_CARDINALITY_THRESHOLD, FieldTier, Filter, Grid, IndexReader, IndexWriter, RowIndex,
};

const ROWS: usize = 3_000;

/// `ROWS` rows, one second apart, each with `name` and `id` taking the
/// row's index modulo `values`; `pins` are pinned before the build.
fn file(values: usize, pins: &[&str]) -> Vec<u8> {
    let arena = Bump::new();
    let mut rows = RowIndex::new(&arena, DEFAULT_CARDINALITY_THRESHOLD);
    rows.pin_fields(pins);
    for i in 0..ROWS {
        let name = rows.intern(None, &format!("name=op-{}", i % values));
        let id = rows.intern(None, &format!("id=v-{}", i % values));
        rows.row(1_000_000_000 * i as i64, &[name, id]);
    }
    let (buf, _, _) =
        IndexWriter::write_into(&rows, std::io::Cursor::new(Vec::new()), Vec::new()).unwrap();
    buf.into_inner()
}

fn tier(reader: &IndexReader<'_>, field: &str) -> FieldTier {
    reader
        .field_table()
        .iter()
        .find(|entry| entry.name == field)
        .unwrap()
        .tier
}

#[test]
fn pinned_field_with_1500_values_is_mid() {
    let bytes = file(1_500, &["name"]);
    let reader = IndexReader::open(&bytes).unwrap();

    assert_eq!(tier(&reader, "name"), FieldTier::Mid);
    assert_eq!(tier(&reader, "id"), FieldTier::High);

    let filter = reader.compile_filter(&Filter::new(), None).unwrap();
    let window = 0..1_000_000_000 * ROWS as i64;
    let facets = reader.facets(&["name"], &filter, window).unwrap();
    assert_eq!(facets[0].values.len(), 1_500);
    let faceted: u64 = facets[0].values.iter().map(|(_, n)| u64::from(*n)).sum();
    assert_eq!(faceted, ROWS as u64);

    let grid = Grid::new(0, 60_000_000_000, 50);
    let timeline = reader.timeline("name", &filter, grid).unwrap();
    assert_eq!(timeline.dimensions.len(), 1_500);
    let charted: u64 = timeline
        .buckets
        .iter()
        .map(|bucket| bucket.counts.iter().sum::<u64>())
        .sum();
    assert_eq!(charted, ROWS as u64);
    assert!(reader.timeline("id", &filter, grid).is_err());
}

#[test]
fn pinning_only_removes_the_high_tier() {
    let cases = [
        (50, FieldTier::Low),
        (500, FieldTier::Mid),
        (1_500, FieldTier::Mid),
    ];
    for (values, expected) in cases {
        let bytes = file(values, &["name"]);
        let reader = IndexReader::open(&bytes).unwrap();
        assert_eq!(tier(&reader, "name"), expected, "{values} values");
    }
}

#[test]
fn a_file_with_no_pins_classifies_by_cardinality_alone() {
    let bytes = file(1_500, &[]);
    let reader = IndexReader::open(&bytes).unwrap();

    assert_eq!(tier(&reader, "name"), FieldTier::High);
    assert_eq!(bytes, file(1_500, &["absent"]));
}
