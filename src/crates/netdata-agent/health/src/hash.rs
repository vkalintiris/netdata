//! A rule's configuration hash (`health_prototypes.c` `health_prototype_hash_id()`, `libnetdata/uuid/uuid.c`
//! `UUID_generate_from_hash()`).

use twox_hash::XxHash3_128;

/// The UUID of `bytes`: XXH3-128 without a seed, its low half first as C's struct lies in memory, then the version
/// and variant bits of a random UUID.
pub fn hash_id(bytes: &[u8]) -> [u8; 16] {
    let mut uuid = XxHash3_128::oneshot(bytes).to_le_bytes();
    uuid[6] = (uuid[6] & 0x0F) | 0x40;
    uuid[8] = (uuid[8] & 0x3F) | 0x80;
    uuid
}
