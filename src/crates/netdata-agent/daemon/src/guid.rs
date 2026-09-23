//! The machine GUID, ported from `src/daemon/machine-guid.c`: read from `<lib>/registry/netdata.public.unique.id`,
//! or generated and published there with a temporary file, a rename and a lock. A GUID that cannot be saved is
//! still used. The daemon status file's fallback GUID comes with the status file.

use std::fs::{self, File, Permissions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use netdata_agent_inicfg::LogLevel;
use netdata_agent_text::parse::uuid_parse_flexi;
use netdata_agent_text::print::print_uuid_lower;
use nix::fcntl::{AT_FDCWD, Flock, FlockArg, OFlag};
use nix::sys::stat::{Mode, UtimensatFlags, umask, utimensat};
use nix::sys::time::TimeSpec;

/// GUIDs cloned into many machines by images; a host carrying one gets a new GUID.
const BLACKLISTED: [&str; 27] = [
    "8a795b0c-2311-11e6-8563-000c295076a6",
    "4aed1458-1c3e-11e6-a53f-000c290fc8f5",
    "a177c1dc-09d9-11f0-a920-0242ac110002",
    "983624e2-09d9-11f0-b90c-0242ac110002",
    "477f97ae-09d9-11f0-903d-0242ac110002",
    "ded81380-09e1-11f0-ae4c-0242ac110002",
    "9abc69ec-09d9-11f0-a8a4-0242ac110002",
    "68a2d17a-0aa2-11f0-97f3-0242ac110002",
    "6499dbbe-0aa2-11f0-9ccd-0242ac110002",
    "a9708cba-0aa2-11f0-98b6-0242ac110002",
    "26903986-0aab-11f0-818e-0242ac110002",
    "ab576242-0aa2-11f0-89c3-0242ac110002",
    "eab387c6-0b6b-11f0-b715-0242ac110002",
    "eaee7dfe-0b6b-11f0-870f-0242ac110002",
    "c7d4e6b4-0b6b-11f0-878c-0242ac110002",
    "40ac6d48-0b74-11f0-9cf4-0242ac110002",
    "e366fc5a-0b6b-11f0-bd77-0242ac110002",
    "c5955806-0c34-11f0-a302-0242ac110002",
    "1d4d05d0-0c35-11f0-a01d-0242ac110002",
    "edfc72b0-0c35-11f0-8e50-0242ac110002",
    "536a030e-0c3d-11f0-837b-0242ac110002",
    "10846e2e-0c35-11f0-8422-0242ac110002",
    "4339f742-0dc7-11f0-838c-0242ac110002",
    "3f28d7e0-0dc7-11f0-b75f-0242ac110002",
    "41815788-0dc7-11f0-88e0-0242ac110002",
    "104b408a-0dd0-11f0-8ca5-0242ac110002",
    "8e45bc30-0dc7-11f0-8e50-0242ac110002",
];

type Log<'a> = &'a mut dyn FnMut(LogLevel, &str);

fn canonical(uuid: &[u8; 16]) -> String {
    let mut text = Vec::with_capacity(36);
    print_uuid_lower(&mut text, uuid);
    String::from_utf8_lossy(&text).into_owned()
}

/// `machine_guid_check_blacklisted()`.
fn blacklisted(guid: &str, log: Log<'_>) -> bool {
    let found = BLACKLISTED.contains(&guid);
    if found {
        log(
            LogLevel::Info,
            &format!("MACHINE_GUID: blacklisted machine GUID '{guid}' found, generating new one."),
        );
    }
    found
}

/// `machine_guid_read_from_file()`: exactly 36 bytes of a regular file, as `uuid_parse_flexi()` reads them, not zero,
/// not blacklisted; returned in canonical lowercase.
fn read_from_file(filename: &Path, log_errors: bool, log: Log<'_>) -> Option<String> {
    let name = filename.display();
    let fail = |log: Log<'_>, message: String| {
        if log_errors {
            log(LogLevel::Error, &message);
        }
        None
    };
    if !fs::metadata(filename).is_ok_and(|m| m.is_file()) {
        return fail(
            log,
            format!("MACHINE_GUID: cannot open GUID file '{name}' for reading"),
        );
    }
    let Ok(mut file) = File::options()
        .read(true)
        .custom_flags((OFlag::O_NONBLOCK | OFlag::O_CLOEXEC).bits())
        .open(filename)
    else {
        return fail(
            log,
            format!("MACHINE_GUID: cannot open GUID file '{name}' for reading"),
        );
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return fail(
            log,
            format!("MACHINE_GUID: cannot stat the GUID file '{name}'"),
        );
    }
    let mut text = [0u8; 36];
    let read = loop {
        match file.read(&mut text) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            other => break other,
        }
    };
    if !matches!(read, Ok(36)) {
        return fail(log, format!("MACHINE_GUID: cannot read GUID file '{name}'"));
    }
    let Some(uuid) = uuid_parse_flexi(&text) else {
        return fail(
            log,
            format!("MACHINE_GUID: cannot parse GUID from file '{name}'"),
        );
    };
    if uuid == [0; 16] {
        return fail(
            log,
            format!("MACHINE_GUID: GUID read from file '{name}' is zero"),
        );
    }
    let guid = canonical(&uuid);
    if blacklisted(&guid, log) {
        return None;
    }
    log(
        LogLevel::Info,
        &format!("MACHINE_GUID: GUID read from file '{name}'"),
    );
    Some(guid)
}

/// `mkstemp()` for `<prefix>XXXXXX`: a new file, mode 0600, never an existing one.
fn mkstemp(prefix: &str) -> io::Result<(File, String)> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    for _ in 0..100 {
        let random = uuid::Uuid::new_v4();
        let suffix: String = random.as_bytes()[..6]
            .iter()
            .map(|b| char::from(CHARS[usize::from(*b) % CHARS.len()]))
            .collect();
        let path = format!("{prefix}{suffix}");
        match File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((file, path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::from(io::ErrorKind::AlreadyExists))
}

/// `machine_guid_write_to_file()`: a temporary file with the 36 bytes, mode `0444 & ~umask`, the GUID's timestamp
/// as its times, synchronized, renamed into place, and the directory synchronized.
fn write_to_file(dir: &Path, filename: &Path, guid: &str, modified_ut: u64, log: Log<'_>) -> bool {
    let (mut file, tmp) = match mkstemp(&format!("{}.tmp.", filename.display())) {
        Ok(created) => created,
        Err(_) => {
            log(
                LogLevel::Error,
                &format!(
                    "MACHINE_GUID: cannot create the temporary GUID file '{}.tmp.XXXXXX'",
                    filename.display()
                ),
            );
            return false;
        }
    };
    let unlink = |log: Log<'_>, reason: &str| {
        if let Err(e) = fs::remove_file(&tmp) {
            if e.kind() != io::ErrorKind::NotFound {
                log(
                    LogLevel::Error,
                    &format!(
                        "MACHINE_GUID: cannot remove the temporary GUID file '{tmp}' after {reason}"
                    ),
                );
            }
        }
        false
    };
    if file.write_all(guid.as_bytes()).is_err() {
        log(
            LogLevel::Error,
            &format!("MACHINE_GUID: cannot write GUID to the temporary GUID file '{tmp}'"),
        );
        return unlink(log, "write failure");
    }
    // Read the umask and put it back.
    let current = umask(Mode::empty());
    umask(current);
    if file
        .set_permissions(Permissions::from_mode(0o444 & !current.bits()))
        .is_err()
    {
        log(
            LogLevel::Error,
            &format!("MACHINE_GUID: cannot set permissions on temporary GUID file '{tmp}'"),
        );
        return unlink(log, "permission failure");
    }
    let time = TimeSpec::new(
        (modified_ut / 1_000_000) as i64,
        ((modified_ut % 1_000_000) * 1000) as i64,
    );
    if utimensat(
        AT_FDCWD,
        tmp.as_str(),
        &time,
        &time,
        UtimensatFlags::FollowSymlink,
    )
    .is_err()
    {
        log(
            LogLevel::Error,
            &format!(
                "MACHINE_GUID: cannot update the timestamps of the temporary GUID file '{tmp}'"
            ),
        );
    }
    if file.sync_all().is_err() {
        log(
            LogLevel::Error,
            &format!("MACHINE_GUID: cannot synchronize temporary GUID file '{tmp}'"),
        );
        return unlink(log, "synchronization failure");
    }
    drop(file);
    if fs::rename(&tmp, filename).is_err() {
        log(
            LogLevel::Error,
            &format!(
                "MACHINE_GUID: cannot rename temporary GUID file '{tmp}' to '{}'",
                filename.display()
            ),
        );
        return unlink(log, "publication failure");
    }
    match File::open(dir) {
        Ok(d) if d.sync_all().is_ok() => {}
        Ok(_) => {
            log(
                LogLevel::Error,
                &format!(
                    "MACHINE_GUID: cannot synchronize GUID directory '{}'",
                    dir.display()
                ),
            );
            return unlink(log, "publication failure");
        }
        Err(_) => {
            log(
                LogLevel::Error,
                &format!(
                    "MACHINE_GUID: cannot open GUID directory '{}' for synchronization",
                    dir.display()
                ),
            );
            return unlink(log, "publication failure");
        }
    }
    log(
        LogLevel::Info,
        &format!("MACHINE_GUID: GUID saved to file '{}'", filename.display()),
    );
    true
}

/// `machine_guid_get_or_create()`: never fails; a GUID that cannot be published is used in memory.
pub fn machine_guid_get(varlib: &str, log: Log<'_>) -> String {
    let dir = Path::new(varlib).join("registry");
    let filename = dir.join("netdata.public.unique.id");
    if let Some(guid) = read_from_file(&filename, true, log) {
        return guid;
    }
    log(
        LogLevel::Error,
        &format!(
            "MACHINE_GUID: failed to read GUID from file '{}'",
            filename.display()
        ),
    );
    log(LogLevel::Info, "MACHINE_GUID: generating a new GUID");
    let guid = canonical(uuid::Uuid::new_v4().as_bytes());
    let modified_ut = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64);
    if let Err(e) = fs::DirBuilder::new().mode(0o775).create(&dir) {
        if e.kind() != io::ErrorKind::AlreadyExists {
            log(
                LogLevel::Error,
                &format!("MACHINE_GUID: cannot create directory '{}'", dir.display()),
            );
            return guid;
        }
        if !fs::metadata(&dir).is_ok_and(|m| m.is_dir()) {
            log(
                LogLevel::Error,
                &format!(
                    "MACHINE_GUID: path '{}' exists but is not a directory",
                    dir.display()
                ),
            );
            return guid;
        }
    }
    let lock_filename = format!("{}.lock", filename.display());
    // file_lock_get_wait(): an exclusive flock on the lock file, released when it closes.
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_filename)
        .ok()
        .and_then(|f| Flock::lock(f, FlockArg::LockExclusive).ok());
    let Some(_lock) = lock else {
        log(
            LogLevel::Error,
            &format!("MACHINE_GUID: cannot acquire publication lock '{lock_filename}'"),
        );
        return guid;
    };
    // Another process may have published one meanwhile.
    if let Some(published) = read_from_file(&filename, false, log) {
        return published;
    }
    if !write_to_file(&dir, &filename, &guid, modified_ut, log) {
        log(
            LogLevel::Error,
            &format!(
                "MACHINE_GUID: cannot save GUID to file '{}'",
                filename.display()
            ),
        );
    }
    guid
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("netdata-guid-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("registry")).unwrap();
        dir
    }

    #[test]
    fn reads_like_c() {
        let mut log = |_: LogLevel, _: &str| {};
        let dir = scratch("read");
        let file = dir.join("registry/netdata.public.unique.id");
        let cases: [(&[u8], Option<&str>); 5] = [
            // Only the first 36 bytes count, in any case.
            (
                b"5A1E0000-0000-4000-8000-0000000000AAtrailing",
                Some("5a1e0000-0000-4000-8000-0000000000aa"),
            ),
            (b"00000000-0000-0000-0000-000000000000", None),
            (b"8a795b0c-2311-11e6-8563-000c295076a6", None),
            (b"5a1e0000-0000-4000-8000-0000000000a", None),
            (b"5a1e0000000040008000000000aa\n", None),
        ];
        for (content, expected) in cases {
            fs::write(&file, content).unwrap();
            assert_eq!(
                read_from_file(&file, true, &mut log).as_deref(),
                expected,
                "{}",
                String::from_utf8_lossy(content)
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_guid_that_cannot_be_saved_is_still_used() {
        let mut log = |_: LogLevel, _: &str| {};
        let dir = scratch("save");
        let guid = machine_guid_get(dir.to_str().unwrap(), &mut log);
        let saved = fs::read_to_string(dir.join("registry/netdata.public.unique.id")).unwrap();
        assert_eq!(saved, guid);
        let mode = fs::metadata(dir.join("registry/netdata.public.unique.id"))
            .unwrap()
            .mode()
            & 0o777;
        assert_eq!(mode & 0o222, 0);
        assert_eq!(machine_guid_get(dir.to_str().unwrap(), &mut log), guid);
        // A registry path that is a file: nothing can be written, a GUID comes back anyway.
        let blocked = dir.join("blocked");
        fs::write(&blocked, b"").unwrap();
        let fresh = machine_guid_get(blocked.to_str().unwrap(), &mut log);
        assert_eq!(fresh.len(), 36);
        let _ = fs::remove_dir_all(&dir);
    }
}
