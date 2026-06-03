/// The high-card string-arena round-trips through bincode (the on-disk
/// codec) and its keys are accessible by index after `rebuild_offsets` —
/// which is what the reader does on load (`offsets` is `#[serde(skip)]`).
#[test]
fn high_field_arena_round_trips() {
    let keys = ["alpha", "bravo", "charlie"];
    let masks = vec![0b0000_0001u8, 0b0000_0011, 0b1000_0000];
    let high = crate::HighField::for_write(&keys, masks);

    let bytes = bincode::serde::encode_to_vec(&high, bincode::config::standard()).unwrap();
    let (mut decoded, _): (crate::HighField, _) =
        bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
    decoded.rebuild_offsets();

    assert_eq!(decoded, high);
    assert_eq!(decoded.len(), 3);
    assert_eq!(decoded.key(0), b"alpha");
    assert_eq!(decoded.key(2), b"charlie");
    assert_eq!(decoded.binary_search(b"bravo"), Ok(1));
    assert_eq!(decoded.binary_search(b"zzz"), Err(3));
    assert_eq!(decoded.masks, vec![0b0000_0001, 0b0000_0011, 0b1000_0000]);
}
