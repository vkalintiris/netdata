//! DynCfg's saved configurations (`src/daemon/dyncfg/dyncfg-files.c`): `<varlib>/config/<escaped id>.dyncfg`, C's
//! `key=value` lines, then `---` and the payload's bytes. Rendering and parsing are pure; the caller reads, writes,
//! renames and deletes the files. The schemas the plugins ship are read here.

use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_nrpc::reply::ContentType;
use netdata_agent_pluginsd_proto::LINE_MAX;
use netdata_agent_text::c::{c_str, fgets_chunks, trim};
use netdata_agent_text::parse::{strtoull10, uuid_parse_flexi};
use netdata_agent_text::print::print_uuid_lower_compact;

use crate::model::{Cmds, SourceType, Type, VERSION, escape_id_for_filename};

/// `DYNCFG_MAX_PAYLOAD_SIZE`: a larger payload marks the file corrupt.
pub const MAX_PAYLOAD_SIZE: usize = 20 * 1024 * 1024;

/// A saved payload and its content type; `None` is C's `CT_NONE`, a file without `content_type`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Payload {
    pub bytes: Vec<u8>,
    pub content_type: Option<ContentType>,
}

/// What a file holds of a node: its identity, the saved (`dyncfg`) half of its state, its commands.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Saved {
    pub template: Option<Vec<u8>>,
    pub host_uuid: [u8; 16],
    pub path: Vec<u8>,
    pub kind: Type,
    pub cmds: Cmds,
    pub sync: bool,
    pub source_type: SourceType,
    pub source: Vec<u8>,
    pub created_ut: u64,
    pub modified_ut: u64,
    pub user_disabled: bool,
    pub saves: u32,
    pub payload: Option<Payload>,
}

/// The file name of `id` in the configuration directory.
pub fn file_name(id: &[u8]) -> Vec<u8> {
    let mut name = escape_id_for_filename(id);
    name.extend_from_slice(b".dyncfg");
    name
}

/// `dyncfg_get_schema()`: `schema.d/<id>.json` of the user's configuration directory, then of the stock one, each by
/// the id's file-name form first and then as it is.
pub fn schema(user_config_dir: &Path, stock_config_dir: &Path, id: &[u8]) -> Option<Vec<u8>> {
    let escaped = escape_id_for_filename(id);
    [user_config_dir, stock_config_dir].into_iter().find_map(|dir| {
        [escaped.as_slice(), id].into_iter().find_map(|name| {
            let mut file = [name, b".json"].concat();
            file.splice(0..0, b"schema.d/".iter().copied());
            read_regular(&dir.join(OsStr::from_bytes(&file)))
        })
    })
}

/// `dyncfg_read_file_to_buffer()`: a regular file's bytes, read to its size at open (a short read fails it).
fn read_regular(filename: &Path) -> Option<Vec<u8>> {
    if !fs::metadata(filename).ok()?.is_file() {
        return None;
    }
    let file = fs::File::open(filename).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > u64::from(u32::MAX - 2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(meta.len()).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 == meta.len()).then_some(bytes)
}

/// `dyncfg_file_save()`'s contents: the save is stamped (`modified` now, `created` when it had none) and counted
/// before it is written; the payload section only for a payload with bytes.
pub fn render(id: &[u8], saved: &mut Saved, now_ut: u64) -> Vec<u8> {
    saved.modified_ut = now_ut;
    if saved.created_ut == 0 {
        saved.created_ut = saved.modified_ut;
    }
    saved.saves = saved.saves.wrapping_add(1);
    let mut out = Vec::new();
    let mut line = |key: &str, value: &[u8]| {
        out.extend_from_slice(key.as_bytes());
        out.push(b'=');
        out.extend_from_slice(value);
        out.push(b'\n');
    };
    line("version", VERSION.to_string().as_bytes());
    line("id", id);
    if let Some(template) = &saved.template {
        line("template", template);
    }
    let mut host = Vec::new();
    print_uuid_lower_compact(&mut host, &saved.host_uuid);
    line("host", &host);
    line("path", &saved.path);
    line("type", saved.kind.name().as_bytes());
    line("source_type", saved.source_type.name().as_bytes());
    line("source", &saved.source);
    line("created", saved.created_ut.to_string().as_bytes());
    line("modified", saved.modified_ut.to_string().as_bytes());
    line("sync", if saved.sync { b"true" } else { b"false" });
    line("user_disabled", if saved.user_disabled { b"true" } else { b"false" });
    line("saves", saved.saves.to_string().as_bytes());
    let mut cmds = Vec::new();
    saved.cmds.write_spaced(&mut cmds);
    line("cmds", &cmds);
    if let Some(payload) = saved.payload.as_ref().filter(|p| !p.bytes.is_empty()) {
        let content_type = payload.content_type.map_or("text/plain", ContentType::name);
        line("content_type", content_type.as_bytes());
        line("content_length", payload.bytes.len().to_string().as_bytes());
        out.extend_from_slice(b"---\n");
        out.extend_from_slice(&payload.bytes);
    }
    out
}

/// `dyncfg_file_load()`'s parse of `data`, read from `filename` (for the records): the id and what the file saved,
/// its commands sanitized for its type and saved source type, or `None` after C's record. Lines are read as C's
/// `fgets()` reads them (`PLUGINSD_LINE_MAX`), keys and values trimmed, empty values and unknown keys skipped, the last
/// of a repeated key kept; the payload is every byte after a `---` line, whatever `content_length` says.
pub fn parse(data: &[u8], filename: &str) -> Option<(Vec<u8>, Saved)> {
    let mut saved = Saved::default();
    let mut id = None;
    let mut content_type = None;
    let mut content_length = 0;
    let mut payload_at = None;
    let mut offset = 0;
    for chunk in fgets_chunks(data, LINE_MAX) {
        offset += chunk.len();
        let line = c_str(chunk);
        if line == b"---\n" {
            payload_at = Some(offset);
            break;
        }
        let Some(eq) = line.iter().position(|&c| c == b'=') else {
            continue;
        };
        let (Some(value), Some(key)) = (trim(&line[eq + 1..]), trim(&line[..eq])) else {
            continue;
        };
        let number = || strtoull10(value).0;
        match key {
            b"version" => {
                let version = number();
                if version > VERSION {
                    nd_log!(
                        Source::Daemon,
                        Priority::Notice,
                        "DYNCFG: configuration file '{filename}' has version {version}, which is newer than our version \
                         {VERSION}"
                    );
                }
            }
            b"id" => id = Some(value.to_vec()),
            b"template" => saved.template = Some(value.to_vec()),
            b"host" => {
                if let Some(uuid) = uuid_parse_flexi(value) {
                    saved.host_uuid = uuid;
                }
            }
            b"path" => saved.path = value.to_vec(),
            b"type" => saved.kind = Type::from_name(value),
            b"source_type" => saved.source_type = SourceType::from_name(value),
            b"source" => saved.source = value.to_vec(),
            b"created" => saved.created_ut = number(),
            b"modified" => saved.modified_ut = number(),
            b"sync" => saved.sync = value == b"true",
            b"user_disabled" => saved.user_disabled = value == b"true",
            // C's uint32_t keeps the low bits
            b"saves" => saved.saves = number() as u32,
            b"content_type" => content_type = Some(ContentType::from_name(value)),
            b"content_length" => content_length = number(),
            b"cmds" => saved.cmds = Cmds::parse(value),
            _ => {}
        }
    }
    if let Some(at) = payload_at {
        let bytes = &data[at..];
        if bytes.len() > MAX_PAYLOAD_SIZE {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "DYNCFG: payload size {} exceeds the maximum allowed {MAX_PAYLOAD_SIZE} for file '{filename}'. Ignoring \
                 it.",
                bytes.len()
            );
            return None;
        }
        if content_length != bytes.len() as u64 {
            nd_log!(
                Source::Daemon,
                Priority::Warning,
                "DYNCFG: content_length {content_length} does not match actual payload size {} for file '{filename}'",
                bytes.len()
            );
        }
        saved.payload = Some(Payload { bytes: bytes.to_vec(), content_type });
    }
    let Some(id) = id else {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "DYNCFG: configuration file '{filename}' does not include a unique id. Ignoring it."
        );
        return None;
    };
    // the loaded node's current state is its saved one, so its saved source type sanitizes its commands
    saved.cmds = saved.cmds.sanitize(saved.kind, saved.source_type);
    Some((id, saved))
}

#[cfg(test)]
mod tests {
    use netdata_agent_log::{Captured, capture};

    use super::*;

    fn records(records: Vec<Captured>) -> Vec<(Priority, String)> {
        records.into_iter().map(|r| (r.priority, r.message.unwrap_or_default())).collect()
    }

    type Parsed = (Option<(Vec<u8>, Saved)>, Vec<(Priority, String)>);

    fn parsed(text: &[u8]) -> Parsed {
        let (loaded, captured) = capture(|| parse(text, "/var/lib/netdata/config/x.dyncfg"));
        (loaded, records(captured))
    }

    /// C's writer vector (`dyncfg-unittest.c:654-711`): the stamps and the count, every line in C's order, the payload
    /// with its NUL kept; read back whole.
    #[test]
    fn a_saved_file_is_cs_and_reads_back() {
        let mut saved = Saved {
            template: Some(b"unittest:dyncfg-load".to_vec()),
            path: b"/unittests/dyncfg-load".to_vec(),
            cmds: Cmds::GET | Cmds::SCHEMA | Cmds::UPDATE | Cmds::REMOVE,
            kind: Type::Job,
            sync: true,
            user_disabled: true,
            source_type: SourceType::Dyncfg,
            source: b"dyncfg-file-unittest".to_vec(),
            payload: Some(Payload { bytes: b"{}\0x".to_vec(), content_type: Some(ContentType::ApplicationJson) }),
            created_ut: 1,
            modified_ut: 2,
            ..Saved::default()
        };
        let text = render(b"unittest:dyncfg-load:writer", &mut saved, 1_790_000_000_000_000);
        assert_eq!(
            String::from_utf8_lossy(&text),
            "version=1\nid=unittest:dyncfg-load:writer\ntemplate=unittest:dyncfg-load\n\
             host=00000000000000000000000000000000\npath=/unittests/dyncfg-load\ntype=job\nsource_type=dyncfg\n\
             source=dyncfg-file-unittest\ncreated=1\nmodified=1790000000000000\nsync=true\nuser_disabled=true\nsaves=1\n\
             cmds=get schema update remove \ncontent_type=application/json\ncontent_length=4\n---\n{}\0x"
        );
        let (loaded, logged) = parsed(&text);
        assert_eq!(logged, []);
        assert_eq!(loaded, Some((b"unittest:dyncfg-load:writer".to_vec(), saved)));
    }

    /// A first save stamps `created` too; a node without template, payload or commands writes no such lines, and its
    /// empty payload no section.
    #[test]
    fn a_first_save_stamps_both_times() {
        let mut saved = Saved {
            host_uuid: [0xab; 16],
            payload: Some(Payload { bytes: Vec::new(), content_type: Some(ContentType::ApplicationJson) }),
            saves: u32::MAX,
            ..Saved::default()
        };
        let text = render(b"x", &mut saved, 7);
        assert_eq!(
            String::from_utf8_lossy(&text),
            "version=1\nid=x\nhost=abababababababababababababababab\npath=\ntype=single\nsource_type=internal\nsource=\n\
             created=7\nmodified=7\nsync=false\nuser_disabled=false\nsaves=0\ncmds=\n"
        );
    }

    /// C's load vectors (`dyncfg-unittest.c:712-787`): a newer version and unknown keys, malformed optional values, an
    /// empty payload, an advisory length, no id or an empty one, an id with spaces.
    #[test]
    fn files_load_as_cs_vectors() {
        let (loaded, logged) = parsed(b"version=99\nfuture_key=future value\nid=unittest:dyncfg-load:minimal\n");
        let (id, saved) = loaded.unwrap();
        assert_eq!((id, saved.kind, saved.payload), (b"unittest:dyncfg-load:minimal".to_vec(), Type::Single, None));
        assert_eq!(
            logged,
            [(
                Priority::Notice,
                "DYNCFG: configuration file '/var/lib/netdata/config/x.dyncfg' has version 99, which is newer than our \
                 version 1"
                    .to_string()
            )]
        );
        let (loaded, logged) = parsed(
            b"id=unittest:dyncfg-load:malformed-metadata\ntype=not-a-type\nsource_type=not-a-source\n\
              created=not-a-number\nmodified=-\nsync=not-a-bool\nuser_disabled=not-a-bool\nsaves=not-a-number\n\
              cmds=not-a-command\n",
        );
        let saved = loaded.unwrap().1;
        assert_eq!(logged, []);
        assert_eq!(saved, Saved { cmds: Cmds::SCHEMA, ..Saved::default() });
        let (loaded, _) = parsed(b"id=e\ncontent_type=application/json\ncontent_length=0\n---\n");
        assert_eq!(
            loaded.unwrap().1.payload,
            Some(Payload { bytes: Vec::new(), content_type: Some(ContentType::ApplicationJson) })
        );
        let (loaded, logged) = parsed(b"id=m\ncontent_type=application/json\ncontent_length=99\n---\nabc");
        assert_eq!(loaded.unwrap().1.payload.unwrap().bytes, b"abc");
        assert_eq!(
            logged,
            [(
                Priority::Warning,
                "DYNCFG: content_length 99 does not match actual payload size 3 for file \
                 '/var/lib/netdata/config/x.dyncfg'"
                    .to_string()
            )]
        );
        let no_id = (
            Priority::Err,
            "DYNCFG: configuration file '/var/lib/netdata/config/x.dyncfg' does not include a unique id. Ignoring it."
                .to_string(),
        );
        let (loaded, logged) =
            parsed(b"path=/unittests\nsource=cleanup-check\ncontent_type=application/json\ncontent_length=2\n---\n{}");
        assert_eq!((loaded, logged), (None, vec![no_id.clone()]));
        let (loaded, logged) = parsed(b"id=\npath=/unittests\n");
        assert_eq!((loaded, logged), (None, vec![no_id]));
        let (loaded, _) = parsed(b"id=unittest dyncfg load compatible\npath=/unittests\n");
        assert_eq!(loaded.unwrap().0, b"unittest dyncfg load compatible");
    }

    /// The reading as C's: keys and values trimmed, a line without `=` or with an empty side skipped, the last of a
    /// repeated key kept, a bad host keeping zeros, a NUL ending a line, `---` only as a whole line, a type of
    /// content unknown read as text/plain and none as `CT_NONE`, the saves' low 32 bits, sanitized commands.
    #[test]
    fn lines_read_as_c() {
        let (loaded, _) = parsed(
            b"  id = a \nno-equals\n=x\ntemplate=\nid=b\nhost=not-a-uuid\npath=p\0ignored=1\nsaves=4294967298\n\
              type=job\nsource_type=dyncfg\ncmds=get update\ncontent_type=application/x-unknown\n---",
        );
        let (id, saved) = loaded.unwrap();
        assert_eq!(id, b"b");
        assert_eq!(
            saved,
            Saved {
                path: b"p".to_vec(),
                saves: 2,
                kind: Type::Job,
                source_type: SourceType::Dyncfg,
                cmds: Cmds::GET | Cmds::UPDATE | Cmds::SCHEMA | Cmds::REMOVE,
                ..Saved::default()
            }
        );
        let (loaded, _) = parsed(b"id=t\nhost=5a1e0000-0000-4000-8000-0000000000aa\ncontent_type=application/x-unknown\n---\nz");
        let saved = loaded.unwrap().1;
        assert_eq!(saved.host_uuid[0], 0x5a);
        assert_eq!(saved.payload.unwrap().content_type, Some(ContentType::TextPlain));
        let (loaded, _) = parsed(b"id=n\n---\nz");
        assert_eq!(loaded.unwrap().1.payload.unwrap().content_type, None);
    }

    /// C reads lines with `fgets(line, PLUGINSD_LINE_MAX)`: a longer line is read in pieces, each a line of its own,
    /// so a value past the first piece is cut and the rest is read as another (here unknown) key.
    #[test]
    fn a_long_line_is_read_in_fgets_pieces() {
        let mut text = b"id=".to_vec();
        text.extend(std::iter::repeat_n(b'a', LINE_MAX - 4));
        text.extend_from_slice(b"tail=x\npath=p\n");
        let (loaded, _) = parsed(&text);
        let (id, saved) = loaded.unwrap();
        assert_eq!(id.len(), LINE_MAX - 4);
        assert_eq!(saved.path, b"p");
    }

    /// A payload over 20 MiB marks the file corrupt; at the limit it loads.
    #[test]
    fn a_payload_over_the_limit_ignores_the_file() {
        let mut text = b"id=big\ncontent_length=0\n---\n".to_vec();
        text.resize(text.len() + MAX_PAYLOAD_SIZE, b'x');
        assert!(parse(&text, "f").is_some_and(|(_, s)| s.payload.unwrap().bytes.len() == MAX_PAYLOAD_SIZE));
        text.push(b'x');
        let (loaded, captured) = capture(|| parse(&text, "f"));
        assert_eq!(loaded, None);
        assert_eq!(
            records(captured)[0],
            (
                Priority::Err,
                format!(
                    "DYNCFG: payload size {} exceeds the maximum allowed 20971520 for file 'f'. Ignoring it.",
                    MAX_PAYLOAD_SIZE + 1
                )
            )
        );
    }

    /// The file name: the escaped id and `.dyncfg`.
    #[test]
    fn file_names_are_cs() {
        assert_eq!(file_name(b"go.d:nginx:local"), b"go.d%3Anginx%3Alocal.dyncfg");
    }
}
