//! `url_encode()` (`libnetdata/url/url.c`).

/// `url_encode()`: ASCII letters, digits and `-_.~` stay, a space becomes `+`, every other byte `%` and two lowercase
/// hex digits (C's "C" locale, so `isalnum` is ASCII only).
pub fn url_encode(dst: &mut Vec<u8>, text: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &b in text {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            dst.push(b);
        } else if b == b' ' {
            dst.push(b'+');
        } else {
            dst.extend_from_slice(&[b'%', HEX[usize::from(b >> 4)], HEX[usize::from(b & 15)]]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_as_c() {
        let mut out = Vec::new();
        url_encode(&mut out, "Etc/UTC a-b_c.d~e+&=é".as_bytes());
        assert_eq!(String::from_utf8(out).unwrap(), "Etc%2fUTC+a-b_c.d~e%2b%26%3d%c3%a9");
    }
}
