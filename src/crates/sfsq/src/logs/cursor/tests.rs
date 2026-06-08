use super::*;

#[test]
fn round_trips() {
    let c = Cursor {
        timestamp_ns: 1_700_000_000_123_456_789,
        file_seq: 42,
        sub_id: 3,
        position: 7,
    };
    let s = c.encode();
    assert_eq!(s, "1700000000123456789:42:3:7");
    assert_eq!(Cursor::decode(&s), Some(c));
}

#[test]
fn decode_rejects_malformed() {
    assert_eq!(Cursor::decode(""), None);
    assert_eq!(Cursor::decode("1:2:3"), None); // too few fields (legacy 3-field)
    assert_eq!(Cursor::decode("1:2:3:4:5"), None); // too many fields
    assert_eq!(Cursor::decode("x:2:3:4"), None); // non-integer timestamp
    assert_eq!(Cursor::decode("1:2:3:-4"), None); // negative u32 position
    assert_eq!(Cursor::decode("1:2:3:4 "), None); // trailing whitespace
}

#[test]
fn ordering_is_ts_then_seq_then_sub_then_position() {
    let c = |timestamp_ns, file_seq, sub_id, position| Cursor {
        timestamp_ns,
        file_seq,
        sub_id,
        position,
    };
    // Same timestamp → lower file_seq sorts first.
    assert!(c(100, 0, 0, 9) < c(100, 1, 0, 0));
    // Higher timestamp wins regardless of the rest.
    assert!(c(100, 1, 0, 0) < c(101, 0, 0, 0));
    // Same (timestamp, seq) → lower sub_id sorts first (chunk before tail).
    assert!(c(100, 5, 2, 99) < c(100, 5, Cursor::TAIL_SUB_ID, 0));
    // Same (timestamp, seq, sub_id) → lower position sorts first.
    assert!(c(100, 0, 0, 9) < c(100, 0, 0, 10));
}
