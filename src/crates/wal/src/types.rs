use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// TimestampNs
// ---------------------------------------------------------------------------

/// Nanoseconds since the Unix epoch.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct TimestampNs(pub u64);

impl TimestampNs {
    pub const ZERO: Self = Self(0);

    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn saturating_sub(self, rhs: Self) -> u64 {
        self.0.saturating_sub(rhs.0)
    }
}

impl fmt::Display for TimestampNs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ns", self.0)
    }
}

// ---------------------------------------------------------------------------
// ByteSize
// ---------------------------------------------------------------------------

/// A byte count (file size, offset, etc.).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct ByteSize(pub u64);

impl ByteSize {
    pub const ZERO: Self = Self(0);

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes", self.0)
    }
}

// ---------------------------------------------------------------------------
// FileId
// ---------------------------------------------------------------------------

/// Uniquely identifies a WAL file across machines, boots, and sequences.
///
/// The filename format is: `<machine_id>-<boot_id>-<seq:010>.bin`
/// where machine_id and boot_id are 32-character lowercase hex (no hyphens).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileId {
    pub machine_id: Uuid,
    pub boot_id: Uuid,
    pub seq: u64,
}

impl FileId {
    pub fn new(machine_id: Uuid, boot_id: Uuid, seq: u64) -> Self {
        Self {
            machine_id,
            boot_id,
            seq,
        }
    }

    /// Format the stem portion: `<machine_id>-<boot_id>-<seq:010>`
    pub fn to_stem(&self) -> String {
        format!(
            "{}-{}-{:010}",
            self.machine_id.as_simple(),
            self.boot_id.as_simple(),
            self.seq,
        )
    }

    /// Format a full filename: `<stem>.<ext>`
    pub fn to_filename(&self, ext: &str) -> String {
        format!("{}.{}", self.to_stem(), ext)
    }

    /// Parse a filename (not a full path) into a FileId.
    ///
    /// Expects: `<machine_id>-<boot_id>-<seq>.<ext>`
    pub fn parse(path: &Path) -> Option<Self> {
        let name = path.file_stem()?.to_str()?;
        Self::parse_stem(name)
    }

    /// Parse just the stem: `<machine_id>-<boot_id>-<seq>`
    pub fn parse_stem(stem: &str) -> Option<Self> {
        // machine_id is 32 hex chars, then '-', boot_id is 32 hex chars, then '-', then seq
        if stem.len() < 32 + 1 + 32 + 1 + 1 {
            return None;
        }

        let machine_str = &stem[..32];
        if stem.as_bytes()[32] != b'-' {
            return None;
        }
        let boot_str = &stem[33..65];
        if stem.as_bytes()[65] != b'-' {
            return None;
        }
        let seq_str = &stem[66..];

        let machine_id = Uuid::try_parse(machine_str).ok()?;
        let boot_id = Uuid::try_parse(boot_str).ok()?;
        let seq = seq_str.parse().ok()?;

        Some(Self {
            machine_id,
            boot_id,
            seq,
        })
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_stem())
    }
}

impl Ord for FileId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.machine_id
            .as_bytes()
            .cmp(other.machine_id.as_bytes())
            .then_with(|| self.boot_id.as_bytes().cmp(other.boot_id.as_bytes()))
            .then_with(|| self.seq.cmp(&other.seq))
    }
}

impl PartialOrd for FileId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_machine_id() -> Uuid {
        Uuid::try_parse("550e8400e29b41d4a716446655440000").unwrap()
    }

    fn test_boot_id() -> Uuid {
        Uuid::try_parse("7f3b2a1e9c4d4f8ab1c2d3e4f5a6b7c8").unwrap()
    }

    #[test]
    fn file_id_stem_roundtrip() {
        let id = FileId::new(test_machine_id(), test_boot_id(), 42);
        let stem = id.to_stem();
        assert_eq!(
            stem,
            "550e8400e29b41d4a716446655440000-7f3b2a1e9c4d4f8ab1c2d3e4f5a6b7c8-0000000042"
        );
        let parsed = FileId::parse_stem(&stem).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn file_id_filename_roundtrip() {
        let id = FileId::new(test_machine_id(), test_boot_id(), 1);
        let filename = id.to_filename("bin");
        let path = Path::new(&filename);
        let parsed = FileId::parse(path).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn file_id_parse_invalid() {
        assert!(FileId::parse_stem("").is_none());
        assert!(FileId::parse_stem("not-a-valid-id").is_none());
        assert!(FileId::parse_stem("wal-0000000001").is_none());
    }

    #[test]
    fn file_id_ordering() {
        let a = FileId::new(test_machine_id(), test_boot_id(), 1);
        let b = FileId::new(test_machine_id(), test_boot_id(), 2);
        assert!(a < b);
    }
}
