//! Per-`key=value` comparison of treight and roaring over the low- and
//! mid-cardinality posting lists of one derived corpus: sizes, container
//! shapes, heap allocations, and a realistic set-operation workload.
//!
//! ```text
//! cargo run --release -p treight-bench --bin treight-perkv -- \
//!     --derived data/derived/jetstream-250k \
//!     --out posts/treight-bitmap/evidence/perkv/jetstream-250k
//! ```
//!
//! Writes `<out>.json` (aggregates), `<out>.md` (readable summary) and
//! `<out>.csv.gz` (one row per list). The CSV carries the field name and a
//! truncated SHA-256 of the value, never the value itself: the corpora hold
//! personal data (certificate subjects, DIDs, handles).
//!
//! Encoding mirrors `treight-footprint`: time-ordered positions, treight
//! complements lists denser than half the universe. Roaring is measured
//! both as built by `from_sorted_iter` and after `optimize()` (run
//! containers), and every load/set-op measurement uses the optimized form,
//! which is what a careful roaring user would store.
//!
//! Allocation counts come from a counting global allocator wrapped around
//! `System`; the binary is single-threaded, so the counters attribute
//! exactly to the measured region.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use roaring::RoaringBitmap;
use sha2::{Digest, Sha256};
use treight::Bitmap;
use treight_bench::DerivedCache;

struct CountingAlloc;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static REALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        REALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(
            new_size.saturating_sub(layout.size()) as u64,
            Ordering::Relaxed,
        );
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Heap activity inside a measured region: fresh allocations, reallocations
/// (buffer growth), and bytes requested (growth counted as the delta).
#[derive(Clone, Copy, Default, Debug)]
struct AllocDelta {
    allocs: u64,
    reallocs: u64,
    bytes: u64,
}

fn measure<T>(f: impl FnOnce() -> T) -> (T, AllocDelta) {
    let a0 = ALLOCS.load(Ordering::Relaxed);
    let r0 = REALLOCS.load(Ordering::Relaxed);
    let b0 = BYTES.load(Ordering::Relaxed);
    let out = f();
    let d = AllocDelta {
        allocs: ALLOCS.load(Ordering::Relaxed) - a0,
        reallocs: REALLOCS.load(Ordering::Relaxed) - r0,
        bytes: BYTES.load(Ordering::Relaxed) - b0,
    };
    (out, d)
}

impl AllocDelta {
    fn json(self) -> serde_json::Value {
        serde_json::json!({ "allocs": self.allocs, "reallocs": self.reallocs, "bytes": self.bytes })
    }
}

const CARDINALITY_THRESHOLD: usize = 100;
/// Timed workloads run this many times; the fastest run is reported.
const TIMING_RUNS: usize = 7;

#[derive(Parser)]
struct Args {
    /// Derived cache directory (from treight-derive)
    #[arg(long)]
    derived: PathBuf,

    /// Output path prefix; `.json`, `.md` and `.csv.gz` are appended
    #[arg(long)]
    out: PathBuf,
}

struct List {
    field: String,
    value_hash: String,
    tier: &'static str,
    card: u64,
    bm: Bitmap,
    tree: Vec<u8>,
    rb: RoaringBitmap,
    rb_plain_ser: u64,
    rb_ser: Vec<u8>,
}

fn treight_wire(bm: &Bitmap, tree: &[u8]) -> Vec<u8> {
    // RawBitmap's `[universe u32][len u32][tree]` plus one byte for the
    // inverted flag, which the raw wire format does not carry.
    let mut w = vec![u8::from(bm.is_inverted())];
    bm.inner().serialize_into(tree, &mut w).expect("vec write");
    w
}

fn treight_unwire(w: &[u8]) -> (Bitmap, Vec<u8>) {
    let (raw, tree) = treight::RawBitmap::deserialize_from(&w[1..]).expect("treight wire");
    // `Bitmap` has no public constructor from a RawBitmap. Descriptors of
    // the same universe are interchangeable (same levels), so `full` /
    // `empty` give the inverted / direct descriptor for the loaded tree.
    let bm = if w[0] == 1 {
        Bitmap::full(raw.universe_size())
    } else {
        Bitmap::empty(raw.universe_size())
    };
    (bm, tree)
}

fn best_of<T>(mut f: impl FnMut() -> T) -> (T, Duration) {
    let mut best = Duration::MAX;
    let mut last = None;
    for _ in 0..TIMING_RUNS {
        let t = Instant::now();
        let r = f();
        best = best.min(t.elapsed());
        last = Some(r);
    }
    (last.expect("TIMING_RUNS > 0"), best)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cache = DerivedCache::load(&args.derived)?;
    let universe = cache.meta.rows as u32;
    anyhow::ensure!(universe > 0, "empty derived cache");
    let half = universe as u64 / 2;

    let mut field_distinct: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for (kv, _) in &cache.postings {
        let field = kv.split('=').next().unwrap_or(kv);
        *field_distinct.entry(field).or_default() += 1;
    }

    // ---- build every low/mid list both ways --------------------------------
    let mut lists: Vec<List> = Vec::new();
    for (kv, positions) in &cache.postings {
        let (field, value) = kv.split_once('=').unwrap_or((kv.as_str(), ""));
        let distinct = field_distinct[field];
        let tier = if distinct < CARDINALITY_THRESHOLD {
            "low"
        } else if distinct < CARDINALITY_THRESHOLD * 10 {
            "mid"
        } else {
            continue;
        };

        let mut sorted: Vec<u32> = positions
            .iter()
            .map(|&p| cache.to_sorted[p as usize])
            .collect();
        sorted.sort_unstable();
        let card = sorted.len() as u64;

        let mut tree = Vec::new();
        let bm = if card > half {
            let mut bits = treight_bench::Bitset::new(universe);
            for &v in &sorted {
                bits.set(v);
            }
            Bitmap::from_sorted_iter_complemented(bits.iter_zeros(), universe, &mut tree)
        } else {
            Bitmap::from_sorted_iter(sorted.iter().copied(), universe, &mut tree)
        };
        tree.shrink_to_fit();

        let mut rb = RoaringBitmap::from_sorted_iter(sorted.iter().copied()).expect("ascending");
        let rb_plain_ser = rb.serialized_size() as u64;
        rb.optimize();
        let mut rb_ser = Vec::with_capacity(rb.serialized_size());
        rb.serialize_into(&mut rb_ser)?;

        let digest = Sha256::digest(value.as_bytes());
        let value_hash: String = digest[..6].iter().map(|b| format!("{b:02x}")).collect();

        lists.push(List {
            field: field.to_string(),
            value_hash,
            tier,
            card,
            bm,
            tree,
            rb,
            rb_plain_ser,
            rb_ser,
        });
    }
    anyhow::ensure!(!lists.is_empty(), "no low/mid lists");

    // ---- per-list sizes, shapes and load allocations ---------------------
    let mut csv_rows = Vec::with_capacity(lists.len());
    let mut tot = BTreeMap::<&str, u64>::new();
    let mut t_wires: Vec<Vec<u8>> = Vec::with_capacity(lists.len());
    for l in &lists {
        let wire = treight_wire(&l.bm, &l.tree);
        let st = l.rb.statistics();

        // Load = turn stored bytes into a queryable value.
        let (rb_loaded, rb_load) =
            measure(|| RoaringBitmap::deserialize_from(&l.rb_ser[..]).expect("roaring"));
        assert_eq!(rb_loaded, l.rb);
        let ((bm2, tree2), t_load) = measure(|| treight_unwire(&wire));
        assert_eq!(tree2, l.tree);
        assert_eq!(bm2.is_inverted(), l.bm.is_inverted());
        drop((rb_loaded, tree2));

        assert_eq!(l.bm.len(&l.tree), l.card, "treight len");
        assert_eq!(l.rb.len(), l.card, "roaring len");

        let row = (
            l.tree.len() as u64,
            wire.len() as u64,
            l.rb_plain_ser,
            l.rb_ser.len() as u64,
            st.n_containers as u64,
            st.n_array_containers as u64,
            st.n_bitset_containers as u64,
            st.n_run_containers as u64,
            rb_load,
            t_load,
        );
        for (k, v) in [
            ("lists", 1),
            ("card", l.card),
            ("treight_tree", row.0),
            ("treight_wire", row.1),
            ("roaring_plain", row.2),
            ("roaring_opt", row.3),
            ("containers", row.4),
            ("array_containers", row.5),
            ("bitset_containers", row.6),
            ("run_containers", row.7),
            ("roaring_load_allocs", rb_load.allocs + rb_load.reallocs),
            ("roaring_load_bytes", rb_load.bytes),
            ("treight_load_allocs", t_load.allocs + t_load.reallocs),
            ("treight_load_bytes", t_load.bytes),
            ("complemented", u64::from(l.bm.is_inverted())),
            ("treight_smaller", u64::from(row.1 < row.3)),
        ] {
            *tot.entry(k).or_default() += v;
        }
        csv_rows.push(row);
        t_wires.push(wire);
    }

    // ---- zstd over each side's concatenated stored form -------------------
    let t_concat: Vec<u8> = t_wires.concat();
    let r_concat: Vec<u8> = lists
        .iter()
        .flat_map(|l| l.rb_ser.iter().copied())
        .collect();
    let mut zstd = serde_json::Map::new();
    for level in [1, 3, 19] {
        let t = zstd::encode_all(&t_concat[..], level)?.len();
        let r = zstd::encode_all(&r_concat[..], level)?.len();
        zstd.insert(
            format!("level_{level}"),
            serde_json::json!({ "treight": t, "roaring": r, "ratio": t as f64 / r as f64 }),
        );
    }

    // ---- load everything (timed) ------------------------------------------
    let (_, rb_load_time) = best_of(|| {
        lists
            .iter()
            .map(|l| RoaringBitmap::deserialize_from(&l.rb_ser[..]).expect("roaring"))
            .collect::<Vec<_>>()
    });
    let (_, t_load_time) = best_of(|| {
        t_wires
            .iter()
            .map(|w| treight_unwire(w))
            .collect::<Vec<_>>()
    });

    // ---- workload 1: clip every list to a time window and count -----------
    // Windows: the middle half of the universe and a 2% slice.
    let mut window_results = serde_json::Map::new();
    for (name, lo, hi) in [
        ("window_50pct", universe / 4, universe / 4 * 3),
        ("window_2pct", universe / 100 * 49, universe / 100 * 51),
    ] {
        let mut win_tree = Vec::new();
        let win = Bitmap::from_range(lo..hi, universe, &mut win_tree);
        let win_rb: RoaringBitmap = (lo..hi).collect();

        let mut out = Vec::new();
        let run_t = |out: &mut Vec<u8>| {
            let mut total = 0u64;
            for l in &lists {
                out.clear();
                let r = l.bm.and(&l.tree, &win, &win_tree, out);
                total += r.len(out);
            }
            total
        };
        let run_r = || {
            let mut total = 0u64;
            for l in &lists {
                let r = &l.rb & &win_rb;
                total += r.len();
            }
            total
        };

        // Allocation counts on a cold `out` (first pass) and a warm one.
        let (t_total, t_cold) = measure(|| run_t(&mut out));
        let (_, t_warm) = measure(|| run_t(&mut out));
        let (r_total, r_alloc) = measure(run_r);
        assert_eq!(t_total, r_total, "{name}: treight and roaring disagree");

        let (_, t_time) = best_of(|| run_t(&mut out));
        let (_, r_time) = best_of(run_r);

        // Same question answered without materializing an intersection:
        // count each list's members inside the window.
        let rc_t = || {
            lists
                .iter()
                .map(|l| l.bm.range_cardinality(&l.tree, lo..hi))
                .sum::<u64>()
        };
        let rc_r = || {
            lists
                .iter()
                .map(|l| l.rb.range_cardinality(lo..hi))
                .sum::<u64>()
        };
        let (rc_t_total, rc_t_alloc) = measure(rc_t);
        let (rc_r_total, rc_r_alloc) = measure(rc_r);
        assert_eq!(
            rc_t_total, t_total,
            "{name}: range_cardinality disagrees with and+len"
        );
        assert_eq!(
            rc_r_total, t_total,
            "{name}: roaring range_cardinality disagrees"
        );
        let (_, rc_t_time) = best_of(rc_t);
        let (_, rc_r_time) = best_of(rc_r);

        window_results.insert(
            name.to_string(),
            serde_json::json!({
                "range": [lo, hi],
                "window_tree_bytes": win_tree.len(),
                "matched_total": t_total,
                "treight_ns": t_time.as_nanos() as u64,
                "roaring_ns": r_time.as_nanos() as u64,
                "treight_allocs_cold": t_cold.json(),
                "treight_allocs_warm": t_warm.json(),
                "roaring_allocs": r_alloc.json(),
                "range_cardinality": {
                    "treight_ns": rc_t_time.as_nanos() as u64,
                    "roaring_ns": rc_r_time.as_nanos() as u64,
                    "treight_allocs": rc_t_alloc.json(),
                    "roaring_allocs": rc_r_alloc.json(),
                },
            }),
        );
    }

    // ---- workload 1b: AND every list with one ~10%-density list -----------
    // The shape of `field_a=x AND field_b=y`. The partner is the direct
    // (non-complemented) list whose density is closest to 10%.
    let partner = lists
        .iter()
        .filter(|l| !l.bm.is_inverted())
        .min_by_key(|l| l.card.abs_diff(universe as u64 / 10))
        .expect("at least one direct list");
    let run_t_pair = |out: &mut Vec<u8>| {
        let mut total = 0u64;
        for l in &lists {
            out.clear();
            let r = l.bm.and(&l.tree, &partner.bm, &partner.tree, out);
            total += r.len(out);
        }
        total
    };
    let run_r_pair = || {
        let mut total = 0u64;
        for l in &lists {
            total += (&l.rb & &partner.rb).len();
        }
        total
    };
    let mut out = Vec::new();
    let (pair_total, pair_t_cold) = measure(|| run_t_pair(&mut out));
    let (_, pair_t_warm) = measure(|| run_t_pair(&mut out));
    let (pair_r_total, pair_r_alloc) = measure(run_r_pair);
    assert_eq!(
        pair_total, pair_r_total,
        "pair AND: treight and roaring disagree"
    );
    let (_, pair_t_time) = best_of(|| run_t_pair(&mut out));
    let (_, pair_r_time) = best_of(run_r_pair);
    let pair_json = serde_json::json!({
        "partner_card": partner.card,
        "partner_tree_bytes": partner.tree.len(),
        "matched_total": pair_total,
        "treight_ns": pair_t_time.as_nanos() as u64,
        "roaring_ns": pair_r_time.as_nanos() as u64,
        "treight_allocs_cold": pair_t_cold.json(),
        "treight_allocs_warm": pair_t_warm.json(),
        "roaring_allocs": pair_r_alloc.json(),
    });

    // ---- workload 2: per field, OR all values together and count ----------
    let mut by_field: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, l) in lists.iter().enumerate() {
        by_field.entry(l.field.as_str()).or_default().push(i);
    }
    let mut acc = Vec::new();
    let mut tmp = Vec::new();
    let run_t_or = |acc: &mut Vec<u8>, tmp: &mut Vec<u8>| {
        let mut total = 0u64;
        for idx in by_field.values() {
            let first = &lists[idx[0]];
            acc.clear();
            acc.extend_from_slice(&first.tree);
            let mut bm = first.bm;
            for &i in &idx[1..] {
                tmp.clear();
                bm = bm.or(acc, &lists[i].bm, &lists[i].tree, tmp);
                std::mem::swap(acc, tmp);
            }
            total += bm.len(acc);
        }
        total
    };
    let run_r_or = || {
        let mut total = 0u64;
        for idx in by_field.values() {
            let mut acc = lists[idx[0]].rb.clone();
            for &i in &idx[1..] {
                acc |= &lists[i].rb;
            }
            total += acc.len();
        }
        total
    };
    let (t_or_total, t_or_cold) = measure(|| run_t_or(&mut acc, &mut tmp));
    let (_, t_or_warm) = measure(|| run_t_or(&mut acc, &mut tmp));
    let (r_or_total, r_or_alloc) = measure(run_r_or);
    assert_eq!(
        t_or_total, r_or_total,
        "field OR: treight and roaring disagree"
    );
    let (_, t_or_time) = best_of(|| run_t_or(&mut acc, &mut tmp));
    let (_, r_or_time) = best_of(run_r_or);

    // ---- per-field aggregates ---------------------------------------------
    let mut fields_json = Vec::new();
    for (field, idx) in &by_field {
        let (mut tt, mut tw, mut rs, mut rc) = (0u64, 0u64, 0u64, 0u64);
        for &i in idx {
            tt += csv_rows[i].0;
            tw += csv_rows[i].1;
            rs += csv_rows[i].3;
            rc += csv_rows[i].4;
        }
        fields_json.push(serde_json::json!({
            "field": field, "tier": lists[idx[0]].tier, "values": idx.len(),
            "treight_tree": tt, "treight_wire": tw, "roaring_opt": rs, "roaring_containers": rc,
        }));
    }

    let json = serde_json::json!({
        "corpus_sha256": cache.meta.corpus_sha256,
        "universe": universe,
        "scope": "low+mid cardinality fields (< 1000 distinct values per field)",
        "totals": tot,
        "zstd_concatenated": zstd,
        "load_all": {
            "treight_ns": t_load_time.as_nanos() as u64,
            "roaring_ns": rb_load_time.as_nanos() as u64,
        },
        "workloads": {
            "and_window_len": window_results,
            "and_partner_len": pair_json,
            "field_or_len": {
                "fields": by_field.len(),
                "matched_total": t_or_total,
                "treight_ns": t_or_time.as_nanos() as u64,
                "roaring_ns": r_or_time.as_nanos() as u64,
                "treight_allocs_cold": t_or_cold.json(),
                "treight_allocs_warm": t_or_warm.json(),
                "roaring_allocs": r_or_alloc.json(),
            },
        },
        "fields": fields_json,
        "notes": [
            "treight_wire = 1 inverted-flag byte + RawBitmap wire (8-byte header + tree)",
            "roaring_opt = serialized size after RoaringBitmap::optimize(); roaring_plain = as built by from_sorted_iter",
            "load allocs/bytes: counting global allocator around deserialize_from of each side's stored bytes",
            "timings: fastest of 7 runs, release build, one thread, shared machine; compare orderings",
        ],
    });
    let with_ext = |ext: &str| {
        let mut p = args.out.clone().into_os_string();
        p.push(ext);
        PathBuf::from(p)
    };
    std::fs::write(
        with_ext(".json"),
        serde_json::to_string_pretty(&json)? + "\n",
    )?;

    // ---- per-list CSV -------------------------------------------------------
    let csv = std::fs::File::create(with_ext(".csv.gz"))?;
    let mut w = std::io::BufWriter::new(flate2::write::GzEncoder::new(
        csv,
        flate2::Compression::default(),
    ));
    writeln!(
        w,
        "field,value_sha256_48,tier,card,density,complemented,treight_tree,treight_wire,roaring_plain,roaring_opt,containers,array,bitset,run,roaring_load_allocs,roaring_load_bytes,treight_load_allocs,treight_load_bytes"
    )?;
    for (l, r) in lists.iter().zip(&csv_rows) {
        writeln!(
            w,
            "{},{},{},{},{:.6},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            l.field.replace(',', ";"),
            l.value_hash,
            l.tier,
            l.card,
            l.card as f64 / universe as f64,
            l.bm.is_inverted(),
            r.0,
            r.1,
            r.2,
            r.3,
            r.4,
            r.5,
            r.6,
            r.7,
            r.8.allocs + r.8.reallocs,
            r.8.bytes,
            r.9.allocs + r.9.reallocs,
            r.9.bytes
        )?;
    }
    w.into_inner().map_err(|e| e.into_error())?.finish()?;

    // ---- markdown summary ---------------------------------------------------
    let g = |k: &str| tot.get(k).copied().unwrap_or(0);
    let ratio = |a: u64, b: u64| a as f64 / b.max(1) as f64;
    let mut md = String::new();
    md.push_str(&format!(
        "# Per-key=value comparison: {}\n\nUniverse {} rows; {} low/mid lists across {} fields.\n\n",
        args.out.file_name().and_then(|s| s.to_str()).unwrap_or("corpus"),
        universe,
        g("lists"),
        by_field.len()
    ));
    md.push_str("| metric | treight | roaring | t/r |\n|---|---|---|---|\n");
    md.push_str(&format!(
        "| stored bytes (treight wire vs roaring optimized) | {} | {} | {:.3} |\n",
        g("treight_wire"),
        g("roaring_opt"),
        ratio(g("treight_wire"), g("roaring_opt"))
    ));
    md.push_str(&format!(
        "| treight tree bytes only / roaring as built | {} | {} | {:.3} |\n",
        g("treight_tree"),
        g("roaring_plain"),
        ratio(g("treight_tree"), g("roaring_plain"))
    ));
    md.push_str(&format!(
        "| heap allocations to load all lists | {} | {} | {:.3} |\n",
        g("treight_load_allocs"),
        g("roaring_load_allocs"),
        ratio(g("treight_load_allocs"), g("roaring_load_allocs"))
    ));
    md.push_str(&format!(
        "| heap bytes requested to load all lists | {} | {} | {:.3} |\n",
        g("treight_load_bytes"),
        g("roaring_load_bytes"),
        ratio(g("treight_load_bytes"), g("roaring_load_bytes"))
    ));
    md.push_str(&format!(
        "| lists where treight is smaller | {} of {} | | |\n",
        g("treight_smaller"),
        g("lists")
    ));
    md.push_str(&format!(
        "\nRoaring containers: {} total ({} array, {} bitset, {} run). Treight complemented lists: {}.\n",
        g("containers"),
        g("array_containers"),
        g("bitset_containers"),
        g("run_containers"),
        g("complemented")
    ));
    md.push_str("\n## zstd over concatenated stored forms\n\n| level | treight | roaring | t/r |\n|---|---|---|---|\n");
    for (k, v) in &zstd {
        md.push_str(&format!(
            "| {} | {} | {} | {:.3} |\n",
            k,
            v["treight"],
            v["roaring"],
            v["ratio"].as_f64().unwrap_or(0.0)
        ));
    }
    md.push_str(&format!(
        "\n## Workloads (fastest of {TIMING_RUNS})\n\n| workload | treight ms | roaring ms | treight allocs (cold / warm) | roaring allocs |\n|---|---|---|---|---|\n"
    ));
    let ms = |ns: u64| ns as f64 / 1e6;
    md.push_str(&format!(
        "| load all lists from bytes | {:.3} | {:.3} | {} | {} |\n",
        ms(t_load_time.as_nanos() as u64),
        ms(rb_load_time.as_nanos() as u64),
        g("treight_load_allocs"),
        g("roaring_load_allocs")
    ));
    let n = |v: &serde_json::Value| {
        v["allocs"].as_u64().unwrap_or(0) + v["reallocs"].as_u64().unwrap_or(0)
    };
    for (name, v) in &window_results {
        md.push_str(&format!(
            "| AND {} ({} tree B) + len, every list | {:.3} | {:.3} | {} / {} | {} |\n",
            name,
            v["window_tree_bytes"],
            ms(v["treight_ns"].as_u64().unwrap_or(0)),
            ms(v["roaring_ns"].as_u64().unwrap_or(0)),
            n(&v["treight_allocs_cold"]),
            n(&v["treight_allocs_warm"]),
            n(&v["roaring_allocs"]),
        ));
        let rc = &v["range_cardinality"];
        md.push_str(&format!(
            "| range_cardinality {}, every list | {:.3} | {:.3} | {} | {} |\n",
            name,
            ms(rc["treight_ns"].as_u64().unwrap_or(0)),
            ms(rc["roaring_ns"].as_u64().unwrap_or(0)),
            n(&rc["treight_allocs"]),
            n(&rc["roaring_allocs"]),
        ));
    }
    md.push_str(&format!(
        "| AND ~10% list (card {}, {} tree B) + len, every list | {:.3} | {:.3} | {} / {} | {} |\n",
        pair_json["partner_card"],
        pair_json["partner_tree_bytes"],
        ms(pair_json["treight_ns"].as_u64().unwrap_or(0)),
        ms(pair_json["roaring_ns"].as_u64().unwrap_or(0)),
        n(&pair_json["treight_allocs_cold"]),
        n(&pair_json["treight_allocs_warm"]),
        n(&pair_json["roaring_allocs"]),
    ));
    md.push_str(&format!(
        "| OR all values per field + len ({} fields) | {:.3} | {:.3} | {} / {} | {} |\n",
        by_field.len(),
        ms(t_or_time.as_nanos() as u64),
        ms(r_or_time.as_nanos() as u64),
        t_or_cold.allocs + t_or_cold.reallocs,
        t_or_warm.allocs + t_or_warm.reallocs,
        r_or_alloc.allocs + r_or_alloc.reallocs
    ));
    std::fs::write(with_ext(".md"), md)?;
    println!("wrote {}.{{json,md,csv.gz}}", args.out.display());
    Ok(())
}
