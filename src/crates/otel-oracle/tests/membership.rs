//! Membership against a small store written the way the agent writes one: WAL
//! frames from the agent's own frame preparation, and a sealed file from the
//! store format's writer. The rows it reads must be the rows the calculator
//! rebuilds from the same requests.

mod common;

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use common::{BASE_NS, request, write_sealed, write_wal};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use otel_oracle::membership::{
    self, Membership, MembershipError, RowKey, SealedCache, UnitKind, WalInfo,
};
use otel_oracle::model::spans_of_request;

fn keys(requests: &[&ExportTraceServiceRequest]) -> Vec<RowKey> {
    requests
        .iter()
        .flat_map(|request| spans_of_request(request, 0))
        .map(|span| RowKey::of(&span))
        .collect()
}

fn kinds_and_rows(membership: &Membership) -> Vec<(UnitKind, Vec<RowKey>)> {
    membership
        .units
        .iter()
        .map(|unit| (unit.kind.clone(), unit.rows.clone()))
        .collect()
}

#[test]
fn a_wal_splits_into_chunks_and_a_tail_holding_the_rebuilt_rows() {
    let store = tempfile::tempdir().unwrap();
    let (a, b, c) = (request(1, 1, 4), request(2, 11, 4), request(3, 21, 3));
    let wal = write_wal(store.path(), &[&a, &b, &c], 0);

    let read = membership::read_store(store.path(), 5).unwrap();

    assert_eq!(
        read.wals,
        vec![WalInfo {
            bytes: fs::metadata(&wal).unwrap().len(),
            path: wal,
            frames: 3,
            entries: 11,
            frame_times: Some((1, 3)),
        }]
    );

    assert_eq!(
        kinds_and_rows(&read),
        vec![
            (UnitKind::Chunk(0), keys(&[&a, &b])),
            (UnitKind::Tail(2), keys(&[&c])),
        ]
    );
    assert!(read.stale_wals.is_empty());
    let first = BASE_NS / 1_000_000_000;
    assert_eq!(
        read.units[0].seconds,
        Some((
            u32::try_from(first + 1).unwrap(),
            u32::try_from(first + 14).unwrap()
        ))
    );
}

#[test]
fn a_sealed_file_hides_the_wal_it_was_sealed_from() {
    let store = tempfile::tempdir().unwrap();
    let (a, b) = (request(1, 1, 3), request(2, 11, 3));
    let wal = write_wal(store.path(), &[&a, &b], 0);
    let rows = keys(&[&a, &b]);
    write_sealed(store.path(), &wal, &rows);

    let read = membership::read_store(store.path(), 2).unwrap();

    assert_eq!(kinds_and_rows(&read), vec![(UnitKind::Sealed, rows)]);
    assert!(read.wals.is_empty());
}

/// A sealed file is read once while its length stays the same, read again
/// when it changes, and forgotten once it is gone.
#[test]
fn sealed_files_are_read_once_per_length() {
    let store = tempfile::tempdir().unwrap();
    let (a, b) = (request(1, 1, 3), request(2, 11, 3));
    let wal = write_wal(store.path(), &[&a, &b], 0);
    let rows = keys(&[&a, &b]);
    let sealed = write_sealed(store.path(), &wal, &rows);
    let unreadable = |path: &Path| {
        let bytes = fs::metadata(path).unwrap().len();
        fs::write(path, vec![0; bytes as usize]).unwrap();
    };
    let mut cache = SealedCache::default();
    let mut read = || membership::read_store_with(store.path(), 2, &mut cache);

    let first = read().unwrap();
    assert_eq!(
        kinds_and_rows(&first),
        vec![(UnitKind::Sealed, rows.clone())]
    );

    unreadable(&sealed);
    assert_eq!(read().unwrap(), first);
    assert!(membership::read_store(store.path(), 2).is_err());

    write_sealed(store.path(), &wal, &rows[..2]);
    assert_eq!(
        kinds_and_rows(&read().unwrap()),
        vec![(UnitKind::Sealed, rows[..2].to_vec())]
    );

    fs::remove_file(&sealed).unwrap();
    let unsealed = read().unwrap();
    assert!(
        unsealed
            .units
            .iter()
            .all(|unit| unit.kind != UnitKind::Sealed)
    );

    write_sealed(store.path(), &wal, &rows[..2]);
    unreadable(&sealed);
    assert!(matches!(read(), Err(MembershipError::Sealed(path, _)) if path == sealed));
}

#[test]
fn a_wal_left_by_an_older_agent_instance_is_set_aside() {
    let store = tempfile::tempdir().unwrap();
    let a = request(1, 1, 2);
    let newest = write_wal(store.path(), &[&a], 0);

    let name = newest.file_name().unwrap().to_str().unwrap().to_string();
    let parts: Vec<&str> = name.splitn(3, '-').collect();
    let older = newest.with_file_name(format!(
        "{}-{}-{}",
        parts[0],
        "0".repeat(31) + "1",
        parts[2]
    ));
    fs::copy(&newest, &older).unwrap();
    let an_hour_ago = SystemTime::now() - Duration::from_secs(3_600);
    fs::File::options()
        .write(true)
        .open(&older)
        .unwrap()
        .set_modified(an_hour_ago)
        .unwrap();

    let read = membership::read_store(store.path(), 100).unwrap();

    assert_eq!(
        read.wals.iter().map(|wal| &wal.path).collect::<Vec<_>>(),
        vec![&newest]
    );
    let second = u32::try_from(BASE_NS / 1_000_000_000).unwrap();
    assert_eq!(
        read.stale_wals,
        vec![membership::StaleWal {
            path: older,
            seconds: Some((second + 1, second + 2)),
            readable: true,
        }]
    );
    assert!(read.stale_wals[0].overlaps(second + 2, second + 3));
    assert!(!read.stale_wals[0].overlaps(second + 3, second + 4));
    assert_eq!(
        kinds_and_rows(&read),
        vec![(UnitKind::Tail(0), keys(&[&a]))]
    );
}

#[test]
fn a_wal_cut_mid_frame_is_not_settled() {
    let store = tempfile::tempdir().unwrap();
    let wal = write_wal(store.path(), &[&request(1, 1, 3)], 0);
    let length = fs::metadata(&wal).unwrap().len();
    fs::File::options()
        .write(true)
        .open(&wal)
        .unwrap()
        .set_len(length - 3)
        .unwrap();

    let error = membership::read_store(store.path(), 100).unwrap_err();

    assert!(matches!(error, MembershipError::NotSettled(path, _) if path == wal));
}

#[test]
fn a_frame_whose_count_disagrees_with_its_spans_is_reported() {
    let store = tempfile::tempdir().unwrap();
    let wal = write_wal(store.path(), &[&request(1, 1, 3)], 1);

    let error = membership::read_store(store.path(), 100).unwrap_err();

    assert_eq!(
        error,
        MembershipError::FrameCount {
            path: wal,
            frame: 0,
            entries: 4,
            spans: 3
        }
    );
}
