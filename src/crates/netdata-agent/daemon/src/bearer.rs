//! Bearer tokens, ported from `src/web/api/http_auth.c`: the files under `<varlib>/bearer_tokens` the agent loads at
//! start and on demand, and the `X-Netdata-Auth` / `Authorization: Bearer` values that present one. Only the Cloud
//! creates tokens (M11). Spec `knowledge/spec-m6-token-store.md`, decisions D96 in the status repository.

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use netdata_agent_ingest::jsonc::{self, Presence::Required};
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_nrpc::access;
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_text::c::filename_from_path_entry;
use netdata_agent_text::parse::uuid_parse_flexi;
use netdata_agent_text::print::print_uuid_lower;
use serde_json::{Map, Value};
use twox_hash::XxHash3_64;

use crate::access_log::Auth;
use crate::status_file::io::read_text;

/// `CLOUD_CLIENT_NAME_LENGTH` less its NUL: what a token keeps of its client's name.
const CLIENT_NAME_MAX: usize = 63;

/// `struct bearer_token`.
#[derive(Debug, Clone, PartialEq)]
struct Token {
    account: [u8; 16],
    client_name: Vec<u8>,
    access: u32,
    role: u8,
    created_s: i64,
    expires_s: i64,
}

/// `netdata_authorized_bearers`, where its files live and the host they belong to.
struct Store {
    dir: String,
    host: [u8; 16],
    /// C's dictionary: insertion order, one entry per token.
    tokens: Mutex<Vec<([u8; 16], Token)>>,
    /// `bearer_token_cleanup()`'s `cleanup_attempts`.
    cleanups: AtomicU32,
}

static STORE: OnceLock<Store> = OnceLock::new();

/// `bearer_tokens_init()` after its key: the tokens saved under `varlib` for the host `host`.
pub fn init(varlib: &str, host: [u8; 16]) {
    STORE.get_or_init(|| Store::new(varlib, host)).load_from_disk();
}

/// `web_client_bearer_token_auth()` of one header's token: whether it authenticated the request.
pub fn authenticate(auth: &Auth, value: &[u8]) -> bool {
    STORE.get().is_some_and(|store| store.authenticate(auth, value))
}

impl Store {
    fn new(varlib: &str, host: [u8; 16]) -> Store {
        Store {
            dir: filename_from_path_entry(varlib, "bearer_tokens", None),
            host,
            tokens: Mutex::new(Vec::new()),
            cleanups: AtomicU32::new(0),
        }
    }

    fn tokens(&self) -> MutexGuard<'_, Vec<([u8; 16], Token)>> {
        self.tokens.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `bearer_token_filename()`: the token's dashed lowercase UUID in the directory.
    fn filename(&self, token: &[u8; 16]) -> String {
        let mut name = Vec::with_capacity(36);
        print_uuid_lower(&mut name, token);
        filename_from_path_entry(&self.dir, &String::from_utf8_lossy(&name), None)
    }

    /// `bearer_tokens_load_from_disk()`: the directory made when missing, then every regular file (or link to one)
    /// named by a UUID, read under the UUID's own name.
    fn load_from_disk(&self) {
        // filename_is_dir(path, true): a failure shows as the directory not opening
        if !std::fs::metadata(&self.dir).is_ok_and(|m| m.is_dir()) {
            let _ = std::fs::DirBuilder::new().mode(0o750).create(&self.dir);
        }
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(_) => {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "Cannot open directory '{}' to read saved bearer tokens",
                    self.dir
                );
                return;
            }
        };
        for entry in entries.flatten() {
            use std::os::unix::ffi::OsStrExt;
            let Some(token) = uuid_parse_flexi(entry.file_name().as_bytes()).filter(|u| *u != [0; 16]) else {
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path: PathBuf = entry.path();
            if kind.is_file() || (kind.is_symlink() && std::fs::metadata(&path).is_ok_and(|m| m.is_file())) {
                self.load_token(&token);
            }
        }
    }

    /// `bearer_token_load_token()`: the token's file, parsed and checked; an invalid one is deleted, a text that is no
    /// JSON kept. Whether the token is now known.
    fn load_token(&self, token: &[u8; 16]) -> bool {
        let filename = self.filename(token);
        let Some(text) = read_text(filename.as_ref(), 1024 * 1024) else {
            return false;
        };
        let empty = Map::new();
        // json-c: a root that is no object has none of the members
        let obj = match jsonc::tokener_parse(&text) {
            None | Some(Value::Null) => {
                nd_log!(Source::Daemon, Priority::Err, "Cannot parse bearer token file '{filename}'");
                return false;
            }
            Some(Value::Object(obj)) => obj,
            Some(_) => empty,
        };
        match self.parse_json(token, &obj) {
            Ok(parsed) => {
                // DICT_OPTION_DONT_OVERWRITE_VALUE
                let mut tokens = self.tokens();
                if !tokens.iter().any(|(id, _)| id == token) {
                    tokens.push((*token, parsed));
                }
                drop(tokens);
                self.cleanup(true);
                true
            }
            Err(error) => {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "Failed to parse bearer token file '{filename}': {error}"
                );
                let _ = std::fs::remove_file(&filename);
                false
            }
        }
    }

    /// `bearer_token_parse_json()`: C's members in its order, each failure's text appended to the last one's, then
    /// the checks, each replacing the text.
    fn parse_json(&self, token: &[u8; 16], obj: &Map<String, Value>) -> Result<Token, String> {
        let mut e = String::new();
        let Some(members) = Self::members(obj, &mut e) else {
            return Err(e);
        };
        let (host, in_file, signature, mut parsed) = members;
        if in_file != *token {
            return Err("token in JSON file does not match the filename".into());
        }
        if host != self.host {
            return Err("Host UUID in JSON file does not match our host UUID".into());
        }
        if parsed.created_s == 0 || parsed.expires_s == 0 || parsed.created_s >= parsed.expires_s {
            return Err("bearer token has invalid dates".into());
        }
        parsed.client_name.truncate(CLIENT_NAME_MAX);
        if signature != self.signature(token, &parsed) {
            return Err("bearer token has invalid signature".into());
        }
        Ok(parsed)
    }

    /// The members of `bearer_token_parse_json()`: the host, the token in the file, the signature, and the token's
    /// fields.
    fn members(obj: &Map<String, Value>, e: &mut String) -> Option<([u8; 16], [u8; 16], u64, Token)> {
        jsonc::int64(obj, ".", "version", Required, e)?;
        let host = jsonc::uuid(obj, ".", "host_uuid", Required, e)?;
        let in_file = jsonc::uuid(obj, ".", "token", Required, e)?;
        let account = jsonc::uuid(obj, ".", "cloud_account_id", Required, e)?;
        let client_name = jsonc::txt(obj, ".", "client_name", Required, e)?.unwrap_or_default();
        let access = jsonc::bitmap(obj, ".", "access", access::id_one, Required, e)?;
        let role = match jsonc::enum_text(obj, ".", "user_role", Required, e)? {
            Some(name) => access::role::to_id(name),
            None => access::role::NONE,
        };
        let created_s = jsonc::uint64(obj, ".", "created_s", Required, e)? as i64;
        let expires_s = jsonc::uint64(obj, ".", "expires_s", Required, e)? as i64;
        let signature = jsonc::uint64(obj, ".", "signature", Required, e)?;
        Some((
            host,
            in_file,
            signature,
            Token {
                account,
                client_name: client_name.into_bytes(),
                access,
                role,
                created_s,
                expires_s,
            },
        ))
    }

    /// `bearer_token_signature()` as the production build computes it: XXH3-64 of the 136-byte `struct` of the host,
    /// the token, the account, the name (63 bytes at most, zero-filled), the role and the two times; the access bits
    /// hash as zeros (D96.4).
    fn signature(&self, token: &[u8; 16], t: &Token) -> u64 {
        let mut buf = [0u8; 136];
        buf[..16].copy_from_slice(&self.host);
        buf[16..32].copy_from_slice(token);
        buf[32..48].copy_from_slice(&t.account);
        let name = &t.client_name[..t.client_name.len().min(CLIENT_NAME_MAX)];
        buf[48..48 + name.len()].copy_from_slice(name);
        buf[114] = t.role;
        buf[120..128].copy_from_slice(&t.created_s.to_le_bytes());
        buf[128..136].copy_from_slice(&t.expires_s.to_le_bytes());
        XxHash3_64::oneshot(&buf)
    }

    /// `bearer_token_cleanup()`: every thousandth call, or when forced, the expired tokens leave memory and disk.
    fn cleanup(&self, force: bool) {
        let attempts = self.cleanups.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        if !attempts.is_multiple_of(1000) && !force {
            return;
        }
        let now_s = now_realtime_s();
        self.tokens().retain(|(token, t)| {
            if t.expires_s >= now_s {
                return true;
            }
            let filename = self.filename(token);
            if std::fs::remove_file(&filename).is_err() {
                nd_log!(Source::Daemon, Priority::Err, "Failed to unlink() file '{filename}'");
            }
            false
        });
    }

    /// `web_client_bearer_token_auth()`: an unknown token is looked for on disk; a known one that has not expired
    /// authenticates the request. `null` and `undefined` are what browsers send for none.
    fn authenticate(&self, auth: &Auth, value: &[u8]) -> bool {
        if value.is_empty() || value == b"null" || value == b"undefined" {
            return false;
        }
        // mcp_api_key_verify(): no key on an unclaimed agent
        let Some(token) = uuid_parse_flexi(value) else {
            nd_log!(
                Source::Daemon,
                Priority::Notice,
                "Invalid bearer token '{}' received.",
                String::from_utf8_lossy(value)
            );
            return false;
        };
        let find = || self.tokens().iter().find(|(id, _)| *id == token).map(|(_, t)| t.clone());
        let found = find().or_else(|| if self.load_token(&token) { find() } else { None });
        let Some(t) = found.filter(|t| t.expires_s > now_realtime_s()) else {
            return false;
        };
        auth.authorize_bearer(t.access, t.role, &String::from_utf8_lossy(&t.client_name), t.account);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: [u8; 16] = [0x5a; 16];
    const TOKEN: &str = "b6b6b6b6-4444-4444-8444-0000000000b1";
    const ACCOUNT: &str = "b6b6b6b6-4444-4444-8444-00000000acc1";

    fn token() -> [u8; 16] {
        uuid_parse_flexi(TOKEN.as_bytes()).unwrap()
    }

    fn store(dir: &tempfile::TempDir) -> Store {
        let store = Store::new(dir.path().to_str().unwrap(), HOST);
        std::fs::create_dir(&store.dir).unwrap();
        store
    }

    fn admin(created_s: i64, expires_s: i64) -> Token {
        Token {
            account: uuid_parse_flexi(ACCOUNT.as_bytes()).unwrap(),
            client_name: b"probe".to_vec(),
            access: access::ALL,
            role: access::role::ADMIN,
            created_s,
            expires_s,
        }
    }

    /// The file `bearer_token_save_to_file()` writes for `t`, with `signature`.
    fn file(store: &Store, host: &str, in_file: &str, t: &Token, signature: u64) {
        let names: Vec<String> = access::names(t.access).map(|n| format!("\"{n}\"")).collect();
        let text = format!(
            r#"{{"version":1,"host_uuid":"{host}","token":"{in_file}","cloud_account_id":"{ACCOUNT}","client_name":"{}","access":[{}],"user_role":"{}","created_s":{},"expires_s":{},"signature":{signature}}}"#,
            String::from_utf8_lossy(&t.client_name),
            names.join(","),
            access::role::name(t.role),
            t.created_s,
            t.expires_s
        );
        std::fs::write(store.filename(&token()), text).unwrap();
    }

    fn host_text() -> String {
        let mut s = Vec::new();
        print_uuid_lower(&mut s, &HOST);
        String::from_utf8(s).unwrap()
    }

    /// A valid file is loaded at start and authenticates its token; an expired one is deleted by the forced cleanup
    /// after the load.
    #[test]
    fn valid_tokens_authenticate_expired_ones_go() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let t = admin(1_700_000_000, 4_102_444_800);
        file(&store, &host_text(), TOKEN, &t, store.signature(&token(), &t));
        store.load_from_disk();
        let auth = Auth::default();
        store.authenticate(&auth, format!("{TOKEN}  ").as_bytes());
        assert!(auth.is_bearer());
        assert_eq!((auth.role(), auth.access()), ("admin", access::ALL));
        assert_eq!(auth.identity(), ("probe".to_string(), t.account));
        let old = admin(1_000, 2_000);
        let other = Store::new(dir.path().to_str().unwrap(), HOST);
        file(&other, &host_text(), TOKEN, &old, other.signature(&token(), &old));
        assert!(other.load_token(&token()));
        assert!(other.tokens().is_empty() && !std::path::Path::new(&other.filename(&token())).exists());
    }

    /// C's checks in C's order, with C's texts; an invalid file is deleted, a text that is no JSON kept.
    #[test]
    fn invalid_files_as_c() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let t = admin(1_700_000_000, 4_102_444_800);
        let sig = store.signature(&token(), &t);
        let parse = |text: &str| {
            let Some(Value::Object(obj)) = jsonc::tokener_parse(text.as_bytes()) else {
                return Err("not an object".to_string());
            };
            store.parse_json(&token(), &obj)
        };
        assert_eq!(parse("{}"), Err("missing '..version'".into()));
        let host = host_text();
        let with = |host: &str, in_file: &str, access: &str, created: i64, signature: u64| {
            format!(
                r#"{{"version":1,"host_uuid":"{host}","token":"{in_file}","cloud_account_id":"{ACCOUNT}","client_name":"probe","access":[{access}],"user_role":"admin","created_s":{created},"expires_s":4102444800,"signature":{signature}}}"#
            )
        };
        let all: Vec<String> = access::names(access::ALL).map(|n| format!("\"{n}\"")).collect();
        let all = all.join(",");
        assert_eq!(parse(&with(&host, TOKEN, &all, 1_700_000_000, sig)), Ok(t.clone()));
        // the access bits are not signed (D96.4): a file with fewer verifies with the same signature
        assert!(parse(&with(&host, TOKEN, r#""signed-in""#, 1_700_000_000, sig)).is_ok());
        assert_eq!(
            parse(&with(&host, ACCOUNT, &all, 1_700_000_000, sig)),
            Err("token in JSON file does not match the filename".into())
        );
        assert_eq!(
            parse(&with(ACCOUNT, TOKEN, &all, 1_700_000_000, sig)),
            Err("Host UUID in JSON file does not match our host UUID".into())
        );
        assert_eq!(
            parse(&with(&host, TOKEN, &all, 4_102_444_800, sig)),
            Err("bearer token has invalid dates".into())
        );
        assert_eq!(
            parse(&with(&host, TOKEN, &all, 1_700_000_000, sig ^ 1)),
            Err("bearer token has invalid signature".into())
        );
        // an unknown access name is only reported; the error it leaves runs into a later one
        let unknown = with(&host, TOKEN, r#""bogus""#, 1_700_000_000, sig).replace(r#""signature":"#, r#""signatur":"#);
        assert_eq!(
            parse(&unknown),
            Err("unknown option 'bogus' in '..access' at index 0missing '..signature'".into())
        );
        std::fs::write(store.filename(&token()), "not json").unwrap();
        assert!(!store.load_token(&token()));
        assert!(std::path::Path::new(&store.filename(&token())).exists(), "kept");
        std::fs::write(store.filename(&token()), "123").unwrap();
        assert!(!store.load_token(&token()));
        assert!(!std::path::Path::new(&store.filename(&token())).exists(), "deleted: no version");
    }
}
