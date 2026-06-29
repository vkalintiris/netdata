//! Stage A integration tests: run SQL over a real SFST file and check results
//! against an independent `sfst::IndexReader` oracle.
//!
//! These need a real SFST fixture. Set `SFST_DF_FIXTURE` to its path (e.g.
//! `~/repos/tmp/ng/out-v9.sfst`); the tests skip cleanly when it is unset or
//! missing so the suite stays green on machines without the fixture.

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::array::{Int64Array, TimestampNanosecondArray};
use datafusion::arrow::util::pretty::pretty_format_batches;
use datafusion::common::{Column, ScalarValue};
use datafusion::logical_expr::Expr;
use datafusion::prelude::SessionContext;
use sfst::ValueKind;
use sfst_datafusion::{SfstTable, TS_COLUMN};

/// A column reference by its exact (dotted) name — `col()` would mis-parse a
/// dotted name as `relation.column`.
fn raw_col(name: &str) -> Expr {
    Expr::Column(Column::new_unqualified(name))
}

fn ts_lit(ns: i64) -> Expr {
    Expr::Literal(ScalarValue::TimestampNanosecond(Some(ns), None), None)
}

/// Returns the fixture path, or `None` (with a skip note) when unavailable.
fn fixture() -> Option<std::path::PathBuf> {
    let p = std::env::var_os("SFST_DF_FIXTURE")?;
    let p = std::path::PathBuf::from(p);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("SKIP: SFST_DF_FIXTURE={} does not exist", p.display());
        None
    }
}

async fn ctx_with_fixture(path: &std::path::Path) -> SessionContext {
    let table = SfstTable::open_path(path).expect("open fixture");
    let ctx = SessionContext::new();
    ctx.register_table("logs", Arc::new(table)).expect("register");
    ctx
}

/// Oracle row count straight from the SFST summary.
fn oracle_record_count(path: &std::path::Path) -> u32 {
    let data = std::fs::read(path).unwrap();
    let reader = sfst::IndexReader::open(&data).unwrap();
    reader.summary().record_count
}

#[tokio::test]
async fn count_star_matches_record_count() {
    let Some(path) = fixture() else { return };
    let expected = oracle_record_count(&path) as i64;

    let ctx = ctx_with_fixture(&path).await;
    let batches = ctx
        .sql("SELECT count(*) AS n FROM logs")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let n = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap()
        .value(0);
    assert_eq!(n, expected, "count(*) must equal the SFST record_count");
}

#[tokio::test]
async fn projection_emits_only_requested_column() {
    let Some(path) = fixture() else { return };
    let ctx = ctx_with_fixture(&path).await;

    let df = ctx.sql("SELECT timestamp FROM logs LIMIT 10").await.unwrap();
    let schema = df.schema().clone();
    assert_eq!(schema.fields().len(), 1, "projection should yield one column");
    assert_eq!(schema.field(0).name(), "timestamp");

    let batches = df.collect().await.unwrap();
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total, 10, "LIMIT 10 should yield 10 rows");
    assert_eq!(
        batches[0].num_columns(),
        1,
        "scan must build only the projected column",
    );
}

/// The first N timestamps from SQL must equal the first N from the on-disk TIMS
/// column (both are chronological), proving the timestamp pivot is faithful.
#[tokio::test]
async fn timestamps_match_oracle_head() {
    let Some(path) = fixture() else { return };

    // Oracle: first 10 timestamps in chronological order.
    let data = std::fs::read(&path).unwrap();
    let reader = sfst::IndexReader::open(&data).unwrap();
    let ts = reader.load_timestamps().unwrap();
    let expected: Vec<i64> = (0..10).filter_map(|i| ts.at(i)).collect();

    let ctx = ctx_with_fixture(&path).await;
    let batches = ctx
        .sql("SELECT timestamp FROM logs LIMIT 10")
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let col = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<TimestampNanosecondArray>()
        .unwrap();
    let got: Vec<i64> = (0..col.len()).map(|i| col.value(i)).collect();
    assert_eq!(got, expected, "SQL timestamps must match the TIMS head");
}

/// Time-window pushdown must count exactly what the index counts for the same
/// half-open window.
#[tokio::test]
async fn time_window_pushdown_matches_oracle() {
    let Some(path) = fixture() else { return };

    let data = std::fs::read(&path).unwrap();
    let reader = sfst::IndexReader::open(&data).unwrap();
    let ts = reader.load_timestamps().unwrap();
    // A cutoff in the middle of the file's time range.
    let mid = ts.at(reader.summary().record_count / 2).unwrap();
    let empty = reader.compile_filter(&sfst::Filter::new(), None).unwrap();
    let oracle = reader
        .matched_positions(&empty, mid..i64::MAX)
        .unwrap()
        .len();

    let ctx = ctx_with_fixture(&path).await;
    let got = ctx
        .table("logs")
        .await
        .unwrap()
        .filter(raw_col(TS_COLUMN).gt_eq(ts_lit(mid)))
        .unwrap()
        .count()
        .await
        .unwrap();

    assert_eq!(got, oracle, "timestamp >= mid count must match the index");
    assert!(oracle > 0 && oracle < reader.summary().record_count as usize);
}

/// Equality pushdown must count exactly what `Filter::select` counts.
#[tokio::test]
async fn string_eq_pushdown_matches_oracle() {
    let Some(path) = fixture() else { return };
    let Some((field, value)) = pick_str_field_value(&path) else {
        eprintln!("SKIP: no scalar Str field/value found in fixture");
        return;
    };

    let data = std::fs::read(&path).unwrap();
    let reader = sfst::IndexReader::open(&data).unwrap();
    let filter = sfst::Filter::new().select(field.clone(), value.clone());
    let compiled = reader.compile_filter(&filter, None).unwrap();
    let oracle = reader
        .matched_positions(&compiled, i64::MIN..i64::MAX)
        .unwrap()
        .len();

    let ctx = ctx_with_fixture(&path).await;
    let got = ctx
        .table("logs")
        .await
        .unwrap()
        .filter(raw_col(&field).eq(Expr::Literal(ScalarValue::Utf8(Some(value.clone())), None)))
        .unwrap()
        .count()
        .await
        .unwrap();

    assert!(oracle > 0, "chosen value should match at least one row");
    assert_eq!(got, oracle, "{field}={value} count must match the index");
}

/// An Exact-pushed predicate must let DataFusion drop its own `FilterExec`.
#[tokio::test]
async fn exact_pushdown_drops_filter_exec() {
    let Some(path) = fixture() else { return };

    let data = std::fs::read(&path).unwrap();
    let reader = sfst::IndexReader::open(&data).unwrap();
    let ts = reader.load_timestamps().unwrap();
    let mid = ts.at(reader.summary().record_count / 2).unwrap();

    let ctx = ctx_with_fixture(&path).await;
    let plan = ctx
        .table("logs")
        .await
        .unwrap()
        .filter(raw_col(TS_COLUMN).gt_eq(ts_lit(mid)))
        .unwrap()
        .select(vec![raw_col(TS_COLUMN)])
        .unwrap()
        .explain(false, false)
        .unwrap()
        .collect()
        .await
        .unwrap();

    let text = pretty_format_batches(&plan).unwrap().to_string();
    assert!(text.contains("SfstExec"), "plan should scan via SfstExec:\n{text}");
    assert!(
        !text.contains("FilterExec"),
        "Exact time pushdown should remove FilterExec:\n{text}"
    );
}

/// The advertised timestamp ordering must let DataFusion skip a `SortExec` for
/// `ORDER BY timestamp`.
#[tokio::test]
async fn order_by_timestamp_needs_no_sort() {
    let Some(path) = fixture() else { return };
    let ctx = ctx_with_fixture(&path).await;

    let plan = ctx
        .table("logs")
        .await
        .unwrap()
        .select(vec![raw_col(TS_COLUMN)])
        .unwrap()
        .sort(vec![raw_col(TS_COLUMN).sort(true, false)])
        .unwrap()
        .explain(false, false)
        .unwrap()
        .collect()
        .await
        .unwrap();

    let text = pretty_format_batches(&plan).unwrap().to_string();
    assert!(text.contains("SfstExec"), "plan should scan via SfstExec:\n{text}");
    assert!(
        !text.contains("SortExec"),
        "advertised timestamp ordering should remove SortExec:\n{text}"
    );
}

/// Column-direct reads of a high-card field (the SB-scan path) must return the
/// exact values the row-major `materialize_rows` oracle extracts.
#[tokio::test]
async fn high_card_field_values_match_oracle() {
    let Some(path) = fixture() else { return };
    let data = std::fs::read(&path).unwrap();
    let reader = sfst::IndexReader::open(&data).unwrap();

    let Some(field) = reader
        .field_table()
        .iter()
        .find(|f| matches!(f.tier, sfst::FieldTier::High) && !f.name.contains("[]"))
        .map(|f| f.name.clone())
    else {
        eprintln!("SKIP: no high-card scalar field in fixture");
        return;
    };

    let n = reader.summary().record_count.min(500);
    let positions: Vec<u32> = (0..n).collect();
    let rows = reader.materialize_rows(&positions).unwrap();
    let direct = reader.materialize_field(&field, &positions).unwrap();

    for (i, row) in rows.iter().enumerate() {
        let expected: Vec<String> = row
            .fields
            .iter()
            .filter(|(k, _)| *k == field)
            .map(|(_, v)| v.clone())
            .collect();
        assert_eq!(direct[i], expected, "high-card {field} at position {i}");
    }
}

/// Timing harness (not an assertion). Run explicitly:
/// `SFST_DF_FIXTURE=... cargo test -p sfst-datafusion bench_projection_cost -- --ignored --nocapture`
#[tokio::test]
#[ignore = "timing harness; run with --ignored --nocapture"]
async fn bench_projection_cost() {
    let Some(path) = fixture() else { return };
    let ctx = ctx_with_fixture(&path).await;

    let queries = [
        r#"SELECT count(*) FROM logs"#,
        r#"SELECT timestamp FROM logs"#,
        r#"SELECT "body.data.leaf_cert.sha256" FROM logs"#,
        r#"SELECT "body.commit.record.text" FROM logs"#,
        r#"SELECT timestamp, "body.data.leaf_cert.sha256" FROM logs"#,
        r#"SELECT * FROM logs LIMIT 100000"#,
    ];

    for sql in queries {
        let t = std::time::Instant::now();
        let batches = match ctx.sql(sql).await {
            Ok(df) => df.collect().await,
            Err(e) => {
                println!("  ERR   {sql}: {e:?}");
                continue;
            }
        };
        let dur = t.elapsed();
        match batches {
            Ok(b) => {
                let rows: usize = b.iter().map(|x| x.num_rows()).sum();
                println!("  {dur:>10.3?}  rows={rows:<8}  {sql}");
            }
            Err(e) => println!("  ERR   {sql}: {e:?}"),
        }
    }
}

/// Find a scalar `Str` field with a concrete, quote-free value from the head of
/// the file, so the equality test has a real predicate to push.
fn pick_str_field_value(path: &std::path::Path) -> Option<(String, String)> {
    let data = std::fs::read(path).unwrap();
    let reader = sfst::IndexReader::open(&data).unwrap();
    let kinds: HashMap<String, ValueKind> =
        reader.tree().derive_scalar_kinds().into_iter().collect();

    let n = reader.summary().record_count.min(50);
    let positions: Vec<u32> = (0..n).collect();
    let rows = reader.materialize_rows(&positions).ok()?;
    for r in &rows {
        for (k, v) in &r.fields {
            if k.contains("[]") || v.is_empty() {
                continue;
            }
            if matches!(kinds.get(k), Some(ValueKind::Str)) {
                return Some((k.clone(), v.clone()));
            }
        }
    }
    None
}
