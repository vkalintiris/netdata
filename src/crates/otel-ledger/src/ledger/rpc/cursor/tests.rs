use super::*;

#[test]
fn round_trips() {
    let c = Cursor {
        timestamp_ns: 1_700_000_000_123_456_789,
        file_seq: 42,
        position: 7,
    };
    let s = c.encode();
    assert_eq!(s, "1700000000123456789:42:7");
    assert_eq!(Cursor::decode(&s), Some(c));
}

#[test]
fn decode_rejects_malformed() {
    assert_eq!(Cursor::decode(""), None);
    assert_eq!(Cursor::decode("1:2"), None); // too few fields
    assert_eq!(Cursor::decode("1:2:3:4"), None); // too many fields
    assert_eq!(Cursor::decode("x:2:3"), None); // non-integer timestamp
    assert_eq!(Cursor::decode("1:2:-3"), None); // negative u32 position
    assert_eq!(Cursor::decode("1:2:3 "), None); // trailing whitespace
}

#[test]
fn ordering_is_ts_then_seq_then_position() {
    let a = Cursor {
        timestamp_ns: 100,
        file_seq: 0,
        position: 9,
    };
    let b = Cursor {
        timestamp_ns: 100,
        file_seq: 1,
        position: 0,
    };
    let c = Cursor {
        timestamp_ns: 101,
        file_seq: 0,
        position: 0,
    };
    // Same timestamp → lower file_seq sorts first.
    assert!(a < b);
    // Higher timestamp wins regardless of seq/position.
    assert!(b < c);
    // Same timestamp + seq → lower position sorts first.
    let d = Cursor {
        timestamp_ns: 100,
        file_seq: 0,
        position: 10,
    };
    assert!(a < d);
}
