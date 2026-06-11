use std::io::Cursor;

use crate::{
    ChunkCounts, Error, FieldEntry, FieldTier, Histogram, IdRanges, KvId, Metadata, StreamWriter,
    Summary, pack,
};

fn counts(mid: u16, high: u16, batches: u8) -> ChunkCounts {
    ChunkCounts {
        mid_fields: mid,
        high_fields: high,
        stream_batches: batches,
    }
}

fn payload() -> Vec<u8> {
    pack(&vec![0u8; 4], 1).unwrap()
}

fn writer(c: ChunkCounts) -> StreamWriter<Cursor<Vec<u8>>> {
    StreamWriter::new(Cursor::new(Vec::new()), c).unwrap()
}

/// Drive the prefix (SUMR, META, TIMS, PRIM) with throwaway payloads.
fn write_prefix(w: &mut StreamWriter<Cursor<Vec<u8>>>) {
    w.summary(&payload()).unwrap();
    w.metadata(&payload()).unwrap();
    w.timestamps(&payload()).unwrap();
    w.primary(&payload()).unwrap();
}

#[test]
fn full_file_in_canonical_order_round_trips() {
    // Real SUMR/META payloads (the reader decodes them); the field
    // table declares the same 2-mid/1-high shape the writer streams.
    let summary = Summary {
        min_timestamp_s: 1,
        max_timestamp_s: 2,
        total_logs: 3,
        stream: crate::ServiceStream::new("ns", "svc"),
    };
    let field = |name: &str, tier| FieldEntry {
        name: name.into(),
        cardinality: 1,
        tier,
    };
    let metadata = Metadata {
        histogram: Histogram {
            timestamps: vec![1],
            counts: vec![3],
        },
        id_ranges: IdRanges {
            low_end: KvId(1),
            mid_end: KvId(3),
            high_end: KvId(4),
        },
        fields: vec![
            field("m0", FieldTier::Mid),
            field("m1", FieldTier::Mid),
            field("h0", FieldTier::High),
        ]
        .into(),
    };

    let mut w = writer(counts(2, 1, 2));
    w.summary(&pack(&summary, 1).unwrap()).unwrap();
    w.metadata(&pack(&metadata, 1).unwrap()).unwrap();
    w.timestamps(&payload()).unwrap();
    w.primary(&payload()).unwrap();
    assert_eq!(w.add_mid_field(&payload()).unwrap(), 0);
    assert_eq!(w.add_mid_field(&payload()).unwrap(), 1);
    assert_eq!(w.add_high_field(&payload()).unwrap(), 0);
    assert_eq!(w.add_stream_batch(&payload()).unwrap(), 0);
    assert_eq!(w.add_stream_batch(&payload()).unwrap(), 1);
    let buf = w.finish().unwrap().into_inner();

    let reader = crate::Reader::open(&buf).unwrap();
    assert!(reader.has_summary());
    assert!(reader.has_metadata());
    assert_eq!(reader.summary().unwrap(), summary);
    assert_eq!(reader.num_mid().unwrap(), 2);
    assert_eq!(reader.num_high().unwrap(), 1);
}

#[test]
fn rejects_zero_and_excess_stream_batch_counts() {
    for n in [0u8, crate::MAX_STREAM_BATCHES + 1] {
        assert!(matches!(
            StreamWriter::new(Cursor::new(Vec::new()), counts(0, 0, n)),
            Err(Error::InvalidStreamBatchCount(_))
        ));
    }
}

#[test]
fn rejects_prefix_chunks_out_of_order() {
    // Metadata before summary.
    let mut w = writer(counts(0, 0, 1));
    assert!(matches!(
        w.metadata(&payload()),
        Err(Error::WriterMisuse(_))
    ));

    // Primary before timestamps.
    let mut w = writer(counts(0, 0, 1));
    w.summary(&payload()).unwrap();
    w.metadata(&payload()).unwrap();
    assert!(matches!(w.primary(&payload()), Err(Error::WriterMisuse(_))));

    // A secondary chunk before the prefix is complete.
    let mut w = writer(counts(1, 0, 1));
    w.summary(&payload()).unwrap();
    assert!(matches!(
        w.add_mid_field(&payload()),
        Err(Error::WriterMisuse(_))
    ));

    // The same prefix chunk twice.
    let mut w = writer(counts(0, 0, 1));
    w.summary(&payload()).unwrap();
    assert!(matches!(w.summary(&payload()), Err(Error::WriterMisuse(_))));
}

#[test]
fn rejects_secondary_chunks_out_of_section_order() {
    // A mid-field after a high-field.
    let mut w = writer(counts(1, 1, 1));
    write_prefix(&mut w);
    w.add_mid_field(&payload()).unwrap();
    w.add_high_field(&payload()).unwrap();
    assert!(matches!(
        w.add_mid_field(&payload()),
        Err(Error::WriterMisuse(_))
    ));

    // A high-field before all declared mid-fields.
    let mut w = writer(counts(2, 1, 1));
    write_prefix(&mut w);
    w.add_mid_field(&payload()).unwrap();
    assert!(matches!(
        w.add_high_field(&payload()),
        Err(Error::WriterMisuse(_))
    ));

    // A stream batch before all declared field chunks.
    let mut w = writer(counts(0, 1, 1));
    write_prefix(&mut w);
    assert!(matches!(
        w.add_stream_batch(&payload()),
        Err(Error::WriterMisuse(_))
    ));
}

#[test]
fn rejects_chunks_beyond_declared_counts() {
    let mut w = writer(counts(1, 0, 1));
    write_prefix(&mut w);
    w.add_mid_field(&payload()).unwrap();
    assert!(matches!(
        w.add_mid_field(&payload()),
        Err(Error::WriterMisuse(_))
    ));

    let mut w = writer(counts(0, 0, 1));
    write_prefix(&mut w);
    w.add_stream_batch(&payload()).unwrap();
    assert!(matches!(
        w.add_stream_batch(&payload()),
        Err(Error::WriterMisuse(_))
    ));
}

#[test]
fn finish_refuses_an_underfilled_file() {
    // Prefix incomplete.
    let mut w = writer(counts(0, 0, 1));
    w.summary(&payload()).unwrap();
    assert!(matches!(w.finish(), Err(Error::WriterMisuse(_))));

    // Declared secondary chunks missing.
    let mut w = writer(counts(1, 0, 1));
    write_prefix(&mut w);
    assert!(matches!(w.finish(), Err(Error::WriterMisuse(_))));

    // Declared batches missing.
    let mut w = writer(counts(0, 0, 2));
    write_prefix(&mut w);
    w.add_stream_batch(&payload()).unwrap();
    assert!(matches!(w.finish(), Err(Error::WriterMisuse(_))));
}
