//! `storage_number` against vectors generated from the C implementation (`tests/oracle/gen-vectors.sh`).

use std::path::PathBuf;

use netdata_agent_storage::storage_number::{pack, unpack};

fn rows(name: &str) -> Vec<Vec<String>> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "vectors", name]
        .iter()
        .collect();
    std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

fn bits(hex: &str) -> u64 {
    u64::from_str_radix(hex, 16).unwrap()
}

#[test]
fn pack_matches_c() {
    let rows = rows("pack.tsv");
    assert!(rows.len() > 20_000);
    let failures: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            let value = f64::from_bits(bits(&r[0]));
            let flags: u32 = r[1].parse().unwrap();
            let want: u32 = r[2].parse().unwrap();
            let got = pack(value, flags);
            (got != want).then(|| {
                format!(
                    "pack({value:e} [{}], {flags:#x}) = {got:#010x}, C {want:#010x}",
                    r[0]
                )
            })
        })
        .take(10)
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn unpack_matches_c() {
    let rows = rows("unpack.tsv");
    assert!(rows.len() > 20_000);
    let failures: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            let packed: u32 = r[0].parse().unwrap();
            let want = bits(&r[1]);
            let got = unpack(packed).to_bits();
            // NaN payloads are not part of the contract; C returns the NAN constant.
            let same =
                got == want || (f64::from_bits(got).is_nan() && f64::from_bits(want).is_nan());
            (!same).then(|| format!("unpack({packed:#010x}) = {got:016x}, C {want:016x}"))
        })
        .take(10)
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
