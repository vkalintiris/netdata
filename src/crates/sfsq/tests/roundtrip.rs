//! Write a synthetic SFST file with `sfst::Writer`, open it via
//! `sfsq::Reader`, and verify that summary fields round-trip.

use std::fs;

use fst_index::FstIndex;
use sfst::{FileSummary, StreamEntry, Writer, pack, pack_metadata};
use tempfile::tempdir;

#[test]
fn roundtrip_summary() {
    let summary = FileSummary {
        min_timestamp_s: 1_700_000_000,
        max_timestamp_s: 1_700_003_600,
        total_logs: 1_234,
        stream: StreamEntry::new("prod", "api"),
    };

    let fst: FstIndex<u64> = FstIndex::build([("a", 1u64)]).unwrap();
    let mut writer = Writer::new();
    writer.set_summary(pack_metadata(&summary, 1).unwrap());
    writer.set_primary(pack(&fst, 1).unwrap());

    let mut buf = Vec::new();
    writer.write_to(&mut buf).unwrap();

    let dir = tempdir().unwrap();
    let path = dir.path().join("test.sfst");
    fs::write(&path, &buf).unwrap();

    let reader = sfsq::Reader::open(&path).unwrap();
    assert_eq!(reader.summary().total_logs, 1_234);
    assert_eq!(reader.summary().min_timestamp_s, 1_700_000_000);
    assert_eq!(reader.summary().max_timestamp_s, 1_700_003_600);
    assert_eq!(reader.stream().namespace, "prod");
    assert_eq!(reader.stream().name, "api");
    assert_eq!(reader.chunk_count(), 0);
}
