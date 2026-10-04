//! The management API (`src/web/api/v1/api_v1_manage.c`): the key a request must carry, read from or written to its
//! file at the start, and the route `/api/v1/manage/health`, whose requests the health crate's silencers answer.

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;

use netdata_agent_log::{errno_of, netdata_log_error_errno, netdata_log_info};
use netdata_agent_text::c::{c_str, set_errno};
use netdata_agent_text::parse::uuid_parse_flexi;
use netdata_agent_text::print::print_uuid_lower;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::status;
use nix::fcntl::OFlag;

use netdata_agent_inicfg::SECTION_REGISTRY;

use crate::conf::Conf;
use crate::router::{Host, Route};
use crate::server::Reply;

/// `GUID_LEN`.
const GUID_LEN: usize = 36;

/// `HLT_MGM`.
const MANAGE_HEALTH: &[u8] = b"manage/health";

/// `regenerate_guid()`: a GUID in lower case, or C's record of a text that is none.
fn regenerate_guid(guid: &[u8]) -> Option<Vec<u8>> {
    let Some(uuid) = uuid_parse_flexi(guid) else {
        netdata_log_info!("Registry: GUID '{}' is not a valid GUID.", String::from_utf8_lossy(c_str(guid)));
        return None;
    };
    let mut text = Vec::with_capacity(GUID_LEN);
    print_uuid_lower(&mut text, &uuid);
    Some(text)
}

/// The key as its file holds it: the file's first 36 bytes, when the file is a regular one (a link is not followed)
/// and they are a GUID. The file is not rewritten: a key in upper case stays so in the file and is answered to in
/// lower case.
fn read_key(filename: &str) -> Option<Vec<u8>> {
    let is_regular = std::fs::symlink_metadata(filename).is_ok_and(|metadata| metadata.is_file());
    if !is_regular {
        return None;
    }
    let flags = (OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW).bits();
    let mut file = std::fs::OpenOptions::new().read(true).custom_flags(flags).open(filename).ok()?;
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        netdata_log_error_errno!("Management API key file '{filename}' is not a regular file, regenerating.");
        return None;
    }
    let mut buf = [0u8; GUID_LEN];
    match file.read(&mut buf) {
        Ok(GUID_LEN) => {}
        read => {
            if let Err(e) = read {
                set_errno(errno_of(&e));
            }
            netdata_log_error_errno!("Failed to read management API key from '{filename}'");
            return None;
        }
    }
    let key = regenerate_guid(&buf);
    if key.is_none() {
        let text = String::from_utf8_lossy(c_str(&buf));
        netdata_log_error_errno!("Failed to validate management API key '{text}' from '{filename}'.");
    }
    key
}

/// A new key's save: false with C's record when the file cannot hold it.
fn save_key(filename: &str, key: &[u8]) -> bool {
    if std::fs::symlink_metadata(filename).is_ok_and(|metadata| !metadata.is_file()) {
        netdata_log_error_errno!("Management API key file '{filename}' is not a regular file.");
        return false;
    }
    // O_RDWR, not O_WRONLY: a FIFO does not block; no O_TRUNC: the file is cut once it is known to be a regular one
    let flags = (OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW).bits();
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).mode(0o600).custom_flags(flags);
    let mut file = match options.open(filename) {
        Ok(file) => file,
        Err(e) => {
            set_errno(errno_of(&e));
            netdata_log_error_errno!(
                "Cannot create unique management API key file '{filename}'. Please adjust config parameter 'netdata \
                 management api key file' to a proper path and file."
            );
            return false;
        }
    };
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        netdata_log_error_errno!("Management API key file '{filename}' is not a regular file.");
        return false;
    }
    if let Err(e) = file.set_len(0) {
        set_errno(errno_of(&e));
        netdata_log_error_errno!("Cannot truncate management API key file '{filename}'.");
        return false;
    }
    match file.write(key) {
        Ok(GUID_LEN) => true,
        written => {
            if let Err(e) = written {
                set_errno(errno_of(&e));
            }
            netdata_log_error_errno!(
                "Cannot write the unique management API key file '{filename}'. Please adjust config parameter \
                 'netdata management api key file' to a proper path and file with enough space left."
            );
            false
        }
    }
}

/// `api_v1_management_init()`: the management key. `[registry] netdata management api key file` (default
/// `<varlib>/netdata.api.key`) is read; without a valid key in it a random one is made and saved there, with mode
/// 0600. A key that cannot be saved is good for this run only, and C tells it to the log.
pub fn management_init(conf: &mut Conf) -> Vec<u8> {
    let default = format!("{}/netdata.api.key", conf.dirs.varlib);
    let filename = conf.netdata.get_filename(SECTION_REGISTRY, "netdata management api key file", Some(&default));
    let filename = String::from_utf8_lossy(&filename.unwrap_or_default()).into_owned();

    if let Some(key) = read_key(&filename) {
        return key;
    }
    let key = uuid::Uuid::new_v4().hyphenated().to_string().into_bytes();
    if !save_key(&filename, &key) {
        netdata_log_info!(
            "You can still continue to use the alarm management API using the authorization token {} during this \
             Netdata session only.",
            String::from_utf8_lossy(&key)
        );
    }
    key
}

/// `api_v1_manage()`: the first `manage/health` of the decoded path must end it; then the request is the
/// silencers'. The host of a `/host/<name>/` prefix is not looked at.
pub fn api_v1_manage(route: &Route<'_>, _host: &Host, query: &[u8]) -> Reply {
    let path = route.path_decoded;
    let Some(at) = path.windows(MANAGE_HEALTH.len()).position(|window| window == MANAGE_HEALTH) else {
        // "Curently" is C's
        return Reply::text(status::NOT_FOUND, "Invalid management request. Curently only 'health' is supported.");
    };
    if at + MANAGE_HEALTH.len() != path.len() {
        return Reply::text(status::NOT_FOUND, "Invalid management request. Currently only 'health' is supported.");
    }
    let shared = route.shared;
    let reply = shared.health.silencers().request(route.auth_token, &shared.management_key, query);
    Reply {
        code: reply.code,
        content_type: if reply.json { ContentType::ApplicationJson } else { ContentType::TextPlain },
        body: reply.body,
        ..Reply::default()
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    const KEY: &[u8] = b"5a1e0000-0000-4000-8000-00000000c0de";

    /// The key the start makes of a key file's state, with the messages of its records.
    fn init(filename: &std::path::Path) -> (Vec<u8>, Vec<String>) {
        let mut conf = Conf::default();
        conf.netdata.set(SECTION_REGISTRY, "netdata management api key file", &filename.to_string_lossy());
        let (key, records) = netdata_agent_log::capture(|| management_init(&mut conf));
        (key, records.into_iter().filter_map(|record| record.message).collect())
    }

    fn is_guid(key: &[u8]) -> bool {
        key.len() == GUID_LEN && regenerate_guid(key).as_deref() == Some(key)
    }

    #[test]
    fn the_key_is_the_file_s_or_a_new_one_saved_there() {
        let dir = tempfile::tempdir().expect("a directory");
        let file = dir.path().join("netdata.api.key");
        let name = file.to_string_lossy().into_owned();

        // no file: a new key, saved without a newline, for the owner alone
        let (key, records) = init(&file);
        assert!(is_guid(&key) && records.is_empty(), "{records:?}");
        assert_eq!(std::fs::read(&file).expect("the file"), key);
        assert_eq!(std::fs::metadata(&file).expect("the file").permissions().mode() & 0o777, 0o600);
        // the file's key is kept
        assert_eq!(init(&file), (key, Vec::new()));

        // what follows the 36 bytes is not read; a key in upper case is answered to in lower case, and stays
        std::fs::write(&file, [KEY, b"\nmore"].concat()).expect("the file");
        assert_eq!(init(&file), (KEY.to_vec(), Vec::new()));
        std::fs::write(&file, KEY.to_ascii_uppercase()).expect("the file");
        assert_eq!(init(&file), (KEY.to_vec(), Vec::new()));
        assert_eq!(std::fs::read(&file).expect("the file"), KEY.to_ascii_uppercase());

        // a short file, and a text that is no GUID: C's records, and a new key in the file
        std::fs::write(&file, &KEY[..35]).expect("the file");
        let (key, records) = init(&file);
        assert_eq!(records, [format!("Failed to read management API key from '{name}'")]);
        assert!(is_guid(&key) && key != KEY);
        assert_eq!(std::fs::read(&file).expect("the file"), key);
        let text = "this is not a key, whatever it holds!";
        std::fs::write(&file, text).expect("the file");
        let (key, records) = init(&file);
        assert_eq!(
            records,
            [
                format!("Registry: GUID '{}' is not a valid GUID.", &text[..36]),
                format!("Failed to validate management API key '{}' from '{name}'.", &text[..36]),
            ]
        );
        assert_eq!(std::fs::read(&file).expect("the file"), key);
    }

    #[test]
    fn a_key_that_cannot_be_saved_is_told_to_the_log() {
        let dir = tempfile::tempdir().expect("a directory");
        let session = |key: &[u8]| {
            format!(
                "You can still continue to use the alarm management API using the authorization token {} during \
                 this Netdata session only.",
                String::from_utf8_lossy(key)
            )
        };

        // a link is not followed, to read or to write: the key of the file it names is not taken
        let target = dir.path().join("target");
        std::fs::write(&target, KEY).expect("the file");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("a link");
        let (key, records) = init(&link);
        assert!(is_guid(&key) && key != KEY);
        let name = link.to_string_lossy();
        assert_eq!(records, [format!("Management API key file '{name}' is not a regular file."), session(&key)]);
        assert_eq!(std::fs::read(&target).expect("the file"), KEY);

        // a directory
        let (key, records) = init(dir.path());
        let name = dir.path().to_string_lossy();
        assert_eq!(records, [format!("Management API key file '{name}' is not a regular file."), session(&key)]);

        // a directory that does not exist
        let missing = dir.path().join("none").join("netdata.api.key");
        let (key, records) = init(&missing);
        let name = missing.to_string_lossy();
        assert_eq!(
            records,
            [
                format!(
                    "Cannot create unique management API key file '{name}'. Please adjust config parameter \
                     'netdata management api key file' to a proper path and file."
                ),
                session(&key),
            ]
        );
    }
}
