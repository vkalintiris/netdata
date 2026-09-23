//! `HTTP_ACCESS`, ported from `src/libnetdata/user-auth/http-access.{h,c}`: the permissions a caller holds and a
//! function requires.

use netdata_agent_text::parse::strtoull16;

pub const NONE: u32 = 0;
pub const SIGNED_ID: u32 = 1 << 0;
pub const SAME_SPACE: u32 = 1 << 1;
pub const COMMERCIAL_SPACE: u32 = 1 << 2;
pub const ANONYMOUS_DATA: u32 = 1 << 3;
pub const SENSITIVE_DATA: u32 = 1 << 4;
pub const VIEW_AGENT_CONFIG: u32 = 1 << 5;
pub const EDIT_AGENT_CONFIG: u32 = 1 << 6;
pub const VIEW_NOTIFICATIONS_CONFIG: u32 = 1 << 7;
pub const EDIT_NOTIFICATIONS_CONFIG: u32 = 1 << 8;
pub const VIEW_ALERTS_SILENCING: u32 = 1 << 9;
pub const EDIT_ALERTS_SILENCING: u32 = 1 << 10;
/// `HTTP_ACCESS_ALL`.
pub const ALL: u32 = (1 << 11) - 1;

/// `HTTP_ACCESS_MAP_OLD_ANY`.
pub const MAP_OLD_ANY: u32 = ANONYMOUS_DATA;
/// `HTTP_ACCESS_MAP_OLD_MEMBER`.
pub const MAP_OLD_MEMBER: u32 = SIGNED_ID | SAME_SPACE | ANONYMOUS_DATA | SENSITIVE_DATA;
/// `HTTP_ACCESS_MAP_OLD_ADMIN`.
pub const MAP_OLD_ADMIN: u32 = MAP_OLD_MEMBER | VIEW_AGENT_CONFIG | EDIT_AGENT_CONFIG;

/// `http_access_from_hex_mapping_old_roles()`: the old role names, else hex bits.
pub fn from_hex_mapping_old_roles(s: &[u8]) -> u32 {
    match s {
        b"" => NONE,
        b"any" | b"all" => MAP_OLD_ANY,
        b"member" | b"members" => MAP_OLD_MEMBER,
        b"admin" | b"admins" => MAP_OLD_ADMIN,
        // C narrows to its packed enum before masking; only the defined low bits survive either way.
        _ => strtoull16(s).0 as u32 & ALL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_hex() {
        let cases: [(&[u8], u32); 7] = [
            (b"", 0),
            (b"members", 0x1b),
            (b"admin", 0x7b),
            (b"all", 0x8),
            (b"0x13", 0x13),
            (b"ffff", ALL),
            (b"bogus", 0xb),
        ];
        for (s, expected) in cases {
            assert_eq!(
                from_hex_mapping_old_roles(s),
                expected,
                "{}",
                String::from_utf8_lossy(s)
            );
        }
    }
}
