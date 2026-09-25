//! Literal text search over stored values.

/// A case-insensitive literal matched against the value part of stored
/// `field=value` pairs. Top-level fields added by the plugin (names starting
/// with `_`, such as `_role` or `_duration_band`) are never searched, so a
/// search for `root` does not match every entry span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralText {
    folded: String,
}

impl LiteralText {
    pub fn new(text: &str) -> Self {
        Self {
            folded: text.to_lowercase(),
        }
    }

    /// Whether the stored `field=value` bytes match.
    pub fn matches_kv(&self, kv: &[u8]) -> bool {
        let Some(split) = kv.iter().position(|&b| b == b'=') else {
            return false;
        };
        if kv.first() == Some(&b'_') {
            return false;
        }
        let value = String::from_utf8_lossy(&kv[split + 1..]);
        value.to_lowercase().contains(&self.folded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_values_only_literally_and_case_insensitively() {
        let text = LiteralText::new("Échec.");
        assert!(text.matches_kv("attributes.error=payment échec. retry".as_bytes()));
        assert!(
            !text.matches_kv("attributes.error=payment échecX".as_bytes()),
            "the dot is literal"
        );
        assert!(
            !text.matches_kv("échec.=x".as_bytes()),
            "keys are not searched"
        );
        assert!(
            !LiteralText::new("root").matches_kv(b"_role=root"),
            "plugin fields are skipped"
        );
        assert!(LiteralText::new("root").matches_kv(b"name=GET /root"));
    }
}
