//! dbengine readers and writers against tier directories the C agent wrote. The snapshots are not committed (D50.2):
//! `NETDATA_DBENGINE_FIXTURES` points at them, and without it each test says it skipped. The expected outputs are
//! the fixture checkers' (`results/<snapshot>/`). Brief `knowledge/brief-dbengine-s0.md` §7.2 L3 in the status
//! repository.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use netdata_agent_storage::dbengine::format::descriptor::{
    PAGE_TYPE_GORILLA_32BIT, PageDescriptor, validate_extent_page_descr,
};
use netdata_agent_storage::dbengine::format::extent::{self, PageSlot};
use netdata_agent_storage::dbengine::format::journal_v1::{self, Event};
use netdata_agent_storage::dbengine::format::journal_v2::{self, Retention, Verdict};
use netdata_agent_storage::dbengine::format::page::DiskPage;
use netdata_agent_storage::dbengine::format::{BLOCK_SIZE, ReadAt, inspect, superblock};
use netdata_agent_storage::storage_number::{SN_EMPTY_SLOT, SN_FLAG_NOT_ANOMALOUS, unpack};
use serde_json::{Value, json};

const SNAPSHOTS: [&str; 6] = [
    "run1",
    "runR",
    "run2",
    "orig-20260924/run1",
    "orig-20260924/runR",
    "orig-20260924/run2",
];
const TIERS: [&str; 3] = [
    "cache/dbengine",
    "cache/dbengine-tier1",
    "cache/dbengine-tier2",
];

fn fixtures() -> Option<PathBuf> {
    let dir = std::env::var_os("NETDATA_DBENGINE_FIXTURES").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("skipped: NETDATA_DBENGINE_FIXTURES unset");
    }
    dir
}

/// The checkers' outputs of a snapshot: `results/` names it with `/` turned into `-`.
fn results(fx: &Path, snapshot: &str) -> PathBuf {
    fx.join("results").join(snapshot.replace('/', "-"))
}

fn json_file(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// Every `.njfv2` of the snapshots with its `.njf`.
fn v2_files(fx: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    for snapshot in SNAPSHOTS {
        for tier in TIERS {
            for f in inspect::tier_files(&fx.join(snapshot).join(tier)).unwrap() {
                if f.njfv2.exists() {
                    out.push((f.njf, f.njfv2));
                }
            }
        }
    }
    out
}

#[test]
fn v2_files_validate_and_rebuild_byte_identical() {
    let Some(fx) = fixtures() else {
        return;
    };
    let files = v2_files(&fx);
    assert_eq!(files.len(), 14);
    for (v1, v2) in files {
        let expected = fs::read(&v2).unwrap();
        let v1_file = File::open(&v1).unwrap();
        let v1_size = v1_file.size().unwrap();
        let verdict = journal_v2::validate(&expected[..], v1_size as u32, true).unwrap();
        assert_eq!(verdict, Verdict::Ok, "{}", v2.display());
        let replay = journal_v1::replay(&v1_file, v1_size).unwrap();
        let built = journal_v2::from_v1(&replay, v1_size, 0, &mut Retention::default()).unwrap();
        assert!(
            built == expected,
            "{} differs from its rebuild",
            v2.display()
        );
    }
}

#[test]
fn reports_and_totals_equal_the_checkers() {
    let Some(fx) = fixtures() else {
        return;
    };
    for snapshot in SNAPSHOTS {
        let res = results(&fx, snapshot);
        for (t, tier) in TIERS.iter().enumerate() {
            let mut mine = inspect::tier_report(&fx.join(snapshot).join(tier)).unwrap();
            let mut theirs = json_file(&res.join(format!("inspect-tier{t}.json")));
            mine.as_object_mut().unwrap().remove("dir");
            theirs.as_object_mut().unwrap().remove("dir");
            assert!(mine == theirs, "{snapshot} tier {t}: {mine:#}");
        }
        let totals = inspect::cache_totals(&fx.join(snapshot).join("cache")).unwrap();
        assert_eq!(totals, json_file(&res.join("totals.json")), "{snapshot}");
    }
}

/// C's own readers take every file as valid: superblocks, every journal record, every extent and page, every v2
/// file with the integrity check; and each extent re-encodes to C's bytes.
#[test]
fn c_readers_accept_every_file() {
    let Some(fx) = fixtures() else {
        return;
    };
    let (mut extents, mut pages) = (0, 0);
    for snapshot in SNAPSHOTS {
        for tier in TIERS {
            for f in inspect::tier_files(&fx.join(snapshot).join(tier)).unwrap() {
                let name = f.ndf.display().to_string();
                let ndf = fs::read(&f.ndf).unwrap();
                let njf = fs::read(&f.njf).unwrap();
                assert_eq!(superblock::check_datafile(&ndf), Ok(()), "{name}");
                assert_eq!(superblock::check_journal(&njf), Ok(()), "{name}");
                let replay = journal_v1::replay(&njf[..], njf.len() as u64).unwrap();
                assert!(replay.read_error.is_none(), "{name}");
                for e in &replay.events {
                    let Event::StoreData { data, .. } = e else {
                        panic!("{name}: {e:?}");
                    };
                    let start = data.extent_offset as usize;
                    let bytes = &ndf[start..start + data.extent_size as usize];
                    let x = extent::decode(bytes).unwrap();
                    assert!(x.crc_ok && !x.read_error, "{name} extent at {start}");
                    assert_eq!(x.descriptors, data.descriptors, "{name} extent at {start}");
                    let mut held = Vec::new();
                    for (i, d) in x.descriptors.iter().enumerate() {
                        let vd = validate_extent_page_descr(d, 0, 0, false);
                        assert!(
                            vd.valid && !vd.updated,
                            "{name} extent at {start} page {i}: {vd:?}"
                        );
                        let PageSlot::Page(page) = x.page(i) else {
                            panic!("{name} extent at {start} page {i} is not held");
                        };
                        held.push((*d, page));
                        pages += 1;
                    }
                    let again = extent::encode(&held, x.compression);
                    assert!(
                        again.bytes[..again.size_bytes] == *bytes,
                        "{name} extent at {start} re-encodes differently"
                    );
                    extents += 1;
                }
                if f.njfv2.exists() {
                    let v2 = fs::read(&f.njfv2).unwrap();
                    let verdict = journal_v2::validate(&v2[..], njf.len() as u32, true).unwrap();
                    assert_eq!(verdict, Verdict::Ok, "{name}");
                }
            }
        }
    }
    assert_eq!(extents, 318);
    assert!(pages > 0);
}

/// The generator's chart and dimension per metric uuid (`metric-map.tsv`: `b6.c<N>`, `d<N>`).
fn generator_metrics(res: &Path) -> HashMap<[u8; 16], (i64, i64)> {
    let mut out = HashMap::new();
    for line in fs::read_to_string(res.join("metric-map.tsv"))
        .unwrap()
        .lines()
    {
        let cols: Vec<_> = line.split('\t').collect();
        let (Some(c), Some(d)) = (
            cols.get(1).and_then(|c| c.strip_prefix("b6.c")),
            cols.get(2).and_then(|d| d.strip_prefix('d')),
        ) else {
            continue;
        };
        let mut uuid = [0u8; 16];
        for (i, b) in uuid.iter_mut().enumerate() {
            *b = u8::from_str_radix(&cols[0][2 * i..2 * i + 2], 16).unwrap();
        }
        out.insert(uuid, (c.parse().unwrap(), d.parse().unwrap()));
    }
    out
}

/// The generator's workload window and gap (`expected.json` `driver`).
struct Driver {
    start: i64,
    end: i64,
    gapfrom: i64,
    gapto: i64,
}

impl Driver {
    fn of(expected: &Value) -> Self {
        let n = |k: &str| expected["driver"][k].as_i64().unwrap();
        Driver {
            start: n("start"),
            end: n("end"),
            gapfrom: n("gapfrom"),
            gapto: n("gapto"),
        }
    }

    /// What the generator sent for a dimension at `t`, whether it was anomalous; `None` for an empty slot.
    fn sample(&self, c: i64, d: i64, t: i64) -> Option<(f64, bool)> {
        if (c == 3 && (self.gapfrom..self.gapto).contains(&t))
            || (c == 1 && d == 4 && t % 1000 == 500)
        {
            return None;
        }
        let v = ((t / 10) % 1000) as f64 + c as f64 * 0.5 + d as f64 * 0.125;
        Some((v, c == 2 && t % 97 == 0))
    }
}

fn count(c: &mut BTreeMap<String, u64>, k: impl Into<String>) {
    *c.entry(k.into()).or_insert(0) += 1;
}

/// `checkvalues.py` on tier 0: every point of the generator's metrics against what it sent; the last page written
/// for a time wins.
fn tier0_values(dir: &Path, metrics: &HashMap<[u8; 16], (i64, i64)>, driver: &Driver) -> Value {
    let mut stats = BTreeMap::new();
    let mut points: HashMap<(i64, i64), HashMap<i64, u32>> = HashMap::new();
    inspect::for_each_page(dir, |d: &PageDescriptor, bytes| {
        let Some(&m) = metrics.get(&d.uuid) else {
            count(&mut stats, "other_metric_pages");
            return;
        };
        count(&mut stats, format!("pages_type{}", d.page_type));
        let st = (d.start_time_ut / 1_000_000) as i64;
        let values = DiskPage::from_disk(d.page_type, bytes)
            .and_then(|p| p.storage_numbers())
            .unwrap_or_else(|| panic!("page type {} at tier 0", d.page_type));
        let ue = if d.page_type == PAGE_TYPE_GORILLA_32BIT {
            let n = i64::from(d.gorilla_entries());
            if values.len() as i64 != n {
                count(&mut stats, "gorilla_entries_mismatch");
            }
            if n > 1 {
                i64::from(d.gorilla_delta_s()) / (n - 1)
            } else {
                1
            }
        } else {
            let (et, n) = ((d.end_time_ut() / 1_000_000) as i64, values.len() as i64);
            if n > 1 { (et - st) / (n - 1) } else { 1 }
        };
        let series = points.entry(m).or_default();
        for (i, v) in (0i64..).zip(values) {
            series.insert(st + i * ue, v);
        }
    })
    .unwrap();
    let (mut checked, mut bad, mut anomaly_ok) = (0u64, 0u64, 0u64);
    for (&(c, d), series) in &points {
        for (&t, &sn) in series {
            let Some((e, anomalous)) = driver.sample(c, d, t) else {
                if sn == SN_EMPTY_SLOT {
                    count(&mut stats, "empty_ok");
                } else {
                    bad += 1;
                }
                continue;
            };
            if (unpack(sn) - e).abs() > 1e-6 * e.abs().max(1.0) {
                bad += 1;
            }
            anomaly_ok += u64::from((sn & SN_FLAG_NOT_ANOMALOUS != 0) != anomalous);
            checked += 1;
        }
    }
    json!({"points_checked": checked, "bad": bad, "anomaly_bit_ok": anomaly_ok, "stats": stats})
}

/// `checktier.py` on tier 1 or 2 (`grp` seconds per point): every record of the generator's metrics against the
/// aggregate of what it sent in the record's window. An empty window takes a record of count 0, or C's gap fill
/// (NaN sum, min and max, count 1), which the Python checker counts as bad.
fn tier_records(
    dir: &Path,
    metrics: &HashMap<[u8; 16], (i64, i64)>,
    driver: &Driver,
    grp: i64,
) -> Value {
    let (mut checked, mut bad, mut empty, mut gap_fills, mut other) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut per_page = BTreeMap::new();
    inspect::for_each_page(dir, |d: &PageDescriptor, bytes| {
        let Some(&(c, dim)) = metrics.get(&d.uuid) else {
            other += 1;
            return;
        };
        let (st, et) = (
            (d.start_time_ut / 1_000_000) as i64,
            (d.end_time_ut() / 1_000_000) as i64,
        );
        let records = match DiskPage::from_disk(d.page_type, bytes) {
            Some(DiskPage::Tier1(records)) => records,
            _ => Vec::new(),
        };
        let n = records.len() as i64;
        let ue = if n > 1 { (et - st) / (n - 1) } else { grp };
        *per_page.entry(n).or_insert(0u64) += 1;
        for (i, r) in (0i64..).zip(&records) {
            let t = st + i * ue;
            let window: Vec<_> = (t - grp + 1..=t)
                .filter(|x| (driver.start..=driver.end).contains(x))
                .filter_map(|x| driver.sample(c, dim, x))
                .collect();
            let ok = if window.is_empty() {
                empty += 1;
                let gap_fill = r.sum.is_nan()
                    && r.min.is_nan()
                    && r.max.is_nan()
                    && r.count == 1
                    && r.anomaly_count == 0;
                gap_fills += u64::from(gap_fill);
                r.count == 0 || gap_fill
            } else {
                let sum: f64 = window.iter().map(|(v, _)| v).sum();
                let min = window.iter().map(|(v, _)| *v).fold(f64::INFINITY, f64::min);
                let max = window
                    .iter()
                    .map(|(v, _)| *v)
                    .fold(f64::NEG_INFINITY, f64::max);
                let anomalies = window.iter().filter(|(_, a)| *a).count();
                (
                    r.sum,
                    r.min,
                    r.max,
                    usize::from(r.count),
                    usize::from(r.anomaly_count),
                ) == (sum as f32, min as f32, max as f32, window.len(), anomalies)
            };
            bad += u64::from(!ok);
            checked += 1;
        }
    })
    .unwrap();
    let per_page: BTreeMap<String, u64> = per_page
        .into_iter()
        .map(|(n, c)| (n.to_string(), c))
        .collect();
    json!({
        "records_checked": checked,
        "bad": bad,
        "gap_fills": gap_fills,
        "empty_points": empty,
        "other_metric_pages": other,
        "records_per_page": per_page,
    })
}

#[test]
fn values_are_what_the_generator_sent() {
    let Some(fx) = fixtures() else {
        return;
    };
    for snapshot in SNAPSHOTS {
        let res = results(&fx, snapshot);
        let expected = json_file(&res.join("expected.json"));
        let (metrics, driver) = (generator_metrics(&res), Driver::of(&expected));
        let cache = fx.join(snapshot).join("cache");
        let mut want = expected["tiers"]["0"]["values"].clone();
        want.as_object_mut().unwrap().remove("first_metric");
        let got = tier0_values(&cache.join("dbengine"), &metrics, &driver);
        assert_eq!(got, want, "{snapshot} tier 0");
        for (t, grp) in [(1, 60), (2, 3600)] {
            let want = &expected["tiers"][t.to_string()]["records"];
            let got = tier_records(
                &cache.join(format!("dbengine-tier{t}")),
                &metrics,
                &driver,
                grp,
            );
            // the Python checker's bad records are exactly C's gap fills
            assert_eq!(
                (got["bad"].as_u64(), got["gap_fills"].as_u64()),
                (Some(0), want["bad"].as_u64()),
                "{snapshot} tier {t}"
            );
            for k in [
                "records_checked",
                "empty_points",
                "other_metric_pages",
                "records_per_page",
            ] {
                assert_eq!(got[k], want[k], "{snapshot} tier {t} {k}");
            }
        }
    }
}

#[test]
fn write_in_place_over_a_larger_file_is_the_fresh_image() {
    let Some(fx) = fixtures() else {
        return;
    };
    let (njf, v2) = v2_files(&fx).remove(0);
    let image = fs::read(&v2).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(v2.file_name().unwrap());
    fs::write(&path, vec![0x5A; image.len() + 3 * BLOCK_SIZE]).unwrap();
    let j = fs::read(&njf).unwrap();
    let replay = journal_v1::replay(&j[..], j.len() as u64).unwrap();
    let built = journal_v2::from_v1(&replay, j.len() as u64, 0, &mut Retention::default()).unwrap();
    journal_v2::write_in_place(&path, &built).unwrap();
    assert!(fs::read(&path).unwrap() == image);
}
