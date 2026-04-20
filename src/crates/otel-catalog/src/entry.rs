use file_registry::{ByteSize, FileId, TimestampNs};
use serde::{Deserialize, Serialize};

/// A `(namespace, name)` pair identifying a log stream source.
///
/// Used both as a listing of streams inside a [`CatalogEntry`] and as a filter
/// in [`crate::CatalogQuery`]. Matching is exact equality; empty strings are
/// valid values that match only empty strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StreamEntry {
    pub namespace: String,
    pub name: String,
}

impl StreamEntry {
    pub fn new<N: Into<String>, M: Into<String>>(namespace: N, name: M) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }
}

/// One uploaded SFST file tracked by the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: FileId,
    pub remote_key: String,
    pub min_timestamp_s: u32,
    pub max_timestamp_s: u32,
    pub total_logs: u32,
    pub streams: Vec<StreamEntry>,
    pub size: ByteSize,
    pub uploaded_at_ns: TimestampNs,
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn stream_entry_roundtrip() {
        let s = StreamEntry::new("prod", "api");
        let json = serde_json::to_string(&s).unwrap();
        let parsed: StreamEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, s);
    }

    #[test]
    fn stream_entry_empty_strings_roundtrip() {
        let s = StreamEntry::new("", "");
        let json = serde_json::to_string(&s).unwrap();
        let parsed: StreamEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, s);
    }

    #[test]
    fn catalog_entry_roundtrip() {
        let entry = CatalogEntry {
            id: FileId::new(Uuid::nil(), Uuid::from_u128(1), 1, 42),
            remote_key: "tenant/sfst/2026-04-17/foo.sfst".into(),
            min_timestamp_s: 1_700_000_000,
            max_timestamp_s: 1_700_003_600,
            total_logs: 1234,
            streams: vec![
                StreamEntry::new("prod", "api"),
                StreamEntry::new("prod", "worker"),
            ],
            size: ByteSize(9876),
            uploaded_at_ns: TimestampNs(1_700_003_700_000_000_000),
        };
        let json = serde_json::to_vec(&entry).unwrap();
        let parsed: CatalogEntry = serde_json::from_slice(&json).unwrap();
        assert_eq!(parsed, entry);
    }
}
