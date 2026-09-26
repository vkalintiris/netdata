//! The CRCs of dbengine's structures (`rrdenginelib.h` `crc32cmp()`/`crc32set()`): zlib's CRC-32 with seed 0, stored
//! little-endian after the bytes it covers.

/// `crc32(0, data, len)`.
pub fn crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

/// `crc32set()`: the 4 stored bytes of `crc`.
pub fn crc_bytes(crc: u32) -> [u8; 4] {
    crc.to_le_bytes()
}

/// `crc32cmp()` inverted: whether the stored 4 bytes hold `crc`.
pub fn crc_matches(stored: &[u8], crc: u32) -> bool {
    stored.len() == 4 && stored == crc.to_le_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zlib_crc32() {
        // the CRC-32 check value
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert!(crc_matches(&crc_bytes(0xCBF4_3926), crc32(b"123456789")));
        assert!(!crc_matches(&[0, 0, 0], 0));
    }
}
