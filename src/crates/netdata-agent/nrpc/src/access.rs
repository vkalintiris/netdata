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

/// `http_access_name[]`, in bit order.
const NAMES: [(u32, &str); 11] = [
    (SIGNED_ID, "signed-in"),
    (SAME_SPACE, "same-space"),
    (COMMERCIAL_SPACE, "commercial"),
    (ANONYMOUS_DATA, "anonymous-data"),
    (SENSITIVE_DATA, "sensitive-data"),
    (VIEW_AGENT_CONFIG, "view-config"),
    (EDIT_AGENT_CONFIG, "edit-config"),
    (VIEW_NOTIFICATIONS_CONFIG, "view-notifications-config"),
    (EDIT_NOTIFICATIONS_CONFIG, "edit-notifications-config"),
    (VIEW_ALERTS_SILENCING, "view-alerts-silencing"),
    (EDIT_ALERTS_SILENCING, "edit-alerts-silencing"),
];

/// `http_access2buffer_json_array()`'s names: each bit held, in bit order.
pub fn names(access: u32) -> impl Iterator<Item = &'static str> {
    NAMES
        .into_iter()
        .filter(move |(bit, _)| access & bit != 0)
        .map(|(_, name)| name)
}

/// `http_access2id_one()`: the bit of an access name, none for another text.
pub fn id_one(name: &[u8]) -> u32 {
    NAMES
        .iter()
        .find(|(_, n)| n.as_bytes() == name)
        .map_or(NONE, |(bit, _)| *bit)
}

/// `HTTP_USER_ROLE`.
pub mod role {
    pub const NONE: u8 = 0;
    pub const ADMIN: u8 = 1;
    pub const MANAGER: u8 = 2;
    pub const TROUBLESHOOTER: u8 = 3;
    pub const OBSERVER: u8 = 4;
    pub const MEMBER: u8 = 5;
    pub const BILLING: u8 = 6;
    pub const ANY: u8 = 7;

    /// `http_id2user_role()`: the first name of the role in C's table.
    pub fn name(role: u8) -> &'static str {
        match role {
            ADMIN => "admin",
            MANAGER => "manager",
            TROUBLESHOOTER => "troubleshooter",
            OBSERVER => "observer",
            MEMBER => "member",
            BILLING => "billing",
            ANY => "any",
            _ => "none",
        }
    }

    /// `http_user_role2id()` without its record: an empty text is member, the aliases members, admins and all are
    /// accepted; `None` for another text (C logs it and takes none).
    pub fn from_name(name: &[u8]) -> Option<u8> {
        Some(match name {
            b"" | b"member" | b"members" => MEMBER,
            b"none" => NONE,
            b"admin" | b"admins" => ADMIN,
            b"manager" => MANAGER,
            b"troubleshooter" => TROUBLESHOOTER,
            b"observer" => OBSERVER,
            b"billing" => BILLING,
            b"any" | b"all" => ANY,
            _ => return None,
        })
    }
}

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
    fn names_in_bit_order() {
        assert_eq!(names(NONE).count(), 0);
        assert_eq!(names(ANONYMOUS_DATA).collect::<Vec<_>>(), ["anonymous-data"]);
        assert_eq!(
            names(EDIT_ALERTS_SILENCING | SIGNED_ID | SENSITIVE_DATA).collect::<Vec<_>>(),
            ["signed-in", "sensitive-data", "edit-alerts-silencing"]
        );
        assert_eq!(names(ALL).count(), 11);
        assert_eq!((role::name(role::ANY), role::name(role::MEMBER), role::name(9)), ("any", "member", "none"));
        assert_eq!((id_one(b"view-config"), id_one(b"none"), id_one(b"View-config")), (VIEW_AGENT_CONFIG, NONE, NONE));
        assert_eq!(
            [b"".as_slice(), b"admins", b"all", b"observer", b"nope"].map(role::from_name),
            [Some(role::MEMBER), Some(role::ADMIN), Some(role::ANY), Some(role::OBSERVER), None]
        );
    }

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
