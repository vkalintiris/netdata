//! DynCfg's vocabulary (`src/libnetdata/inicfg/dyncfg.{c,h}`, `src/daemon/dyncfg/dyncfg-internals.h`): the node
//! types, source types, statuses and commands with C's names, the id rules, the file name escape, the commands every
//! node type supports, and the default response body.

use netdata_agent_nrpc::reply::{ContentType, Reply};
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_text::c::{is_print, is_space};
use netdata_agent_text::json::{JsonOptions, JsonWriter};

/// `DYNCFG_VERSION`: the file format this agent writes.
pub const VERSION: u64 = 1;

/// `DYNCFG_RESP_RUNNING`: accepted and running.
pub const RESP_RUNNING: u16 = 200;
/// `DYNCFG_RESP_ACCEPTED`: accepted, not running yet.
pub const RESP_ACCEPTED: u16 = 202;
/// `DYNCFG_RESP_ACCEPTED_DISABLED`: accepted, but disabled.
pub const RESP_ACCEPTED_DISABLED: u16 = 298;
/// `DYNCFG_RESP_ACCEPTED_RESTART_REQUIRED`: accepted, a restart applies it.
pub const RESP_ACCEPTED_RESTART_REQUIRED: u16 = 299;

/// `DYNCFG_RESP_SUCCESS()`.
pub fn resp_success(code: u16) -> bool {
    (200..=299).contains(&code)
}

/// A name table's lookup in C's order: an empty or unknown name is the first entry's value.
fn by_name<T: Copy>(table: &[(T, &'static str)], name: &[u8]) -> T {
    table.iter().find(|(_, n)| n.as_bytes() == name).map_or(table[0].0, |&(v, _)| v)
}

/// A value's name, the table's first name when it has none (C's fallback string).
fn name_of<T: Copy + PartialEq>(table: &[(T, &'static str)], value: T) -> &'static str {
    table.iter().find(|(v, _)| *v == value).map_or(table[0].1, |&(_, n)| n)
}

/// `DYNCFG_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Type {
    #[default]
    Single,
    Template,
    Job,
}

const TYPES: [(Type, &str); 3] = [(Type::Single, "single"), (Type::Template, "template"), (Type::Job, "job")];

impl Type {
    /// `dyncfg_type2id()`: an empty or unknown name is `single`.
    pub fn from_name(name: &[u8]) -> Self {
        by_name(&TYPES, name)
    }

    /// `dyncfg_id2type()`.
    pub fn name(self) -> &'static str {
        name_of(&TYPES, self)
    }
}

/// `DYNCFG_SOURCE_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceType {
    #[default]
    Internal,
    Stock,
    User,
    Dyncfg,
    Discovered,
}

const SOURCE_TYPES: [(SourceType, &str); 5] = [
    (SourceType::Internal, "internal"),
    (SourceType::Stock, "stock"),
    (SourceType::User, "user"),
    (SourceType::Dyncfg, "dyncfg"),
    (SourceType::Discovered, "discovered"),
];

impl SourceType {
    /// `dyncfg_source_type2id()`: an empty or unknown name is `internal`.
    pub fn from_name(name: &[u8]) -> Self {
        by_name(&SOURCE_TYPES, name)
    }

    /// `dyncfg_id2source_type()`.
    pub fn name(self) -> &'static str {
        name_of(&SOURCE_TYPES, self)
    }
}

/// `DYNCFG_STATUS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    None,
    /// The plugin accepted the configuration.
    Accepted,
    /// The plugin runs it.
    Running,
    /// The plugin fails to run it.
    Failed,
    /// A user disabled it.
    Disabled,
    /// No plugin claims it.
    Orphan,
    /// A special kind of failed configuration.
    Incomplete,
}

const STATUSES: [(Status, &str); 7] = [
    (Status::None, "none"),
    (Status::Accepted, "accepted"),
    (Status::Running, "running"),
    (Status::Failed, "failed"),
    (Status::Disabled, "disabled"),
    (Status::Orphan, "orphan"),
    (Status::Incomplete, "incomplete"),
];

impl Status {
    /// `dyncfg_status2id()`: an empty or unknown name is `none`.
    pub fn from_name(name: &[u8]) -> Self {
        by_name(&STATUSES, name)
    }

    /// `dyncfg_id2status()`.
    pub fn name(self) -> &'static str {
        name_of(&STATUSES, self)
    }

    /// `dyncfg_status_from_successful_response()`: what a 2xx answer makes the node.
    pub fn from_successful_response(code: u16) -> Self {
        match code {
            RESP_ACCEPTED_DISABLED => Status::Disabled,
            RESP_RUNNING => Status::Running,
            _ => Status::Accepted,
        }
    }
}

/// `DYNCFG_CMDS`: a set of commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cmds(pub u16);

impl Cmds {
    pub const NONE: Cmds = Cmds(0);
    pub const GET: Cmds = Cmds(1 << 0);
    pub const SCHEMA: Cmds = Cmds(1 << 1);
    pub const UPDATE: Cmds = Cmds(1 << 2);
    pub const ADD: Cmds = Cmds(1 << 3);
    pub const TEST: Cmds = Cmds(1 << 4);
    pub const REMOVE: Cmds = Cmds(1 << 5);
    pub const ENABLE: Cmds = Cmds(1 << 6);
    pub const DISABLE: Cmds = Cmds(1 << 7);
    pub const RESTART: Cmds = Cmds(1 << 8);
    pub const USERCONFIG: Cmds = Cmds(1 << 9);

    /// `cmd_map`: every command and its name, in C's order (the order every rendering follows).
    const MAP: [(Cmds, &'static str); 10] = [
        (Cmds::GET, "get"),
        (Cmds::SCHEMA, "schema"),
        (Cmds::UPDATE, "update"),
        (Cmds::ADD, "add"),
        (Cmds::TEST, "test"),
        (Cmds::REMOVE, "remove"),
        (Cmds::ENABLE, "enable"),
        (Cmds::DISABLE, "disable"),
        (Cmds::RESTART, "restart"),
        (Cmds::USERCONFIG, "userconfig"),
    ];

    pub fn contains(self, other: Cmds) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: Cmds) -> bool {
        self.0 & other.0 != 0
    }

    /// `dyncfg_cmds2id()`: the space-separated names, each matched exactly, unknown ones skipped.
    pub fn parse(cmds: &[u8]) -> Self {
        let mut result = Cmds::NONE;
        for word in cmds.split(|&c| c == b' ').filter(|w| !w.is_empty()) {
            if let Some(&(cmd, _)) = Self::MAP.iter().find(|(_, name)| name.as_bytes() == word) {
                result = result | cmd;
            }
        }
        result
    }

    /// The names of the commands in the set, in C's order.
    pub fn names(self) -> impl Iterator<Item = &'static str> {
        Self::MAP.into_iter().filter(move |&(cmd, _)| self.intersects(cmd)).map(|(_, name)| name)
    }

    /// `dyncfg_cmds2fp()`: each name followed by a space.
    pub fn write_spaced(self, out: &mut Vec<u8>) {
        for name in self.names() {
            out.extend_from_slice(name.as_bytes());
            out.push(b' ');
        }
    }

    /// `dyncfg_cmds2buffer()`: the names joined by single spaces.
    pub fn write_joined(self, out: &mut Vec<u8>) {
        for (i, name) in self.names().enumerate() {
            if i > 0 {
                out.push(b' ');
            }
            out.extend_from_slice(name.as_bytes());
        }
    }

    /// `dyncfg_id2cmd_one()`: the name of exactly one command; `None` (C's NULL, printed `(null)`) for anything else.
    pub fn name_one(self) -> Option<&'static str> {
        Self::MAP.iter().find(|(cmd, _)| *cmd == self).map(|&(_, name)| name)
    }

    /// `dyncfg_sanitize_cmds()`: every node has `schema`; `enable` and `disable` come together; only templates have
    /// `add`, and always; only the jobs DynCfg made can be removed, and always; templates have no data.
    pub fn sanitize(self, kind: Type, source_type: SourceType) -> Self {
        let mut cmds = self | Cmds::SCHEMA;
        if cmds.intersects(Cmds::ENABLE | Cmds::DISABLE) {
            cmds = cmds | Cmds::ENABLE | Cmds::DISABLE;
        }
        if kind == Type::Template {
            cmds = cmds | Cmds::ADD;
        } else {
            cmds = cmds - Cmds::ADD;
        }
        if source_type == SourceType::Dyncfg && kind == Type::Job {
            cmds = cmds | Cmds::REMOVE;
        } else {
            cmds = cmds - Cmds::REMOVE;
        }
        if kind == Type::Template {
            cmds = cmds - (Cmds::GET | Cmds::UPDATE);
        }
        cmds
    }
}

impl std::ops::BitOr for Cmds {
    type Output = Cmds;
    fn bitor(self, rhs: Cmds) -> Cmds {
        Cmds(self.0 | rhs.0)
    }
}

impl std::ops::Sub for Cmds {
    type Output = Cmds;
    fn sub(self, rhs: Cmds) -> Cmds {
        Cmds(self.0 & !rhs.0)
    }
}

/// `dyncfg_is_valid_id()`: no whitespace and no `'`.
pub fn is_valid_id(id: &[u8]) -> bool {
    !id.iter().any(|&c| is_space(c) || c == b'\'')
}

/// `dyncfg_escape_id_for_filename()`: whitespace, what is not printable (bytes from 0x80 too), `` ` ``, `$`, `/`, `:`
/// and `|` become `%XX` in uppercase hex; `%` itself stays, so two ids can share a file name.
pub fn escape_id_for_filename(id: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(id.len());
    for &c in id {
        if is_space(c) || !is_print(c) || matches!(c, b'`' | b'$' | b'/' | b':' | b'|') {
            escaped.extend_from_slice(format!("%{c:02X}").as_bytes());
        } else {
            escaped.push(c);
        }
    }
    escaped
}

/// `dyncfg_default_response()`: the reply becomes `{"status":code,"message":msg}`, JSON, expiring now.
pub fn default_response(reply: &mut Reply, code: u16, msg: &str) -> u16 {
    let mut w = JsonWriter::new(JsonOptions::MINIFY);
    w.member_add_uint64("status", u64::from(code));
    w.member_add_string("message", msg);
    w.finalize();
    reply.body = w.into_bytes();
    reply.content_type = ContentType::ApplicationJson;
    reply.expires = now_realtime_s();
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C's tables: names both ways, an empty or unknown name the first entry, a value without a name the first name.
    #[test]
    fn names_are_cs() {
        let types: Vec<_> = [b"single" as &[u8], b"template", b"job", b"", b"jobs"].map(Type::from_name).into();
        assert_eq!(types, [Type::Single, Type::Template, Type::Job, Type::Single, Type::Single]);
        assert_eq!([Type::Single, Type::Template, Type::Job].map(Type::name), ["single", "template", "job"]);
        let sources: Vec<_> =
            [b"internal" as &[u8], b"stock", b"user", b"dyncfg", b"discovered", b"", b"x"].map(SourceType::from_name).into();
        assert_eq!(
            sources,
            [
                SourceType::Internal,
                SourceType::Stock,
                SourceType::User,
                SourceType::Dyncfg,
                SourceType::Discovered,
                SourceType::Internal,
                SourceType::Internal
            ]
        );
        let statuses: Vec<_> = [b"orphan" as &[u8], b"incomplete", b"", b"Running"].map(Status::from_name).into();
        assert_eq!(statuses, [Status::Orphan, Status::Incomplete, Status::None, Status::None]);
        assert_eq!(
            [Status::None, Status::Accepted, Status::Running, Status::Failed, Status::Disabled].map(Status::name),
            ["none", "accepted", "running", "failed", "disabled"]
        );
    }

    /// `dyncfg_status_from_successful_response()`: 298 disabled, 200 running, any other code accepted.
    #[test]
    fn a_successful_response_sets_cs_status() {
        let statuses = [200, 202, 298, 299, 201, 250].map(Status::from_successful_response);
        assert_eq!(
            statuses,
            [Status::Running, Status::Accepted, Status::Disabled, Status::Accepted, Status::Accepted, Status::Accepted]
        );
        assert_eq!([199, 200, 299, 300].map(resp_success), [false, true, true, false]);
    }

    /// `dyncfg_cmds2id()` and the three renderings: exact words, extra spaces, unknown words skipped; C's order.
    #[test]
    fn commands_parse_and_render_as_c() {
        let cmds = Cmds::parse(b"  remove get  schemas schema enable ");
        assert_eq!(cmds, Cmds::REMOVE | Cmds::GET | Cmds::SCHEMA | Cmds::ENABLE);
        assert_eq!(Cmds::parse(b""), Cmds::NONE);
        assert_eq!(Cmds::parse(b"ge get_"), Cmds::NONE);
        let mut spaced = Vec::new();
        cmds.write_spaced(&mut spaced);
        assert_eq!(spaced, b"get schema remove enable ");
        let mut joined = Vec::new();
        cmds.write_joined(&mut joined);
        assert_eq!(joined, b"get schema remove enable");
        assert_eq!(Cmds::USERCONFIG.name_one(), Some("userconfig"));
        assert_eq!((Cmds::GET | Cmds::UPDATE).name_one(), None, "C prints (null)");
        assert_eq!(Cmds::NONE.name_one(), None);
    }

    /// `dyncfg_sanitize_cmds()`: each rule alone and the template's data removed after its add.
    #[test]
    fn commands_are_sanitized_as_c() {
        let all = Cmds(0x3ff);
        assert_eq!(Cmds::NONE.sanitize(Type::Single, SourceType::Internal), Cmds::SCHEMA);
        assert_eq!(Cmds::DISABLE.sanitize(Type::Single, SourceType::User), Cmds::SCHEMA | Cmds::ENABLE | Cmds::DISABLE);
        assert_eq!(all.sanitize(Type::Single, SourceType::Dyncfg), all - Cmds::ADD - Cmds::REMOVE);
        assert_eq!(Cmds::NONE.sanitize(Type::Template, SourceType::Stock), Cmds::SCHEMA | Cmds::ADD);
        assert_eq!(all.sanitize(Type::Template, SourceType::Dyncfg), all - Cmds::REMOVE - Cmds::GET - Cmds::UPDATE);
        assert_eq!(Cmds::GET.sanitize(Type::Job, SourceType::Dyncfg), Cmds::GET | Cmds::SCHEMA | Cmds::REMOVE);
        assert_eq!(Cmds::REMOVE.sanitize(Type::Job, SourceType::User), Cmds::SCHEMA);
    }

    /// `dyncfg_is_valid_id()` and `dyncfg_escape_id_for_filename()`, including the escape's collision through `%`.
    #[test]
    fn ids_and_file_names_are_cs() {
        assert!(is_valid_id(b"go.d:nginx:local"));
        assert!(is_valid_id(b""));
        assert!(!is_valid_id(b"a b") && !is_valid_id(b"a\tb") && !is_valid_id(b"a'b"));
        assert_eq!(escape_id_for_filename(b"go.d:nginx/x y`$|~\x7f\x80%"), b"go.d%3Anginx%2Fx%20y%60%24%7C~%7F%80%".to_vec());
        assert_eq!(escape_id_for_filename(b"a:b"), escape_id_for_filename(b"a%3Ab"), "not injective");
    }

    /// `dyncfg_default_response()`: C's minified body, JSON, expiring now.
    #[test]
    fn the_default_response_is_cs() {
        let mut reply = Reply::new(ContentType::TextPlain);
        let before = now_realtime_s();
        assert_eq!(default_response(&mut reply, 404, "Unknown config id given."), 404);
        assert_eq!(reply.body, br#"{"status":404,"message":"Unknown config id given."}"#);
        assert_eq!(reply.content_type, ContentType::ApplicationJson);
        assert!((before..=before + 1).contains(&reply.expires));
    }
}
