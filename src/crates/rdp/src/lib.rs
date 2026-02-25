// String tokenizer and parser for field names with compact encoding
//
// Tokenizes strings into words (lowercase, UPPERCASE, Capitalized) and separators (. _ -)
// Parses tokens into fields: Lowercase, Uppercase, LowerCamel, UpperCamel, Empty
// Encodes token stream into compact lossless representation
//
// Example: "log.body.HostName" → encoded: "3RAAO" (5 chars: 2-char checksum + 3-char structure)

mod encoder;
mod parser;
mod tokenizer;

use encoder::encode;

/// Returns the fully encoded key: `ND` + structure encoding + `_` + normalized key.
///
/// The result combines:
/// - `ND` prefix (Netdata namespace identifier)
/// - The compact structure encoding with run-length compression
/// - An underscore separator
/// - The original key converted to uppercase with dots and hyphens replaced by underscores,
///   and common prefixes shortened:
///   - `resource.attributes.` → `RA_`
///   - `log.attributes.` → `LA_`
///   - `log.body.` → `LB_`
///
/// Run-length compression replaces 3+ consecutive identical characters with count + character.
/// If the key contains camel case, a 2-character checksum is prepended to the encoding.
///
/// # MD5 Fallback
///
/// Falls back to `ND_<32-hex-chars>` format when:
/// - Input contains invalid characters (anything except a-z, A-Z, 0-9, '.', '-', '_')
/// - Encoded result would exceed 64 bytes (systemd's field name limit)
///
/// The result is guaranteed to be systemd-compatible and ≤ 64 bytes.
///
/// # Examples
///
/// ```
/// use rdp::remap;
///
/// // Simple lowercase field
/// assert_eq!(remap(b"hello"), "NDE_HELLO");
///
/// // With dot separators (2 A's — no compression)
/// assert_eq!(remap(b"log.body.hostname"), "NDAAE_LB_HOSTNAME");
///
/// // Many nested levels — structure compression (9 A's → 9A, then E for end)
/// assert_eq!(remap(b"my.very.deeply.nested.field.that.ends.in.the.abyss"), "ND9AE_MY_VERY_DEEPLY_NESTED_FIELD_THAT_ENDS_IN_THE_ABYSS");
///
/// // With camel case (2-char checksum + structure)
/// assert_eq!(remap(b"log.body.HostName"), "ND3RAAO_LB_HOSTNAME");
///
/// // With hyphens
/// assert_eq!(remap(b"hello-world"), "NDCE_HELLO_WORLD");
///
/// // With resource.attributes prefix — compression (3 A's → 3A)
/// assert_eq!(remap(b"resource.attributes.host.name"), "ND3AE_RA_HOST_NAME");
///
/// // With invalid characters (space) — falls back to MD5
/// let md5_result = remap(b"field name");
/// assert!(md5_result.starts_with("ND_"));
/// assert_eq!(md5_result.len(), 35); // ND_ + 32 hex chars
///
/// // Non-UTF8 — falls back to MD5
/// let non_utf8 = b"\xFF\xFE invalid";
/// let result = remap(non_utf8);
/// assert!(result.starts_with("ND_"));
/// assert_eq!(result.len(), 35);
///
/// // Long names that would exceed 64 bytes — falls back to MD5
/// let long_name = b"very.long.deeply.nested.field.name.that.would.definitely.exceed.the.systemd.limit";
/// let result = remap(long_name);
/// assert!(result.starts_with("ND_"));
/// assert!(result.len() <= 64);
/// ```
pub fn remap(key: &[u8]) -> String {
    // Detect common prefix on the raw key, keep only the suffix for normalization.
    let (short_prefix, suffix) = if let Some(rest) = key.strip_prefix(b"resource.attributes.") {
        ("RA_", rest)
    } else if let Some(rest) = key.strip_prefix(b"log.attributes.") {
        ("LA_", rest)
    } else if let Some(rest) = key.strip_prefix(b"log.body.") {
        ("LB_", rest)
    } else {
        ("", key)
    };

    let mut remapped_key = String::with_capacity(64);
    remapped_key.push_str("ND");

    // If encoding contains non-valid bytes, or the length of the remapped
    // key would be larger than systemd's 64-byte limit, fallback to using
    // an md5 hash.
    if !encode(key, &mut remapped_key)
        || remapped_key.len() + 1 + short_prefix.len() + suffix.len() > 64
    {
        return format!("ND_{:X}", md5::compute(key));
    }

    remapped_key.push('_');
    remapped_key.push_str(short_prefix);

    // Normalize and push only the suffix: uppercase, dots and hyphens become underscores.
    for &b in suffix {
        remapped_key.push(match b {
            b'.' | b'-' => b'_',
            _ => b.to_ascii_uppercase(),
        } as char);
    }

    remapped_key
}
