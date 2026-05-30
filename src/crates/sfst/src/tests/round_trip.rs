//! Format round-trip tests: write a file via [`Writer`] / [`pack`],
//! read it back via [`Reader`], assert the chunks decode to the
//! values we put in.

use crate::*;
use fst_index::FstIndex;
use treight::Bitmap;

fn empty_bitmap() -> BitmapValue {
    BitmapValue {
        desc: Bitmap::empty(0),
        data: Vec::new(),
    }
}

fn build_primary(keys: &[&str]) -> FstIndex<BitmapValue> {
    let entries: Vec<(&str, BitmapValue)> = keys.iter().map(|k| (*k, empty_bitmap())).collect();
    FstIndex::build(entries).unwrap()
}

fn empty_timestamps() -> Vec<u8> {
    pack(&Vec::<i64>::new(), 1).unwrap()
}

fn empty_stream_batch() -> Vec<u8> {
    pack(&Vec::<Vec<KvId>>::new(), 1).unwrap()
}

fn sample_summary() -> Summary {
    Summary {
        min_timestamp_s: 1_700_000_000,
        max_timestamp_s: 1_700_003_600,
        total_logs: 1234,
        stream: ServiceStream::new("prod", "api"),
    }
}

fn sample_metadata() -> Metadata {
    Metadata {
        histogram: Histogram {
            timestamps: vec![100, 200, 300],
            counts: vec![10, 25, 50],
        },
        id_ranges: IdRanges {
            low_end: KvId(3),
            mid_end: KvId(5),
            high_end: KvId(8),
        },
        fields: Default::default(),
    }
}

#[test]
fn round_trip_primary_only() {
    let primary = build_primary(&["alpha", "beta", "gamma"]);

    let mut writer = Writer::new();
    writer.set_primary(pack(&primary, 1).unwrap());
    writer.set_timestamps(empty_timestamps());
    writer.add_stream_batch(empty_stream_batch());

    let mut buf = Vec::new();
    writer.write_to(&mut buf).unwrap();

    let reader = Reader::open(&buf).unwrap();
    assert!(!reader.has_summary());
    assert!(!reader.has_metadata());

    let p = reader.primary().unwrap();
    assert!(p.get(b"alpha").is_some());
    assert!(p.get(b"beta").is_some());
    assert!(p.get(b"gamma").is_some());
    assert!(p.get(b"missing").is_none());
}

#[test]
fn round_trip_summary() {
    let summary = sample_summary();
    let primary = build_primary(&["a"]);

    let mut writer = Writer::new();
    writer.set_summary(pack(&summary, 1).unwrap());
    writer.set_primary(pack(&primary, 1).unwrap());
    writer.set_timestamps(empty_timestamps());
    writer.add_stream_batch(empty_stream_batch());

    let mut buf = Vec::new();
    writer.write_to(&mut buf).unwrap();

    let reader = Reader::open(&buf).unwrap();
    assert!(reader.has_summary());
    assert!(!reader.has_metadata());
    assert_eq!(reader.summary().unwrap(), summary);
}

#[test]
fn round_trip_metadata() {
    let metadata = sample_metadata();
    let primary = build_primary(&["a", "b"]);

    let mut writer = Writer::new();
    writer.set_metadata(pack(&metadata, 1).unwrap());
    writer.set_primary(pack(&primary, 1).unwrap());
    writer.set_timestamps(empty_timestamps());
    writer.add_stream_batch(empty_stream_batch());

    let mut buf = Vec::new();
    writer.write_to(&mut buf).unwrap();

    let reader = Reader::open(&buf).unwrap();
    assert!(reader.has_metadata());

    let read = reader.metadata().unwrap();
    assert_eq!(read, &metadata);
}

#[test]
fn round_trip_fields_and_secondary_chunks() {
    // Field table: 1 low, 2 mid, 1 high. Secondary chunks: 2 mid +
    // 1 high + 1 stream-batch.
    let fields = vec![
        FieldEntry {
            name: "level".into(),
            cardinality: 3,
            tier: FieldTier::Low,
        },
        FieldEntry {
            name: "host".into(),
            cardinality: 200,
            tier: FieldTier::Mid,
        },
        FieldEntry {
            name: "pod".into(),
            cardinality: 300,
            tier: FieldTier::Mid,
        },
        FieldEntry {
            name: "trace_id".into(),
            cardinality: 50_000,
            tier: FieldTier::High,
        },
    ];
    let metadata = Metadata {
        histogram: Histogram {
            timestamps: vec![1_700_000_000],
            counts: vec![2],
        },
        id_ranges: IdRanges {
            low_end: KvId(1),
            mid_end: KvId(6),
            high_end: KvId(7),
        },
        fields: fields.into(),
    };

    let primary = build_primary(&["level=info"]);
    let mid_host = build_primary(&["host=h1", "host=h2"]);
    let mid_pod = build_primary(&["pod=p1", "pod=p2", "pod=p3"]);
    // Bit 0 set: the value lives in the single stream batch we emit.
    let high_trace = HighField {
        keys: vec!["trace_id=abc".into()],
        // Bit 0: the value lives in the single stream batch we emit.
        masks: vec![0b0000_0001],
    };
    let stream_entries: Vec<Vec<KvId>> = vec![vec![KvId(0), KvId(1)], vec![KvId(2)]];
    let timestamps: Vec<i64> = vec![1_700_000_000_000_000_000, 1_700_000_000_500_000_000];

    let mut writer = Writer::new();
    writer.set_metadata(pack(&metadata, 1).unwrap());
    writer.set_primary(pack(&primary, 1).unwrap());
    assert_eq!(writer.add_mid_field(pack(&mid_host, 1).unwrap()), 0);
    assert_eq!(writer.add_mid_field(pack(&mid_pod, 1).unwrap()), 1);
    assert_eq!(writer.add_high_field(pack(&high_trace, 1).unwrap()), 0);
    writer.set_timestamps(pack(&timestamps, 1).unwrap());
    assert_eq!(
        writer.add_stream_batch(pack(&stream_entries, 1).unwrap()),
        0
    );

    let mut buf = Vec::new();
    writer.write_to(&mut buf).unwrap();

    let reader = Reader::open(&buf).unwrap();
    assert_eq!(reader.num_mid().unwrap(), 2);
    assert_eq!(reader.num_high().unwrap(), 1);

    assert_eq!(reader.fields().unwrap().len(), 4);

    // Mid-card chunks.
    let m0 = reader.mid_field(0).unwrap();
    assert!(m0.get(b"host=h1").is_some());
    let m1 = reader.mid_field(1).unwrap();
    assert!(m1.get(b"pod=p2").is_some());

    // High-card chunk: columnar (keys, masks) round-trips.
    let h0 = reader.high_field(0).unwrap();
    assert_eq!(h0, high_trace);

    // Timestamps chunk.
    assert_eq!(reader.timestamps().unwrap(), timestamps);

    // Stream-batch chunk: the only batch (index 0) carries everything.
    let batch = reader.stream_batch(0).unwrap();
    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0], vec![KvId(0), KvId(1)]);
    // Asking for a non-existent batch yields ChunkNotFound.
    assert!(matches!(
        reader.stream_batch(1),
        Err(Error::ChunkNotFound(1))
    ));
}

#[test]
fn mid_field_out_of_range_errors() {
    let primary = build_primary(&["k"]);
    let mid = build_primary(&["host=h"]);

    let mut writer = Writer::new();
    writer.set_primary(pack(&primary, 1).unwrap());
    writer.add_mid_field(pack(&mid, 1).unwrap());
    writer.set_timestamps(empty_timestamps());
    writer.add_stream_batch(empty_stream_batch());

    let mut buf = Vec::new();
    writer.write_to(&mut buf).unwrap();

    let reader = Reader::open(&buf).unwrap();
    assert!(reader.mid_field(0).is_ok());
    assert!(matches!(reader.mid_field(1), Err(Error::ChunkNotFound(1))));
}

#[test]
fn error_on_no_timestamps() {
    let primary = build_primary(&["k"]);
    let mut writer = Writer::new();
    writer.set_primary(pack(&primary, 1).unwrap());

    let mut buf = Vec::new();
    assert!(matches!(
        writer.write_to(&mut buf),
        Err(Error::NoTimestamps)
    ));
}

#[test]
fn full_file_round_trip() {
    let summary = sample_summary();
    let mut metadata = sample_metadata();
    metadata.fields = vec![FieldEntry {
        name: "level".into(),
        cardinality: 3,
        tier: FieldTier::Low,
    }]
    .into();
    let primary = build_primary(&["level=info"]);
    let stream_entries: Vec<Vec<KvId>> = vec![vec![KvId(0)]];
    let timestamps: Vec<i64> = vec![1_700_000_000_000_000_000];

    let mut writer = Writer::new();
    writer.set_summary(pack(&summary, 1).unwrap());
    writer.set_metadata(pack(&metadata, 1).unwrap());
    writer.set_primary(pack(&primary, 1).unwrap());
    writer.set_timestamps(pack(&timestamps, 1).unwrap());
    writer.add_stream_batch(pack(&stream_entries, 1).unwrap());

    let mut buf = Vec::new();
    writer.write_to(&mut buf).unwrap();

    let reader = Reader::open(&buf).unwrap();
    assert_eq!(reader.summary().unwrap(), summary);
    assert_eq!(reader.fields().unwrap().len(), 1);
    assert_eq!(reader.num_mid().unwrap(), 0);
    assert_eq!(reader.num_high().unwrap(), 0);
    assert_eq!(reader.timestamps().unwrap(), timestamps);
    // A small `total_logs` puts everything in a single batch.
    assert_eq!(reader.num_stream_batches().unwrap(), 1);
    assert_eq!(reader.stream_batch(0).unwrap(), stream_entries);
}

#[test]
fn round_trip_multi_batch_stream() {
    // 3072 logs → exactly 3 batches of 1024.
    let total_logs = 3072u32;
    assert_eq!(num_stream_batches(total_logs), 3);
    assert_eq!(stream_batch_size(total_logs), 1024);

    let summary = Summary {
        min_timestamp_s: 1_700_000_000,
        max_timestamp_s: 1_700_003_071,
        total_logs,
        stream: ServiceStream::new("prod", "api"),
    };
    let metadata = Metadata {
        histogram: Histogram {
            timestamps: vec![1_700_000_000],
            counts: vec![total_logs],
        },
        id_ranges: IdRanges {
            low_end: KvId(1),
            mid_end: KvId(1),
            high_end: KvId(2),
        },
        fields: vec![
            FieldEntry {
                name: "level".into(),
                cardinality: 1,
                tier: FieldTier::Low,
            },
            FieldEntry {
                name: "trace_id".into(),
                cardinality: 50_000,
                tier: FieldTier::High,
            },
        ]
        .into(),
    };

    let primary = build_primary(&["level=info"]);
    // A high-card value that lives only in batches 0 and 2.
    let high_trace = HighField {
        keys: vec!["trace_id=abc".into()],
        masks: vec![0b0000_0101],
    };
    let entries: Vec<Vec<KvId>> = (0..total_logs).map(|i| vec![KvId(i)]).collect();
    let timestamps: Vec<i64> = (0..total_logs as i64).collect();

    let mut writer = Writer::new();
    writer.set_summary(pack(&summary, 1).unwrap());
    writer.set_metadata(pack(&metadata, 1).unwrap());
    writer.set_primary(pack(&primary, 1).unwrap());
    writer.add_high_field(pack(&high_trace, 1).unwrap());
    writer.set_timestamps(pack(&timestamps, 1).unwrap());
    for (i, batch) in entries.chunks(1024).enumerate() {
        // pack accepts &[T] thanks to its ?Sized bound — no Vec
        // materialisation needed.
        assert_eq!(writer.add_stream_batch(pack(batch, 1).unwrap()) as usize, i);
    }

    let mut buf = Vec::new();
    writer.write_to(&mut buf).unwrap();

    // Reader sees three SB chunks, each holding its expected slice.
    let reader = Reader::open(&buf).unwrap();
    assert_eq!(reader.num_stream_batches().unwrap(), 3);
    for i in 0..3u8 {
        let batch = reader.stream_batch(i).unwrap();
        assert_eq!(batch.len(), 1024);
        assert_eq!(batch[0], vec![KvId(u32::from(i) * 1024)]);
        assert_eq!(batch[1023], vec![KvId(u32::from(i) * 1024 + 1023)]);
    }
    // Out-of-range batch fails cleanly.
    assert!(matches!(
        reader.stream_batch(3),
        Err(Error::ChunkNotFound(3))
    ));
    // High-card chunk survives bit-for-bit.
    assert_eq!(reader.high_field(0).unwrap(), high_trace);

    // IndexReader's convenience walks every batch in chronological order.
    let index = IndexReader::open(&buf).unwrap();
    assert_eq!(index.num_stream_batches(), 3);
    let all = index.load_all_stream_entries().unwrap();
    assert_eq!(all.len(), total_logs as usize);
    assert_eq!(all[0], vec![KvId(0)]);
    assert_eq!(all[total_logs as usize - 1], vec![KvId(total_logs - 1)]);
}
