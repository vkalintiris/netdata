use super::*;

/// Guards the cross-shape invariant: bytes produced by [`HighFieldRef`]
/// (write side, borrowed columns) must decode back as [`crate::HighField`]
/// (read side, owned columns). If the two derives ever diverge — e.g.
/// someone adds a serde rename or changes field order on one but not
/// the other — this catches it before any file gets written.
#[test]
fn high_field_ref_wire_format_matches_owned() {
    let owned_keys: Vec<String> = vec!["alpha".into(), "bravo".into(), "charlie".into()];
    let owned_masks: Vec<u8> = vec![0b0000_0001, 0b0000_0011, 0b1000_0000];
    let owned = crate::HighField {
        keys: owned_keys.clone(),
        masks: owned_masks.clone(),
    };

    let keys_ref: Vec<&str> = owned_keys.iter().map(|s| s.as_str()).collect();
    let view = HighFieldRef {
        keys: &keys_ref,
        masks: &owned_masks,
    };

    let owned_bytes =
        bincode::serde::encode_to_vec(&owned, bincode::config::standard()).unwrap();
    let view_bytes = bincode::serde::encode_to_vec(&view, bincode::config::standard()).unwrap();

    assert_eq!(
        owned_bytes, view_bytes,
        "HighFieldRef and HighField produce different wire bytes",
    );

    let (round_trip, _): (crate::HighField, _) =
        bincode::serde::decode_from_slice(&view_bytes, bincode::config::standard()).unwrap();
    assert_eq!(round_trip, owned);
}
