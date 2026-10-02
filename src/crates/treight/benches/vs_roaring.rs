//! treight vs roaring on REAL posting-list shapes — the blog's CPU and
//! memory benchmark.
//!
//! ```text
//! TREIGHT_BENCH_DERIVED=data/derived/certstream-250k \
//!     cargo bench -p treight --features roaring --bench vs_roaring
//! ```
//!
//! `TREIGHT_BENCH_DERIVED` points at one derived cache directory (one with
//! `postings.bin`) or a root containing several; every corpus found is
//! benched as its own group. Unset → quiet skip (the `sfst/benches/decode.rs`
//! pattern), so a plain `cargo bench` never fails for a missing corpus.
//!
//! Input: posting lists derived from captured certstream/jetstream corpora
//! through the production OTLP→flatten→RowIndex pipeline (see
//! posts/treight-bitmap/evidence/bench/TRACE.md). Each list carries its
//! insertion-order positions and the production time-order permutation, so
//! both phases of the index lifecycle are measured on identical data:
//!
//! - **seal/build phase** (`bulk_*`): remap insertion→time order, then bulk
//!   encode — treight `Bitmap::from_sorted_iter[_complemented]` (the exact
//!   `sfst::build::remap_one_bitmap` encode, complement above 50% density)
//!   vs roaring `from_sorted_iter` on the same values.
//! - **accumulate phase** (`insert_arrival`): insert positions in arrival
//!   order — what `sfst::RowIndex::row` does into a `RoaringBitmap` per
//!   posting list (production), and what a hypothetical treight accumulator
//!   would pay (`RawBitmap::insert` — treight is not used this way in
//!   production; labeled as such). `insert_shuffled` is the adversarial
//!   variant of the same multiset.
//! - **query phase**: contains hit/miss, len/min/max, full iteration,
//!   and/or/and_not on real operand pairs, range counting.
//!
//! Fairness rules (also enforced in results.md):
//! - roaring is the workspace pin (netdata/roaring-rs, branch allocative) —
//!   the exact crate sfst accumulates with; never overridden.
//! - roaring's native APIs are used throughout: `from_sorted_iter`,
//!   `range_cardinality` (rank-based, not a naive iter-count), std trait
//!   set ops. Treight ops write into a reused caller buffer (its API);
//!   roaring ops allocate their result (its API) — the API difference is
//!   real and reported, not tuned away.
//! - treight bulk-build is timed via remap+sort+from_sorted_iter for ALL
//!   lists. Production's dense route (bitset set + linear scan, no sort) is
//!   slightly cheaper, so this bias disfavors treight — noted in results.
//! - every fixture is cross-validated treight-vs-roaring (len/min/max/
//!   contains/iteration equality) BEFORE timing; a mismatch panics.
//!
//! Output beyond criterion's own target/criterion tree: a `bench-fixtures.json`
//! written next to each corpus's cache (inside the derived dir, gitignored),
//! recording what was selected and the per-bucket normalization counts the
//! evidence harvest divides by.

use std::collections::HashMap;
use std::hint::black_box;
use std::path::{Path, PathBuf};

use criterion::{Criterion, criterion_group, criterion_main};
use roaring::RoaringBitmap;
use treight::{Bitmap, RawBitmap};

// ---------------------------------------------------------------------------
// Derived-cache reader (format contract with treight-bench's derive bin;
// see that crate's lib docs — the files, not the crate, are the interface).
// ---------------------------------------------------------------------------

const CACHE_FORMAT_VERSION: u32 = 1;

struct Cache {
    name: String,
    dir: PathBuf,
    universe: u32,
    /// time_order().to_sorted[insertion_pos]
    to_sorted: Vec<u32>,
    /// (kv, insertion-order positions)
    postings: Vec<(String, Vec<u32>)>,
}

fn read_le_u32s(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn load_cache(dir: &Path) -> Cache {
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).expect("meta.json"))
            .expect("parse meta.json");
    assert_eq!(
        meta["format_version"].as_u64().unwrap() as u32,
        CACHE_FORMAT_VERSION,
        "derived cache format version mismatch in {dir:?} — re-run treight-derive"
    );
    let universe = meta["rows"].as_u64().unwrap() as u32;
    let to_sorted = read_le_u32s(&std::fs::read(dir.join("to_sorted.bin")).expect("to_sorted.bin"));
    assert_eq!(to_sorted.len(), universe as usize, "to_sorted size mismatch");
    let postings_bytes = std::fs::read(dir.join("postings.bin")).expect("postings.bin");
    let mut postings = Vec::new();
    let mut rest = postings_bytes.as_slice();
    let count = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
    rest = &rest[4..];
    for _ in 0..count {
        let kv_len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
        rest = &rest[4..];
        let kv = String::from_utf8(rest[..kv_len].to_vec()).unwrap();
        rest = &rest[kv_len..];
        let card = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
        rest = &rest[4..];
        let positions = read_le_u32s(&rest[..card * 4]);
        rest = &rest[card * 4..];
        postings.push((kv, positions));
    }
    Cache {
        name: dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        dir: dir.to_path_buf(),
        universe,
        to_sorted,
        postings,
    }
}

fn discover_caches(root: &Path) -> Vec<Cache> {
    if root.join("postings.bin").is_file() {
        return vec![load_cache(root)];
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap_or_else(|e| panic!("read {root:?}: {e}"))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("postings.bin").is_file())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "no derived caches under {root:?}");
    dirs.into_iter().map(|d| load_cache(&d)).collect()
}

// ---------------------------------------------------------------------------
// Fixtures: one posting list prepared for both implementations, with the
// production encode decisions applied.
// ---------------------------------------------------------------------------

const MAX_LISTS_PER_BUCKET: usize = 12;

/// Per-list position cap for the INSERT-phase benches (both impls, same
/// data). treight's `insert` walks/skips already-serialized sibling
/// subtrees, so its cost grows with blob occupancy — an uncapped dense-list
/// insert benchmark runs to ~90 s PER ITERATION (measured), which is itself
/// one of the blog's findings: this is exactly why production accumulates
/// in roaring and bulk-encodes to treight at seal. The cap keeps the
/// insertion-cost curve measurable (append-ish AND mid-blob splices both
/// occur) in bounded wall time; `insert_positions_sampled` in
/// bench-fixtures.json carries the normalization count.
const INSERT_POSITION_CAP: usize = 10_000;

#[derive(Clone)]
struct Fixture {
    kv: String,
    card: usize,
    density: f64,
    /// Time-ordered, ascending — the encode input both sides build from.
    sorted: Vec<u32>,
    /// The production treight encode of `sorted` (complement above 50%).
    desc: Bitmap,
    blob: Vec<u8>,
    complemented: bool,
    /// The roaring twin of the same set (time-ordered values).
    rb: RoaringBitmap,
}

impl Fixture {
    fn build(cache: &Cache, kv: String, positions: &[u32]) -> Self {
        let universe = cache.universe;
        let card = positions.len();
        let mut sorted: Vec<u32> = positions.iter().map(|&p| cache.to_sorted[p as usize]).collect();
        sorted.sort_unstable();
        let complemented = card as u64 > universe as u64 / 2;
        let mut blob = Vec::new();
        let desc = if complemented {
            // Zeros of the sorted set, ascending — the stream production's
            // bitset scan produces (merge walk instead of a bitset; same
            // values, no third bitset copy needed here).
            let zeros = zeros_of(&sorted, universe);
            Bitmap::from_sorted_iter_complemented(zeros.into_iter(), universe, &mut blob)
        } else {
            Bitmap::from_sorted_iter(sorted.iter().copied(), universe, &mut blob)
        };
        let rb = RoaringBitmap::from_sorted_iter(sorted.iter().copied()).expect("ascending");
        Self {
            kv,
            card,
            density: card as f64 / universe as f64,
            sorted,
            desc,
            blob,
            complemented,
            rb,
        }
    }
}

/// Ascending stream of the values in `0..universe` NOT in `sorted`
/// (both endpoints ascending — single merge walk).
fn zeros_of(sorted: &[u32], universe: u32) -> Vec<u32> {
    let mut zeros = Vec::with_capacity((universe as usize).saturating_sub(sorted.len()));
    let mut i = 0usize;
    for v in 0..universe {
        while i < sorted.len() && sorted[i] < v {
            i += 1;
        }
        if i < sorted.len() && sorted[i] == v {
            i += 1;
        } else {
            zeros.push(v);
        }
    }
    zeros
}

/// Validate fixture equality BEFORE timing: len/min/max, sampled contains
/// (hits and gaps), and full iteration.
///
/// min/max asymmetry (mirrored in the timed op): treight's complement-aware
/// `Bitmap` has no native min/max — the direct form descends the tree in
/// O(levels) via `inner()`, the complemented form must go through iteration.
fn treight_min(f: &Fixture) -> Option<u32> {
    if f.complemented {
        f.desc.iter(&f.blob).next()
    } else {
        f.desc.inner().min(&f.blob)
    }
}

fn treight_max(f: &Fixture) -> Option<u32> {
    if f.complemented {
        f.desc.iter(&f.blob).last()
    } else {
        f.desc.inner().max(&f.blob)
    }
}

fn validate(cache_universe: u32, f: &Fixture) {
    assert_eq!(
        f.desc.len(&f.blob),
        f.rb.len(),
        "len mismatch on {:?}",
        f.kv
    );
    assert_eq!(treight_min(f), f.rb.min(), "min mismatch on {:?}", f.kv);
    assert_eq!(treight_max(f), f.rb.max(), "max mismatch on {:?}", f.kv);
    // Contains: every 97th member (hit) and its +1 neighbor when absent.
    let mut checked = 0;
    for (i, &v) in f.sorted.iter().enumerate() {
        if i % 97 == 0 {
            assert!(f.desc.contains(&f.blob, v) && f.rb.contains(v), "hit {}", v);
            checked += 1;
        }
        let miss = v.saturating_add(1);
        if miss < cache_universe && !f.sorted.contains(&(miss)) {
            assert!(
                !f.desc.contains(&f.blob, miss) && !f.rb.contains(miss),
                "miss {miss}"
            );
            checked += 1;
        }
        if checked > 256 {
            break;
        }
    }
    // Full iteration equality.
    let t: Vec<u32> = f.desc.iter(&f.blob).collect();
    let r: Vec<u32> = f.rb.iter().collect();
    assert_eq!(t, r, "iteration mismatch on {:?}", f.kv);
}

/// The three bench density buckets.
fn bench_bucket(density: f64) -> Option<&'static str> {
    Some(if density < 0.01 {
        "lt1pct"
    } else if density < 0.5 {
        "1to50pct"
    } else {
        "gt50pct"
    })
}

/// Deterministic sample of up to MAX_LISTS_PER_BUCKET lists, evenly spaced
/// across the bucket's density span (sorted by (cardinality, kv)).
fn select_bucket(postings: &[(String, Vec<u32>)], idxs: &[usize]) -> Vec<usize> {
    let mut sorted: Vec<usize> = idxs.to_vec();
    sorted.sort_by(|&a, &b| {
        postings[a]
            .1
            .len()
            .cmp(&postings[b].1.len())
            .then_with(|| postings[a].0.cmp(&postings[b].0))
    });
    let n = sorted.len();
    if n <= MAX_LISTS_PER_BUCKET {
        return sorted;
    }
    (0..MAX_LISTS_PER_BUCKET)
        .map(|i| {
            let at = (i * (n - 1)) / (MAX_LISTS_PER_BUCKET - 1);
            sorted[at]
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Operand pairs for the set-op benches (deterministic, recorded).
// ---------------------------------------------------------------------------

struct Pair {
    op: &'static str,
    kind: &'static str,
    a: usize, // fixture index into the pair-fixture arena
    b: usize,
}

/// Clone a fixture into the pair arena (blobs duplicated on purpose: the
/// pair benches must not share mutable state with the bucket fixtures).
fn push_clone(arena: &mut Vec<Fixture>, f: &Fixture) -> usize {
    arena.push(Fixture {
        kv: f.kv.clone(),
        card: f.card,
        density: f.density,
        sorted: Vec::new(),
        desc: f.desc,
        blob: f.blob.clone(),
        complemented: f.complemented,
        rb: f.rb.clone(),
    });
    arena.len() - 1
}

fn select_pairs(fixtures: &[Fixture]) -> (Vec<Fixture>, Vec<Pair>) {
    let mut arena: Vec<Fixture> = Vec::new();
    let mut pairs: Vec<Pair> = Vec::new();

    // Group by field (part before '=').
    let mut by_field: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, f) in fixtures.iter().enumerate() {
        let field = f.kv.split('=').next().unwrap_or(&f.kv);
        by_field.entry(field).or_default().push(i);
    }

    // (1) same-field disjoint: the two values of one field with the closest
    // nonzero densities, preferring the field with the largest such pair.
    let mut best: Option<(f64, usize, usize)> = None;
    for idxs in by_field.values() {
        let mut idxs = idxs.clone();
        idxs.sort_by(|&a, &b| {
            fixtures[a]
                .density
                .partial_cmp(&fixtures[b].density)
                .unwrap()
                .then(fixtures[a].kv.cmp(&fixtures[b].kv))
        });
        for w in idxs.windows(2) {
            let (a, b) = (w[0], w[1]);
            if fixtures[a].card == 0 || fixtures[b].card == 0 {
                continue;
            }
            let gap = fixtures[b].density - fixtures[a].density;
            // Prefer a substantial, similar-density pair (the interesting
            // same-field case): maximize min density, then minimize gap.
            let score = fixtures[a].density.min(fixtures[b].density) - gap;
            if best.map(|(s, _, _)| score > s).unwrap_or(true) {
                best = Some((score, a, b));
            }
        }
    }
    if let Some((_, a, b)) = best {
        pairs.push(Pair {
            op: "and",
            kind: "same-field-disjoint",
            a: push_clone(&mut arena, &fixtures[a]),
            b: push_clone(&mut arena, &fixtures[b]),
        });
    }

    // (2) cross-field overlapping: the densest list of two different fields.
    let mut densest: Vec<(f64, usize)> = fixtures
        .iter()
        .enumerate()
        .map(|(i, f)| (f.density, i))
        .collect();
    densest.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
    let mut seen_fields = std::collections::BTreeSet::new();
    let mut top_two: Vec<usize> = Vec::new();
    for (_, i) in densest {
        let field = fixtures[i].kv.split('=').next().unwrap_or(&fixtures[i].kv);
        if seen_fields.insert(field.to_string()) {
            top_two.push(i);
            if top_two.len() == 2 {
                break;
            }
        }
    }
    if top_two.len() == 2 {
        pairs.push(Pair {
            op: "or",
            kind: "cross-field",
            a: push_clone(&mut arena, &fixtures[top_two[0]]),
            b: push_clone(&mut arena, &fixtures[top_two[1]]),
        });
    }

    // (3) dense-vs-sparse: the densest (complemented) list ANDed-NOT with a
    // genuinely different, sparser list (guards: distinct fixture, distinct
    // field, both non-empty — otherwise the pair is degenerate and skipped).
    if let Some(&dense) = top_two.first() {
        let field_of = |i: usize| fixtures[i].kv.split('=').next().unwrap_or(&fixtures[i].kv);
        let mut sparse: Vec<usize> = (0..fixtures.len()).collect();
        sparse.sort_by_key(|&i| fixtures[i].card);
        if let Some(&sp) = sparse.first()
            && fixtures[sp].card > 0
            && sp != dense
            && field_of(sp) != field_of(dense)
            && fixtures[sp].kv != fixtures[dense].kv
        {
            pairs.push(Pair {
                op: "and_not",
                kind: "dense-vs-sparse",
                a: push_clone(&mut arena, &fixtures[dense]),
                b: push_clone(&mut arena, &fixtures[sp]),
            });
        }
    }
    (arena, pairs)
}

// ---------------------------------------------------------------------------
// The bench itself.
// ---------------------------------------------------------------------------

fn bench_corpus(c: &mut Criterion, cache: &Cache) {
    let universe = cache.universe;
    let mut group_meta: HashMap<String, serde_json::Value> = HashMap::new();

    // Bucket posting-list INDICES (no cloning of 2M+ lists), sample, validate.
    let mut buckets: Vec<(&'static str, Vec<usize>)> = vec![
        ("lt1pct", Vec::new()),
        ("1to50pct", Vec::new()),
        ("gt50pct", Vec::new()),
    ];
    for (i, (_kv, positions)) in cache.postings.iter().enumerate() {
        let density = positions.len() as f64 / universe as f64;
        if let Some(b) = bench_bucket(density) {
            buckets
                .iter_mut()
                .find(|(name, _)| *name == b)
                .unwrap()
                .1
                .push(i);
        }
    }

    let mut all_fixtures: Vec<Fixture> = Vec::new();
    for (bucket, lists) in &buckets {
        let selected = select_bucket(&cache.postings, lists);
        // The timed loops run over the SAMPLE (≤ MAX_LISTS_PER_BUCKET
        // lists), not the whole bucket: the same fixture set the per-list
        // normalizations in bench-fixtures.json divide by.
        let sampled: Vec<(String, Vec<u32>)> = selected
            .iter()
            .map(|&i| cache.postings[i].clone())
            .collect();
        let fixtures: Vec<Fixture> = selected
            .iter()
            .map(|&i| {
                let (kv, p) = &cache.postings[i];
                Fixture::build(cache, kv.clone(), p)
            })
            .collect();
        for f in &fixtures {
            validate(universe, f);
        }
        all_fixtures.extend(fixtures.iter().cloned());
        let positions_sampled: u64 = fixtures.iter().map(|f| f.card as u64).sum();
        // Identical-data cap for the insert phase (see INSERT_POSITION_CAP).
        let insert_sampled: Vec<(String, Vec<u32>)> = sampled
            .iter()
            .map(|(kv, p)| {
                (
                    kv.clone(),
                    if p.len() > INSERT_POSITION_CAP {
                        p[..INSERT_POSITION_CAP].to_vec()
                    } else {
                        p.clone()
                    },
                )
            })
            .collect();
        let insert_positions: u64 = insert_sampled.iter().map(|(_, p)| p.len() as u64).sum();

        group_meta.insert(
            bucket.to_string(),
            serde_json::json!({
                "lists_in_bucket": lists.len(),
                "lists_sampled": fixtures.len(),
                "positions_sampled": positions_sampled,
                "insert_positions_sampled": insert_positions,
            }),
        );

        let prefix = format!("{}/{}/", cache.name, bucket);

        // ---- (a) bulk build (seal phase) ----
        // treight: remap + sort + production encode (complement >50%).
        c.bench_function(
            format!("{}bulk_build/treight", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut total = 0usize;
                for (kv, positions) in sampled.iter() {
                    let card = positions.len();
                    let mut sorted: Vec<u32> =
                        positions.iter().map(|&p| cache.to_sorted[p as usize]).collect();
                    sorted.sort_unstable();
                    let mut blob = Vec::new();
                    if card as u64 > universe as u64 / 2 {
                        let zeros = zeros_of(&sorted, universe);
                        black_box(Bitmap::from_sorted_iter_complemented(
                            zeros.into_iter(),
                            universe,
                            &mut blob,
                        ));
                    } else {
                        black_box(Bitmap::from_sorted_iter(
                            sorted.iter().copied(),
                            universe,
                            &mut blob,
                        ));
                    }
                    total += blob.len();
                    black_box(&kv);
                }
                black_box(total)
            })
        });
        // roaring: same remap + sort + from_sorted_iter (its bulk API).
        c.bench_function(
            format!("{}bulk_build/roaring", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut total = 0usize;
                for (kv, positions) in sampled.iter() {
                    let mut sorted: Vec<u32> =
                        positions.iter().map(|&p| cache.to_sorted[p as usize]).collect();
                    sorted.sort_unstable();
                    let rb = RoaringBitmap::from_sorted_iter(sorted.iter().copied())
                        .expect("ascending");
                    total += rb.len() as usize;
                    black_box(&kv);
                }
                black_box(total)
            })
        });

        // ---- (b) incremental insert ----
        // Production accumulate: arrival-order inserts (ascending per list).
        c.bench_function(
            format!("{}insert_arrival/roaring", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut total = 0u64;
                for (kv, positions) in insert_sampled.iter() {
                    let mut rb = RoaringBitmap::new();
                    for &p in positions {
                        rb.insert(p);
                    }
                    total += rb.len();
                    black_box(&kv);
                }
                black_box(total)
            })
        });
        c.bench_function(
            format!("{}insert_arrival/treight", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut total = 0u64;
                for (kv, positions) in insert_sampled.iter() {
                    let raw = RawBitmap::empty(universe);
                    let mut blob = Vec::new();
                    for &p in positions {
                        raw.insert(&mut blob, p);
                    }
                    total += raw.len(&blob);
                    black_box(&kv);
                }
                black_box(total)
            })
        });
        // Adversarial: same multisets, shuffled (deterministic seed).
        c.bench_function(
            format!("{}insert_shuffled/roaring", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut total = 0u64;
                for (kv, positions) in insert_sampled.iter() {
                    let mut order = positions.clone();
                    splitmix_shuffle(&mut order);
                    let mut rb = RoaringBitmap::new();
                    for p in order {
                        rb.insert(p);
                    }
                    total += rb.len();
                    black_box(&kv);
                }
                black_box(total)
            })
        });
        c.bench_function(
            format!("{}insert_shuffled/treight", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut total = 0u64;
                for (kv, positions) in insert_sampled.iter() {
                    let mut order = positions.clone();
                    splitmix_shuffle(&mut order);
                    let raw = RawBitmap::empty(universe);
                    let mut blob = Vec::new();
                    for p in order {
                        raw.insert(&mut blob, p);
                    }
                    total += raw.len(&blob);
                    black_box(&kv);
                }
                black_box(total)
            })
        });

        // ---- (e) contains hit / miss ----
        let hits: Vec<Vec<u32>> = fixtures
            .iter()
            .map(|f| sample_spread(&f.sorted, 64).collect())
            .collect();
        let misses: Vec<Vec<u32>> = fixtures
            .iter()
            .map(|f| {
                sample_spread(&f.sorted, 64)
                    .filter_map(|v| {
                        let m = v.saturating_add(3);
                        (m < universe && !f.rb.contains(m)).then_some(m)
                    })
                    .collect()
            })
            .collect();
        let hits_total: u64 = hits.iter().map(|h| h.len() as u64).sum();
        let misses_total: u64 = misses.iter().map(|m| m.len() as u64).sum();
        group_meta.get_mut(*bucket).unwrap()["hits_sampled"] =
            serde_json::json!(hits_total);
        group_meta.get_mut(*bucket).unwrap()["misses_sampled"] =
            serde_json::json!(misses_total);
        c.bench_function(
            format!("{}contains_hit/treight", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut n = 0;
                for (f, hs) in fixtures.iter().zip(&hits) {
                    for &v in hs {
                        n += f.desc.contains(&f.blob, v) as usize;
                    }
                }
                black_box(n)
            })
        });
        c.bench_function(
            format!("{}contains_hit/roaring", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut n = 0;
                for (f, hs) in fixtures.iter().zip(&hits) {
                    for &v in hs {
                        n += f.rb.contains(v) as usize;
                    }
                }
                black_box(n)
            })
        });
        c.bench_function(
            format!("{}contains_miss/treight", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut n = 0;
                for (f, ms) in fixtures.iter().zip(&misses) {
                    for &v in ms {
                        n += f.desc.contains(&f.blob, v) as usize;
                    }
                }
                black_box(n)
            })
        });
        c.bench_function(
            format!("{}contains_miss/roaring", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut n = 0;
                for (f, ms) in fixtures.iter().zip(&misses) {
                    for &v in ms {
                        n += f.rb.contains(v) as usize;
                    }
                }
                black_box(n)
            })
        });

        // ---- (f) len + min + max ----
        // treight: native len; min/max native on the direct form
        // (O(levels) tree descent), iter-based on the complemented form
        // (no native min/max there). roaring: native throughout.
        c.bench_function(
            format!("{}len_min_max/treight", prefix).as_str(),
        
            |b| {
                b.iter(|| {
                    let mut acc = 0u64;
                    for f in &fixtures {
                        acc += f.desc.len(&f.blob);
                        acc = acc.wrapping_add(treight_min(f).unwrap_or(0) as u64);
                        acc = acc.wrapping_add(treight_max(f).unwrap_or(0) as u64);
                    }
                    black_box(acc)
                })
            },
        );
        c.bench_function(
            format!("{}len_min_max/roaring", prefix).as_str(),
        
            |b| {
                b.iter(|| {
                    let mut acc = 0u64;
                    for f in &fixtures {
                        acc += f.rb.len();
                        acc = acc.wrapping_add(f.rb.min().unwrap_or(0) as u64);
                        acc = acc.wrapping_add(f.rb.max().unwrap_or(0) as u64);
                    }
                    black_box(acc)
                })
            },
        );

        // ---- (g) full iteration ----
        c.bench_function(
            format!("{}iterate/treight", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut acc = 0u64;
                for f in &fixtures {
                    for v in f.desc.iter(&f.blob) {
                        acc += v as u64;
                    }
                }
                black_box(acc)
            })
        });
        c.bench_function(
            format!("{}iterate/roaring", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut acc = 0u64;
                for f in &fixtures {
                    for v in f.rb.iter() {
                        acc += v as u64;
                    }
                }
                black_box(acc)
            })
        });

        // ---- (i) range counting ----
        // Three real query windows: first quarter, middle half, last tenth.
        let ranges: [(u32, u32); 3] = [
            (0, universe / 4),
            (universe / 4, universe / 4 * 3),
            (universe - universe / 10, universe),
        ];
        c.bench_function(
            format!("{}range_count/treight", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut acc = 0u64;
                for f in &fixtures {
                    for &(lo, hi) in &ranges {
                        acc += f.desc.range_cardinality(&f.blob, lo..hi);
                    }
                }
                black_box(acc)
            })
        });
        c.bench_function(
            format!("{}range_count/roaring", prefix).as_str(),
         |b| {
            b.iter(|| {
                let mut acc = 0u64;
                for f in &fixtures {
                    for &(lo, hi) in &ranges {
                        acc += f.rb.range_cardinality(lo..hi);
                    }
                }
                black_box(acc)
            })
        });

        // ---- (c)+(d) serialized / heap sizes: constants, printed not timed ----
        let treight_blob: u64 = fixtures.iter().map(|f| f.blob.len() as u64).sum();
        let roaring_serialized: u64 = fixtures.iter().map(|f| f.rb.serialized_size() as u64).sum();
        let roaring_heap: u64 = fixtures
            .iter()
            .map(|f| allocative::size_of_unique(&f.rb) as u64)
            .sum();
        println!(
            "{prefix}sizes: {} sampled lists, {} positions | treight {} B blob, roaring {} B serialized, {} B heap",
            fixtures.len(),
            positions_sampled,
            treight_blob,
            roaring_serialized,
            roaring_heap
        );
    }

    // ---- (h) boolean ops on real operand pairs (corpus-global: a query's
    // operands span density buckets; guards pick distinct lists and, for the
    // cross-field / dense-vs-sparse kinds, distinct fields) ----
    let (pair_arena, pairs) = select_pairs(&all_fixtures);
    for p in &pairs {
        let a = &pair_arena[p.a];
        let b = &pair_arena[p.b];
        let name = format!("{}/pairs/{}/{}/treight", cache.name, p.op, p.kind);
        c.bench_function(name.as_str(), |bencher| {
            bencher.iter(|| {
                let mut out = Vec::new();
                let result = match p.op {
                    "and" => a.desc.and(&a.blob, &b.desc, &b.blob, &mut out),
                    "or" => a.desc.or(&a.blob, &b.desc, &b.blob, &mut out),
                    _ => a.desc.and_not(&a.blob, &b.desc, &b.blob, &mut out),
                };
                black_box((result, out.len()))
            })
        });
        let name = format!("{}/pairs/{}/{}/roaring", cache.name, p.op, p.kind);
        c.bench_function(name.as_str(), |bencher| {
            bencher.iter(|| {
                let result = match p.op {
                    "and" => &a.rb & &b.rb,
                    "or" => &a.rb | &b.rb,
                    _ => &a.rb - &b.rb,
                };
                black_box(result.len())
            })
        });
    }

    // Record what this run selected, for the evidence harvest.
    let fixtures_path = cache.dir.join("bench-fixtures.json");
    let doc = serde_json::json!({
        "corpus": cache.name,
        "universe": universe,
        "buckets": group_meta,
        "pairs": pairs.iter().map(|p| serde_json::json!({
            "op": p.op, "kind": p.kind,
            "a": pair_arena[p.a].kv, "b": pair_arena[p.b].kv,
            "a_density": pair_arena[p.a].density,
            "b_density": pair_arena[p.b].density,
            "a_complemented": pair_arena[p.a].complemented,
            "a_card": pair_arena[p.a].card, "b_card": pair_arena[p.b].card,
        })).collect::<Vec<_>>(),
    });
    let _ = std::fs::write(&fixtures_path, serde_json::to_string_pretty(&doc).unwrap());
}

/// Evenly spaced sample of up to `n` members (ascending input).
fn sample_spread(sorted: &[u32], n: usize) -> impl Iterator<Item = u32> + '_ {
    let step = (sorted.len() / n.max(1)).max(1);
    sorted.iter().step_by(step).copied()
}

/// Deterministic in-place shuffle (SplitMix64 — fixed seed, reproducible).
fn splitmix_shuffle(v: &mut [u32]) {
    let mut state: u64 = 0x9E3779B97F4A7C15;
    let mut next = || {
        state = state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    };
    for i in (1..v.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
}

fn bench_vs_roaring(c: &mut Criterion) {
    let Ok(root) = std::env::var("TREIGHT_BENCH_DERIVED") else {
        eprintln!("TREIGHT_BENCH_DERIVED not set; skipping vs_roaring benches");
        return;
    };
    let caches = discover_caches(Path::new(&root));
    eprintln!(
        "bench corpora: {:?} (universe {})",
        caches.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        caches.first().map(|c| c.universe).unwrap_or(0)
    );
    for cache in &caches {
        bench_corpus(c, cache);
    }
}

fn config() -> Criterion {
    Criterion::default()
        .warm_up_time(std::time::Duration::from_millis(1000))
        .measurement_time(std::time::Duration::from_millis(2000))
        .sample_size(50)
}

criterion_group! { name = benches; config = config(); targets = bench_vs_roaring }
criterion_main!(benches);
