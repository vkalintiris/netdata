#![allow(dead_code)]

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HttpAccess(pub u32);

impl HttpAccess {
    pub const NONE: Self = Self(0);
    pub const SIGNED_ID: Self = Self(1 << 0);
    pub const SAME_SPACE: Self = Self(1 << 1);
    pub const COMMERCIAL_SPACE: Self = Self(1 << 2);
    pub const ANONYMOUS_DATA: Self = Self(1 << 3);
    pub const SENSITIVE_DATA: Self = Self(1 << 4);
    pub const VIEW_AGENT_CONFIG: Self = Self(1 << 5);
    pub const EDIT_AGENT_CONFIG: Self = Self(1 << 6);
    pub const VIEW_NOTIFICATIONS_CONFIG: Self = Self(1 << 7);
    pub const EDIT_NOTIFICATIONS_CONFIG: Self = Self(1 << 8);
    pub const VIEW_ALERTS_SILENCING: Self = Self(1 << 9);
    pub const EDIT_ALERTS_SILENCING: Self = Self(1 << 10);

    pub const ALL: Self = Self(0x7FF);

    // Old role mappings
    pub const MAP_OLD_ANY: Self = Self(Self::ANONYMOUS_DATA.0);

    pub const MAP_OLD_MEMBER: Self = Self(
        Self::SIGNED_ID.0 | Self::SAME_SPACE.0 | Self::ANONYMOUS_DATA.0 | Self::SENSITIVE_DATA.0,
    );

    pub const MAP_OLD_ADMIN: Self = Self(
        Self::SIGNED_ID.0
            | Self::SAME_SPACE.0
            | Self::ANONYMOUS_DATA.0
            | Self::SENSITIVE_DATA.0
            | Self::VIEW_AGENT_CONFIG.0
            | Self::EDIT_AGENT_CONFIG.0,
    );

    /// Parse from hex string (with or without "0x" prefix)
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return Some(Self::NONE);
        }

        let s = s.strip_prefix("0x").unwrap_or(s);
        u32::from_str_radix(s, 16)
            .ok()
            .map(|v| Self(v & Self::ALL.0))
    }

    /// Parse from hex string with support for old role names
    pub fn from_slice(bytes: &[u8]) -> Self {
        let s = str::from_utf8(bytes).unwrap_or("").trim();
        if s.is_empty() {
            return Self::NONE;
        }

        match s {
            "any" | "all" => Self::MAP_OLD_ANY,
            "member" | "members" => Self::MAP_OLD_MEMBER,
            "admin" | "admins" => Self::MAP_OLD_ADMIN,
            _ => Self::from_hex(s).unwrap_or(Self::NONE),
        }
    }

    /// Check if has specific permission
    pub fn has(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Convert to u32
    pub fn as_u32(&self) -> u32 {
        self.0
    }

    /// Create from u32 (masks with ALL to ensure valid range)
    pub fn from_u32(value: u32) -> Self {
        Self(value & Self::ALL.0)
    }
}

impl From<u32> for HttpAccess {
    fn from(value: u32) -> Self {
        Self::from_u32(value)
    }
}

impl From<HttpAccess> for u32 {
    fn from(access: HttpAccess) -> Self {
        access.0
    }
}

impl fmt::Display for HttpAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

impl std::ops::BitOr for HttpAccess {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitAnd for HttpAccess {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_hex() {
        assert_eq!(HttpAccess::from_hex("0x9"), Some(HttpAccess(0x9)));
        assert_eq!(HttpAccess::from_hex("7ff"), Some(HttpAccess::ALL));
        assert_eq!(HttpAccess::from_hex(""), Some(HttpAccess::NONE));
    }

    #[test]
    fn test_from_hex_mapping_old_roles() {
        assert_eq!(HttpAccess::from_slice(b"any"), HttpAccess::MAP_OLD_ANY);
        assert_eq!(HttpAccess::from_slice(b"all"), HttpAccess::MAP_OLD_ANY);
        assert_eq!(
            HttpAccess::from_slice(b"member"),
            HttpAccess::MAP_OLD_MEMBER
        );
        assert_eq!(
            HttpAccess::from_slice(b"members"),
            HttpAccess::MAP_OLD_MEMBER
        );
        assert_eq!(HttpAccess::from_slice(b"admin"), HttpAccess::MAP_OLD_ADMIN);
        assert_eq!(HttpAccess::from_slice(b"admins"), HttpAccess::MAP_OLD_ADMIN);
        assert_eq!(HttpAccess::from_slice(b"0x7ff"), HttpAccess::ALL);
        assert_eq!(HttpAccess::from_slice(b""), HttpAccess::NONE);
    }

    #[test]
    fn test_has() {
        let access = HttpAccess::from_hex("0x9").unwrap();
        assert!(access.has(HttpAccess::SIGNED_ID));
        assert!(access.has(HttpAccess::ANONYMOUS_DATA));
        assert!(!access.has(HttpAccess::SENSITIVE_DATA));
    }

    #[test]
    fn test_u32_conversion() {
        // Test from_u32 and as_u32
        let access = HttpAccess::from_u32(0x9);
        assert_eq!(access.as_u32(), 0x9);

        // Test From traits
        let access: HttpAccess = 0x9u32.into();
        assert_eq!(access, HttpAccess(0x9));

        let value: u32 = HttpAccess::SIGNED_ID.into();
        assert_eq!(value, 1);

        // Test that values beyond ALL are masked
        let access = HttpAccess::from_u32(0xFFFF);
        assert_eq!(access.as_u32(), 0x7FF);
    }
}
