//! `HTTP_ACCESS`, ported from `src/libnetdata/user-auth/http-access.{h,c}`: the permissions a caller holds and a
//! function requires.

use netdata_agent_text::c::c_str;
use netdata_agent_text::parse::{strtoull16, uuid_parse_flexi};

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

    /// `http_user_role2id()`: `from_name()`, an unknown name `NONE` after C's WARNING.
    pub fn to_id(name: &[u8]) -> u8 {
        from_name(name).unwrap_or_else(|| {
            use netdata_agent_log::{Priority, Source, nd_log};
            nd_log!(Source::Daemon, Priority::Warning, "HTTP user role '{}' is not valid", String::from_utf8_lossy(name));
            NONE
        })
    }
}

/// `HTTP_ACCESS_PERMISSION_DENIED_HTTP_CODE()`: 403 for a signed-in caller, else 412.
pub fn denied_code(user: u32) -> u16 {
    if user & SIGNED_ID != 0 { 403 } else { 412 }
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

/// `http_access_from_hex_str()`: hex bits, none for no text.
pub fn from_hex_str(s: &[u8]) -> u32 {
    if s.is_empty() { NONE } else { strtoull16(s).0 as u32 & ALL }
}

/// `USER_AUTH_METHOD`, with C's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Method {
    #[default]
    None,
    Cloud,
    Bearer,
    God,
}

impl Method {
    const NAMES: [(Method, &'static str); 4] =
        [(Method::None, "none"), (Method::Cloud, "NC"), (Method::Bearer, "api-bearer"), (Method::God, "god")];

    /// `USER_AUTH_METHOD_2id()`: an unknown name is `none`.
    pub fn from_name(name: &[u8]) -> Self {
        Self::NAMES.iter().find(|(_, n)| n.as_bytes() == name).map_or(Method::None, |&(m, _)| m)
    }

    /// `USER_AUTH_METHOD_2str()`.
    pub fn name(self) -> &'static str {
        Self::NAMES.iter().find(|(m, _)| *m == self).map_or("none", |&(_, n)| n)
    }
}

/// `CLOUD_CLIENT_NAME_LENGTH`: a user name keeps one byte less.
const CLIENT_NAME_LENGTH: usize = 64;
/// `INET6_ADDRSTRLEN`: an address keeps one byte less.
const ADDRESS_LENGTH: usize = 46;

/// `USER_AUTH`: a caller as a call's source string describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserAuth {
    pub method: Method,
    pub role: u8,
    pub access: u32,
    pub client_name: Vec<u8>,
    pub account: [u8; 16],
    pub client_ip: Vec<u8>,
    pub forwarded_for: Vec<u8>,
}

impl UserAuth {
    /// `user_auth_from_source()`, the inverse of `user_auth_to_source_buffer()`: `key=value` tokens between commas
    /// (empty ones and ones without `=` skipped, the last of a key kept, unknown keys ignored); `role=god` sets the
    /// method; the user name and the addresses are cut to C's buffers; an account that is not a UUID is zeros.
    pub fn from_source(src: &[u8]) -> Self {
        let cut = |value: &[u8], size: usize| value[..value.len().min(size - 1)].to_vec();
        let mut parsed = UserAuth::default();
        for token in c_str(src).split(|&c| c == b',') {
            let Some(eq) = token.iter().position(|&c| c == b'=') else {
                continue;
            };
            let value = &token[eq + 1..];
            match &token[..eq] {
                b"method" => parsed.method = Method::from_name(value),
                b"role" if value == b"god" => parsed.method = Method::God,
                b"role" => parsed.role = role::to_id(value),
                b"permissions" => parsed.access = from_hex_str(value),
                b"user" => parsed.client_name = cut(value, CLIENT_NAME_LENGTH),
                b"account" => parsed.account = uuid_parse_flexi(value).unwrap_or([0; 16]),
                b"ip" => parsed.client_ip = cut(value, ADDRESS_LENGTH),
                b"forwarded_for" => parsed.forwarded_for = cut(value, ADDRESS_LENGTH),
                _ => {}
            }
        }
        parsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `user_auth_from_source()` over the strings `user_auth_to_source_buffer()` writes and C's edges: empty and
    /// `=`-less tokens, a forwarded list cut at its comma, `role=god`, an empty role (member) and an unknown one (none,
    /// after the WARNING), the hex permissions masked, the cuts, a bad account.
    #[test]
    fn sources_parse_as_c() {
        let account = *b"\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xab";
        let (parsed, logged) = netdata_agent_log::capture(|| {
            UserAuth::from_source(
                b"method=api-bearer,role=admin,permissions=0x7ff,user=fnhttp-admin,\
                  account=000000000000000000000000000000ab,ip=localhost,forwarded_for=a, b",
            )
        });
        assert!(logged.is_empty());
        assert_eq!(
            parsed,
            UserAuth {
                method: Method::Bearer,
                role: role::ADMIN,
                access: ALL,
                client_name: b"fnhttp-admin".to_vec(),
                account,
                client_ip: b"localhost".to_vec(),
                forwarded_for: b"a".to_vec(),
            }
        );
        let (parsed, logged) =
            netdata_agent_log::capture(|| UserAuth::from_source(b",,noequals,role=god,role=,permissions=fffff,x=y"));
        assert!(logged.is_empty());
        assert_eq!(parsed, UserAuth { method: Method::God, role: role::MEMBER, access: ALL, ..UserAuth::default() });
        let (parsed, logged) = netdata_agent_log::capture(|| UserAuth::from_source(b"role=king,account=nope,method=x"));
        assert_eq!(parsed, UserAuth::default());
        assert_eq!(logged.into_iter().filter_map(|r| r.message).collect::<Vec<_>>(), ["HTTP user role 'king' is not valid"]);
        let long = UserAuth::from_source(format!("user={},ip={}", "u".repeat(70), "1".repeat(50)).as_bytes());
        assert_eq!((long.client_name.len(), long.client_ip.len()), (63, 45));
        assert_eq!([Method::None, Method::Cloud, Method::Bearer, Method::God].map(Method::name), ["none", "NC", "api-bearer", "god"]);
    }

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
