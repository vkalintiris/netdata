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

// ── Stage D: facet aggregation pushdown ─────────────────────────────────────

/// A pushdown-enabled context with the fixture registered as `logs`.
async fn pushed_ctx(path: &std::path::Path) -> SessionContext {
    let table = SfstTable::open_path(path).expect("open fixture");
    let ctx = sfst_datafusion::session_context();
    ctx.register_table("logs", Arc::new(table)).expect("register");
    ctx
}

/// Run a `GROUP BY` query and return `(group, count)` rows as a sorted vec
/// (NULL group sorts first), so two result sets compare order-independently.
async fn group_counts(ctx: &SessionContext, sql: &str) -> Vec<(Option<String>, i64)> {
    use datafusion::arrow::array::{Array, AsArray, StringArray};
    use datafusion::arrow::datatypes::Int64Type;

    let batches = ctx.sql(sql).await.unwrap().collect().await.unwrap();
    let mut out = Vec::new();
    for b in &batches {
        let keys = b.column(0).as_any().downcast_ref::<StringArray>().unwrap();
        let counts = b.column(1).as_primitive::<Int64Type>();
        for i in 0..b.num_rows() {
            let k = if keys.is_null(i) {
                None
            } else {
                Some(keys.value(i).to_string())
            };
            out.push((k, counts.value(i)));
        }
    }
    out.sort();
    out
}

async fn explain_text(ctx: &SessionContext, sql: &str) -> String {
    let batches = ctx
        .sql(&format!("EXPLAIN {sql}"))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    pretty_format_batches(&batches).unwrap().to_string()
}

/// Find a low/mid-card scalar Str field with cardinality >= 2 — eligible for
/// facet pushdown and interesting enough to have multiple groups.
fn pick_facet_field(path: &std::path::Path) -> Option<String> {
    let data = std::fs::read(path).unwrap();
    let reader = sfst::IndexReader::open(&data).unwrap();
    let kinds: HashMap<String, ValueKind> =
        reader.tree().derive_scalar_kinds().into_iter().collect();
    reader
        .field_table()
        .iter()
        .find(|f| {
            !matches!(f.tier, sfst::FieldTier::High)
                && !f.name.contains("[]")
                && f.cardinality >= 2
                && matches!(kinds.get(&f.name), Some(ValueKind::Str))
        })
        .map(|f| f.name.clone())
}

/// Facet pushdown must (a) actually fire and (b) return exactly what the normal
/// aggregation plan returns — including the NULL group for rows lacking the field.
#[tokio::test]
async fn facet_pushdown_matches_normal_plan() {
    let Some(path) = fixture() else { return };
    let Some(field) = pick_facet_field(&path) else {
        eprintln!("SKIP: no low/mid scalar field in fixture");
        return;
    };
    let sql = format!(r#"SELECT "{field}" AS g, count(*) AS n FROM logs GROUP BY "{field}""#);

    let pushed = pushed_ctx(&path).await;
    let normal = ctx_with_fixture(&path).await; // plain SessionContext::new()

    let explain = explain_text(&pushed, &sql).await;
    assert!(
        explain.contains("SfstFacet"),
        "facet pushdown should fire:\n{explain}"
    );

    let got = group_counts(&pushed, &sql).await;
    let expected = group_counts(&normal, &sql).await;
    assert_eq!(got, expected, "pushed facet result must equal the normal plan");
    assert!(got.len() >= 2, "expected multiple groups");
}

/// A high-card group column and a WHERE that constrains the group column must
/// both fall back to the normal plan (no SfstFacet node).
#[tokio::test]
async fn facet_pushdown_falls_back() {
    let Some(path) = fixture() else { return };
    let ctx = pushed_ctx(&path).await;

    let data = std::fs::read(&path).unwrap();
    let reader = sfst::IndexReader::open(&data).unwrap();

    // High-card group column → fall back.
    if let Some(hc) = reader
        .field_table()
        .iter()
        .find(|f| matches!(f.tier, sfst::FieldTier::High) && !f.name.contains("[]"))
        .map(|f| f.name.clone())
    {
        let sql = format!(r#"SELECT "{hc}", count(*) FROM logs GROUP BY "{hc}""#);
        let explain = explain_text(&ctx, &sql).await;
        assert!(
            !explain.contains("SfstFacet"),
            "high-card group must NOT push:\n{explain}"
        );
    }

    // WHERE constrains the group column → fall back (facets exclude own selection).
    if let Some(field) = pick_facet_field(&path) {
        let sql = format!(
            r#"SELECT "{field}", count(*) FROM logs WHERE "{field}" = 'x' GROUP BY "{field}""#
        );
        let explain = explain_text(&ctx, &sql).await;
        assert!(
            !explain.contains("SfstFacet"),
            "WHERE on the group column must NOT push:\n{explain}"
        );
    }
}

// ── Stage D: timeline (date_bin) pushdown ────────────────────────────────────

/// `(bucket_ns, count)` rows from a 1-D `GROUP BY date_bin(...)`, sorted.
async fn timeline_1d(ctx: &SessionContext, sql: &str) -> Vec<(i64, i64)> {
    use datafusion::arrow::array::AsArray;
    use datafusion::arrow::datatypes::{Int64Type, TimestampNanosecondType};

    let batches = ctx.sql(sql).await.unwrap().collect().await.unwrap();
    let mut out = Vec::new();
    for b in &batches {
        let t = b.column(0).as_primitive::<TimestampNanosecondType>();
        let n = b.column(1).as_primitive::<Int64Type>();
        for i in 0..b.num_rows() {
            out.push((t.value(i), n.value(i)));
        }
    }
    out.sort();
    out
}

/// `(bucket_ns, value, count)` rows from a 2-D `GROUP BY date_bin(...), field`.
async fn timeline_2d(ctx: &SessionContext, sql: &str) -> Vec<(i64, Option<String>, i64)> {
    use datafusion::arrow::array::{Array, AsArray, StringArray};
    use datafusion::arrow::datatypes::{Int64Type, TimestampNanosecondType};

    let batches = ctx.sql(sql).await.unwrap().collect().await.unwrap();
    let mut out = Vec::new();
    for b in &batches {
        let t = b.column(0).as_primitive::<TimestampNanosecondType>();
        let g = b.column(1).as_any().downcast_ref::<StringArray>().unwrap();
        let n = b.column(2).as_primitive::<Int64Type>();
        for i in 0..b.num_rows() {
            let key = if g.is_null(i) {
                None
            } else {
                Some(g.value(i).to_string())
            };
            out.push((t.value(i), key, n.value(i)));
        }
    }
    out.sort();
    out
}

/// 1-D time histogram: pushdown must fire and match the normal plan exactly
/// (this is the date_bin bucket-alignment correctness check).
#[tokio::test]
async fn timeline_1d_pushdown_matches_normal() {
    let Some(path) = fixture() else { return };
    let sql =
        r#"SELECT date_bin(INTERVAL '1 minute', timestamp) AS t, count(*) AS n FROM logs GROUP BY t"#;

    let pushed = pushed_ctx(&path).await;
    let normal = ctx_with_fixture(&path).await;

    let explain = explain_text(&pushed, sql).await;
    assert!(
        explain.contains("SfstTimeline"),
        "timeline pushdown should fire:\n{explain}"
    );

    let got = timeline_1d(&pushed, sql).await;
    let expected = timeline_1d(&normal, sql).await;
    assert_eq!(got, expected, "pushed timeline must equal the normal plan");
    assert!(got.len() >= 2, "expected several time buckets");
}

/// 2-D time × value grid: pushdown must fire and match the normal plan,
/// including the NULL (absent-field) group per bucket.
#[tokio::test]
async fn timeline_2d_pushdown_matches_normal() {
    let Some(path) = fixture() else { return };
    let Some(field) = pick_facet_field(&path) else {
        eprintln!("SKIP: no low/mid scalar field in fixture");
        return;
    };
    let sql = format!(
        r#"SELECT date_bin(INTERVAL '1 minute', timestamp) AS t, "{field}" AS g, count(*) AS n FROM logs GROUP BY t, "{field}""#
    );

    let pushed = pushed_ctx(&path).await;
    let normal = ctx_with_fixture(&path).await;

    let explain = explain_text(&pushed, &sql).await;
    assert!(
        explain.contains("SfstTimeline"),
        "2-D timeline pushdown should fire:\n{explain}"
    );

    let got = timeline_2d(&pushed, &sql).await;
    let expected = timeline_2d(&normal, &sql).await;
    assert_eq!(got, expected, "pushed 2-D timeline must equal the normal plan");
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

/// Timing harness: facet pushdown vs the normal aggregation plan for the same
/// GROUP BY. Run with `--ignored --nocapture`.
#[tokio::test]
#[ignore = "timing harness; run with --ignored --nocapture"]
async fn bench_facet_pushdown() {
    let Some(path) = fixture() else { return };
    let Some(field) = pick_facet_field(&path) else { return };
    let sql = format!(r#"SELECT "{field}", count(*) FROM logs GROUP BY "{field}""#);

    let tl = r#"SELECT date_bin(INTERVAL '10 seconds', timestamp) AS t, count(*) AS n FROM logs GROUP BY t"#;
    for (label, q) in [("facet", sql.as_str()), ("timeline-1d", tl)] {
        for (ctx_label, ctx) in [
            ("pushed", pushed_ctx(&path).await),
            ("normal", ctx_with_fixture(&path).await),
        ] {
            let t = std::time::Instant::now();
            let rows = ctx.sql(q).await.unwrap().collect().await.unwrap();
            let n: usize = rows.iter().map(|b| b.num_rows()).sum();
            println!("  {:>10.3?}  rows={n:<6}  [{label}/{ctx_label}]", t.elapsed());
        }
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
