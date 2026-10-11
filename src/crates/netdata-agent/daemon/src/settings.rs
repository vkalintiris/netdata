//! `/api/v3/settings` (`src/web/api/v3/api_v3_settings.c`): the dashboard's stored preferences. One JSON document
//! per file under `<varlib>/settings/`: a GET returns the stored bytes, a PUT stores json-c's print of its payload
//! with `version` raised by one, when the payload names the version that is stored.

use std::fs::{File, Metadata};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use netdata_agent_log::{Priority, Source, errno_of, nd_log, take_errno};
use netdata_agent_nrpc::reply::Reply as NrpcReply;
use netdata_agent_query::request::pairs;
use netdata_agent_rrd::host::Host;
use netdata_agent_text::c::filename_from_path_entry;
use netdata_agent_text::jsonc_doc::{self, Value};
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::request::Mode;
use netdata_agent_web::status;
use nix::errno::Errno;
use nix::fcntl::OFlag;

use crate::functions::reply_of;
use crate::router::Route;
use crate::server::Reply;
use crate::status_file::io::read_text;

/// `MAX_SETTINGS_SIZE_BYTES`.
const MAX_SIZE: u64 = 20 * 1024 * 1024;

/// What a file that is missing, or gives no version, reads as (`settings_initial_version()`).
const INITIAL: &[u8] = br#"{"version":1}"#;

/// `settings_spinlock`: the read of a settings file never overlaps the write of one, and writers take turns.
static LOCK: RwLock<()> = RwLock::new(());

/// A refusal: its HTTP code and C's text.
type Refusal = (u16, &'static str);

/// `is_settings_file_valid()`: ASCII letters, digits, `-` and `_`, at least one of them.
fn valid_file(file: &[u8]) -> bool {
    !file.is_empty() && file.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

/// `settings_extract_json_version()`: a document's `version` as json-c reads it as an int, widened as C widens it
/// to a `size_t` (a negative one becomes huge); 0 for a text that does not parse or has no such member. With the
/// `errno` json-c leaves behind (`None`: it touched none).
fn version_of(json: &[u8]) -> (u64, Option<i32>) {
    let (doc, parsed_errno) = jsonc_doc::parse_errno(json);
    let Some(version) = doc.as_ref().and_then(|doc| doc.object_get(b"version")) else {
        return (0, parsed_errno);
    };
    let (version, errno) = version.get_int_errno();
    (i64::from(version) as u64, errno.or(parsed_errno))
}

/// `settings_get()`'s buffer: the stored file's bytes as they are when it parses and its version is not 0;
/// otherwise the initial document, with C's record when a file is there and gives no version. Nothing is made.
fn stored(dir: &str, file: &str) -> Vec<u8> {
    let path = filename_from_path_entry(dir, file, None);
    match read_text(Path::new(&path), MAX_SIZE) {
        Some(content) => match version_of(&content) {
            // C's logger attaches the thread's errno and clears it: what json-c left reading the file, else what the
            // request left before it (an unknown bearer token's ENOENT; C clears it at each receive): D242, D260
            (0, errno) => {
                let left = take_errno();
                nd_log!(Source::Daemon, Priority::Err, errno = errno.unwrap_or(left);
                    "file '{path}' cannot be parsed to extract version");
                INITIAL.to_vec()
            }
            _ => content,
        },
        None => INITIAL.to_vec(),
    }
}

/// `settings_open_tmp_file()`: the `.new` file, opened for writing without following a link. One that does not
/// exist is created (and must not appear meanwhile); a regular file that is there is reused and emptied, when
/// what was opened is still that file; anything else there is refused. A refusal is the `errno` C's record
/// carries: the failing call's, or `EINVAL` for what is no regular file or no longer the file that was looked at.
fn open_tmp(path: &Path) -> Result<File, i32> {
    let invalid = Errno::EINVAL as i32;
    let before = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => Some(meta),
        Ok(_) => return Err(invalid),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(errno_of(&e)),
    };
    let mut options = File::options();
    options.write(true).mode(0o666).custom_flags((OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW).bits());
    if before.is_none() {
        options.create_new(true);
    }
    let file = options.open(path).map_err(|e| errno_of(&e))?;
    let after = file.metadata().map_err(|e| errno_of(&e))?;
    let same = |before: &Metadata| (before.dev(), before.ino()) == (after.dev(), after.ino());
    if !after.file_type().is_file() || !before.as_ref().is_none_or(same) {
        return Err(invalid);
    }
    if before.is_some() {
        file.set_len(0).map_err(|e| errno_of(&e))?;
    }
    Ok(file)
}

/// `settings_put()`: under the one write lock, in C's order: the directory is made (before the payload is looked
/// at); the stored version is read; the payload must parse, have a `version`, and name the stored version; its
/// `version` becomes the next one where the member is; json-c's spaced print of it goes to `<file>.new`, which
/// then takes the file's place.
fn put(dir: &str, file: &str, payload: &[u8]) -> Result<(), Refusal> {
    let _writer = LOCK.write().unwrap_or_else(PoisonError::into_inner);
    // filename_is_dir(path, true)
    if !std::fs::metadata(dir).is_ok_and(|meta| meta.is_dir())
        && std::fs::DirBuilder::new().mode(0o750).create(dir).is_err()
    {
        return Err((status::BAD_REQUEST, "Settings path cannot be created or accessed."));
    }
    let (old_version, _) = version_of(&stored(dir, file));
    let Some(mut doc) = jsonc_doc::parse(payload) else {
        return Err((status::BAD_REQUEST, "Payload cannot be parsed as a JSON object"));
    };
    let Some(version) = doc.object_get(b"version") else {
        return Err((status::BAD_REQUEST, "Field version is not found in payload"));
    };
    let new_version = i64::from(version.get_int()) as u64;
    if old_version != new_version {
        return Err((status::CONFLICT, "Payload version does not match the version of the stored object"));
    }
    // json_object_new_int((int)new_version) after the increment
    doc.object_set(b"version", Value::Int(i64::from(new_version.wrapping_add(1) as i32)));
    let mut text = Vec::new();
    doc.print_spaced(&mut text);

    let tmp = filename_from_path_entry(dir, file, Some("new"));
    let mut fp = match open_tmp(Path::new(&tmp)) {
        Ok(fp) => fp,
        Err(errno) => {
            nd_log!(Source::Daemon, Priority::Err, errno = errno; "cannot open/create settings file '{tmp}'");
            return Err((status::INTERNAL_SERVER_ERROR, "Cannot create payload file"));
        }
    };
    // fwrite() then fclose(): either failing is a failed save, and the record's errno is the first failure's
    let written = fp.write_all(&text).map_err(|e| errno_of(&e));
    let closed = nix::unistd::close(fp).map_err(|e| e as i32);
    if let Err(errno) = written.and(closed) {
        let _ = std::fs::remove_file(&tmp);
        nd_log!(Source::Daemon, Priority::Err, errno = errno; "cannot save settings to file '{tmp}'");
        return Err((status::INTERNAL_SERVER_ERROR, "Cannot save payload to file"));
    }
    let path = filename_from_path_entry(dir, file, None);
    if let Err(e) = std::fs::rename(&tmp, &path) {
        // the record's errno is the last failure's: the removal's, when that fails too
        let errno = std::fs::remove_file(&tmp).err().map_or(errno_of(&e), |e| errno_of(&e));
        nd_log!(Source::Daemon, Priority::Err, errno = errno; "cannot rename file '{tmp}' to '{path}'");
        return Err((status::INTERNAL_SERVER_ERROR, "Failed to move the payload file to its final location"));
    }
    Ok(())
}

/// `nrpc_call_error()` as the web server sends it: `{"status":code,"errorMessage":text}`.
fn answer((code, text): Refusal) -> Reply {
    let mut reply = NrpcReply::new(ContentType::ApplicationJson);
    reply.error(text, code);
    let (date, expires) = (reply.expires - 1, reply.expires);
    let sent = reply_of(reply, code);
    // the buffer's own date and its expiry a second later stand for the 200; any other code is sent no-cache
    if code == status::OK { Reply { date, expires, ..sent } } else { sent }
}

/// `api_v3_settings()`: the checks in C's order: the last `file=` must be a valid name; the routed host must be
/// the agent's own; a client without a bearer token gets the file `default` alone; GET reads, PUT stores (a
/// payload is required, of at most 20 MiB), any other method is refused.
pub fn settings(route: &Route<'_>, host: &Arc<Host>, query: &[u8]) -> Reply {
    let file = pairs(query).filter(|(name, _)| *name == b"file").last().map(|(_, value)| value);
    let Some(file) = file.filter(|file| valid_file(file)) else {
        return answer((status::BAD_REQUEST, "Invalid settings file given."));
    };
    if !Arc::ptr_eq(host, route.shared.hosts.localhost()) {
        return answer((status::BAD_REQUEST, "Settings API is only allowed for the agent node."));
    }
    if !route.ctx.auth.is_bearer() && file != b"default" {
        return answer((status::BAD_REQUEST, "Only the 'default' settings file is allowed for anonymous users"));
    }
    let dir = filename_from_path_entry(&route.shared.varlib_dir, "settings", None);
    // (a valid name is ASCII)
    let file = String::from_utf8_lossy(file);
    match route.ctx.mode {
        // (the web server gives a request without a mode of its own, and an upgrade, the GET's)
        None | Some(Mode::Get | Mode::Websocket) => {
            let _reader = LOCK.read().unwrap_or_else(PoisonError::into_inner);
            Reply {
                code: status::OK,
                content_type: ContentType::ApplicationJson,
                body: stored(&dir, &file),
                ..Reply::default()
            }
        }
        Some(Mode::Put) => {
            let payload = route.payload.map_or(&[][..], |payload| payload.body.as_slice());
            if payload.is_empty() {
                return answer((status::BAD_REQUEST, "Settings API PUT action requires a payload."));
            }
            if payload.len() as u64 > MAX_SIZE {
                let too_long = "Settings API PUT payload exceeds the maximum allowed size.";
                return answer((status::CONTENT_TOO_LONG, too_long));
            }
            match put(&dir, &file, payload) {
                Ok(()) => answer((status::OK, "OK")),
                Err(refusal) => answer(refusal),
            }
        }
        Some(_) => answer((status::BAD_REQUEST, "Invalid HTTP mode. HTTP modes GET and PUT are supported.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> (tempfile::TempDir, String) {
        let top = tempfile::tempdir().unwrap();
        let settings = top.path().join("settings").to_str().unwrap().to_owned();
        (top, settings)
    }

    fn file_of(dir: &str, name: &str) -> Option<String> {
        std::fs::read(Path::new(dir).join(name)).ok().map(|bytes| String::from_utf8(bytes).unwrap())
    }

    fn get(dir: &str, file: &str) -> String {
        String::from_utf8(stored(dir, file)).unwrap()
    }

    #[test]
    fn a_file_name_is_letters_digits_dash_and_underscore() {
        for name in ["default", "a-b_C9", "0", "_", "-"] {
            assert!(valid_file(name.as_bytes()), "{name}");
        }
        for name in ["", "a.b", "../x", "a/b", "a b", "\u{e9}", "a\0", "a%"] {
            assert!(!valid_file(name.as_bytes()), "{name:?}");
        }
    }

    /// The dashboard's sequence on a fresh agent, with C's stored bytes: a GET reads the initial document and
    /// makes nothing; the first PUT makes the directory and stores json-c's print with the next version; a stale
    /// PUT is a conflict; the next version is stored over the file; a payload that does not parse, or has no
    /// version, is refused with C's texts and changes nothing.
    #[test]
    fn a_put_stores_the_next_version_of_what_it_names() {
        let (_top, dir) = dir();
        assert_eq!(get(&dir, "default"), r#"{"version":1}"#);
        assert!(!Path::new(&dir).exists());

        let first = br#"{"version":1,"value":{"preferred_node_ids":["0f4b6e5c"]}}"#;
        assert_eq!(put(&dir, "default", first), Ok(()));
        let stored_first = r#"{ "version": 2, "value": { "preferred_node_ids": [ "0f4b6e5c" ] } }"#;
        assert_eq!(get(&dir, "default"), stored_first);
        assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o7777 & !0o750, 0);
        assert_eq!(file_of(&dir, "default.new"), None);

        let conflict = (status::CONFLICT, "Payload version does not match the version of the stored object");
        assert_eq!(put(&dir, "default", first), Err(conflict));
        assert_eq!(put(&dir, "default", br#"{"version":2}"#), Ok(()));
        assert_eq!(get(&dir, "default"), r#"{ "version": 3 }"#);

        let unparsed = (status::BAD_REQUEST, "Payload cannot be parsed as a JSON object");
        assert_eq!(put(&dir, "default", b"not json"), Err(unparsed));
        let no_version = (status::BAD_REQUEST, "Field version is not found in payload");
        assert_eq!(put(&dir, "default", br#"{"value":1}"#), Err(no_version));
        assert_eq!(put(&dir, "default", b"[1]"), Err(no_version));
        assert_eq!(get(&dir, "default"), r#"{ "version": 3 }"#);

        // another file: json-c's print of every kind of value; a number with a fraction keeps its text
        let other = br#"{"version":1,"value":{"url":"http://x/y","n":1.50,"on":true,"off":null,"list":[]}}"#;
        assert_eq!(put(&dir, "other", other), Ok(()));
        let stored_other =
            r#"{ "version": 2, "value": { "url": "http:\/\/x\/y", "n": 1.50, "on": true, "off": null, "list": [ ] } }"#;
        assert_eq!(get(&dir, "other"), stored_other);
        assert_eq!(get(&dir, "default"), r#"{ "version": 3 }"#);
    }

    /// The version rule: `version` is read as json-c reads an int (a string by its digits, a double truncated, a
    /// boolean 0 or 1, anything else 0), and takes the next number where the member is; a repeated key keeps its
    /// first place and its last value. A failed first PUT has still made the directory.
    #[test]
    fn the_version_is_an_int_as_json_c_reads_one() {
        for accepted in [r#""1""#, "1.9", "true", "1e0", r#"" 1x""#] {
            let (_top, dir) = dir();
            let payload = format!(r#"{{"a":0,"version":{accepted},"z":9}}"#);
            assert_eq!(put(&dir, "default", payload.as_bytes()), Ok(()), "{accepted}");
            assert_eq!(get(&dir, "default"), r#"{ "a": 0, "version": 2, "z": 9 }"#, "{accepted}");
        }
        let conflict = (status::CONFLICT, "Payload version does not match the version of the stored object");
        for refused in ["null", "{}", "[1]", "false", "0", "2", "-1", r#""x""#] {
            let (_top, dir) = dir();
            let payload = format!(r#"{{"version":{refused}}}"#);
            assert_eq!(put(&dir, "default", payload.as_bytes()), Err(conflict), "{refused}");
            assert!(Path::new(&dir).is_dir() && file_of(&dir, "default").is_none(), "{refused}");
        }
        let (_top, dir) = dir();
        let repeated = br#"{"z":1,"version":1,"a":{"b":1,"b":2},"z":3}"#;
        assert_eq!(put(&dir, "default", repeated), Ok(()));
        assert_eq!(get(&dir, "default"), r#"{ "z": 3, "version": 2, "a": { "b": 2 } }"#);
    }

    /// A stored file that gives no version reads as the initial document, with C's record, and a PUT of version 1
    /// replaces it; one that gives a version is served as it is, byte for byte.
    #[test]
    fn a_stored_file_without_a_version_reads_as_the_initial_one() {
        for content in ["not json", r#"{"version":0}"#, "", r#"{"value":1}"#, "[]"] {
            let (_top, dir) = dir();
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(Path::new(&dir).join("default"), content).unwrap();
            let (read, records) = netdata_agent_log::capture(|| get(&dir, "default"));
            assert_eq!(read, r#"{"version":1}"#, "{content:?}");
            let messages: Vec<String> = records.into_iter().filter_map(|record| record.message).collect();
            assert_eq!(messages.len(), 1, "{content:?}: {messages:?}");
            assert!(messages[0].ends_with("/settings/default' cannot be parsed to extract version"), "{messages:?}");
            assert_eq!(put(&dir, "default", br#"{"version":1}"#), Ok(()), "{content:?}");
            assert_eq!(get(&dir, "default"), r#"{ "version": 2 }"#, "{content:?}");
        }
        let (_top, dir) = dir();
        std::fs::create_dir(&dir).unwrap();
        let verbatim = "{\"version\":2,\n  \"x\" : [1,2]}\n";
        std::fs::write(Path::new(&dir).join("default"), verbatim).unwrap();
        assert_eq!(get(&dir, "default"), verbatim);
        assert_eq!(put(&dir, "default", br#"{"version":2}"#), Ok(()));
        assert_eq!(get(&dir, "default"), r#"{ "version": 3 }"#);
    }

    /// C's record takes the thread's errno and clears it: what json-c left reading the file, else what the request left
    /// before it (an unknown bearer token's ENOENT, decision D260 in the status repository).
    #[test]
    fn the_record_takes_the_errno_the_request_left() {
        let enoent = Errno::ENOENT as i32;
        let mut stale_taken = 0;
        for content in ["not json", r#"{"version":0}"#, "", r#"{"value":1}"#, "[]", r#"{"version":"x"}"#] {
            let (_top, dir) = dir();
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(Path::new(&dir).join("default"), content).unwrap();
            let expected = version_of(content.as_bytes()).1.unwrap_or(enoent);
            stale_taken += usize::from(expected == enoent);
            netdata_agent_text::c::set_errno(enoent);
            let (_, records) = netdata_agent_log::capture(|| get(&dir, "default"));
            assert_eq!(records.iter().map(|record| record.errno).collect::<Vec<_>>(), [expected], "{content:?}");
            assert_eq!(take_errno(), 0, "{content:?}: the record cleared it");
        }
        assert!(stale_taken > 0, "a file json-c leaves no errno for takes the request's");
    }

    /// The `.new` file: a regular one left behind is reused and emptied; a link or a directory in its place is
    /// refused with C's text, and the stored file stays.
    #[test]
    fn the_new_file_is_never_a_link() {
        let (_top, dir) = dir();
        assert_eq!(put(&dir, "default", br#"{"version":1}"#), Ok(()));
        let new = Path::new(&dir).join("default.new");
        std::fs::write(&new, "left behind, and longer than the document that follows").unwrap();
        assert_eq!(put(&dir, "default", br#"{"version":2}"#), Ok(()));
        assert_eq!((get(&dir, "default"), new.exists()), (r#"{ "version": 3 }"#.to_owned(), false));

        let refused = Err((status::INTERNAL_SERVER_ERROR, "Cannot create payload file"));
        // C's record of it: an error of the daemon with the errno of what is no regular file
        let refused_with_its_record = || {
            let (result, records) = netdata_agent_log::capture(|| put(&dir, "default", br#"{"version":3}"#));
            assert_eq!(result, refused);
            let seen: Vec<_> = records.iter().map(|r| (r.priority, r.errno, r.message.as_deref())).collect();
            let message = format!("cannot open/create settings file '{dir}/default.new'");
            assert_eq!(seen, [(Priority::Err, nix::errno::Errno::EINVAL as i32, Some(message.as_str()))]);
        };
        let target = Path::new(&dir).join("elsewhere");
        std::os::unix::fs::symlink(&target, &new).unwrap();
        refused_with_its_record();
        assert!(!target.exists() && new.is_symlink());
        std::fs::remove_file(&new).unwrap();
        std::fs::create_dir(&new).unwrap();
        refused_with_its_record();
        assert_eq!(get(&dir, "default"), r#"{ "version": 3 }"#);
    }

    /// A rename that fails (the file's place is taken by a directory that is not empty): C's text, its record
    /// with the rename's errno, and the `.new` file removed.
    #[test]
    fn a_failed_rename_removes_the_new_file() {
        let (_top, dir) = dir();
        std::fs::create_dir_all(Path::new(&dir).join("default").join("inside")).unwrap();
        let (result, records) = netdata_agent_log::capture(|| put(&dir, "default", br#"{"version":1}"#));
        let moved = "Failed to move the payload file to its final location";
        assert_eq!(result, Err((status::INTERNAL_SERVER_ERROR, moved)));
        let seen: Vec<_> = records.iter().map(|r| (r.priority, r.errno, r.message.as_deref())).collect();
        let message = format!("cannot rename file '{dir}/default.new' to '{dir}/default'");
        assert_eq!(seen, [(Priority::Err, nix::errno::Errno::EISDIR as i32, Some(message.as_str()))]);
        assert!(!Path::new(&dir).join("default.new").exists());
    }

    /// The version at the limits of C's `int`: the next of the highest is the lowest, and the next of -1 is 0,
    /// which then reads as a file without a version.
    #[test]
    fn the_next_version_wraps_as_c_int_does() {
        let (_top, dir) = dir();
        std::fs::create_dir(&dir).unwrap();
        let file = Path::new(&dir).join("default");
        std::fs::write(&file, r#"{"version":2147483647}"#).unwrap();
        assert_eq!(put(&dir, "default", br#"{"version":2147483647,"a":1}"#), Ok(()));
        assert_eq!(get(&dir, "default"), r#"{ "version": -2147483648, "a": 1 }"#);
        // above the 32 bits, a payload's version reads as the highest: a conflict with the lowest that is stored
        let conflict = (status::CONFLICT, "Payload version does not match the version of the stored object");
        assert_eq!(put(&dir, "default", br#"{"version":2147483648}"#), Err(conflict));
        assert_eq!(put(&dir, "default", br#"{"version":-2147483648}"#), Ok(()));
        assert_eq!(get(&dir, "default"), r#"{ "version": -2147483647 }"#);
        std::fs::write(&file, r#"{"version":-1}"#).unwrap();
        assert_eq!(put(&dir, "default", br#"{"version":-1}"#), Ok(()));
        assert_eq!(file_of(&dir, "default").as_deref(), Some(r#"{ "version": 0 }"#));
        assert_eq!(get(&dir, "default"), r#"{"version":1}"#);
    }
}
