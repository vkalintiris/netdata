//! The environment every child gets (D134.4): until [`freeze`] the process's own, changed with `sys::setenv()` while
//! the process has one thread; from then on an owned snapshot of it, changed under a lock and sent with each request,
//! so no thread writes `environ` while another reads it (C changes it at runtime, which a Rust process cannot do
//! soundly).

use std::ffi::{CString, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

static FROZEN: OnceLock<Mutex<Vec<CString>>> = OnceLock::new();

fn lock(store: &Mutex<Vec<CString>>) -> MutexGuard<'_, Vec<CString>> {
    store.lock().unwrap_or_else(PoisonError::into_inner)
}

fn pair(key: &[u8], value: &[u8]) -> Option<CString> {
    let mut entry = key.to_vec();
    entry.push(b'=');
    entry.extend_from_slice(value);
    CString::new(entry).ok()
}

/// The process's environment, in `environ`'s order.
fn live() -> Vec<CString> {
    std::env::vars_os().filter_map(|(key, value)| pair(key.as_bytes(), value.as_bytes())).collect()
}

/// Takes the snapshot, once: a later call keeps the first.
pub fn freeze() {
    FROZEN.get_or_init(|| Mutex::new(live()));
}

/// `nd_setenv(key, value, 1)`: in the snapshot once frozen (replaced where it is, else appended), else in the process.
pub fn set(key: &str, value: &str) -> std::io::Result<()> {
    let Some(store) = FROZEN.get() else {
        return netdata_agent_sys::setenv(key, value);
    };
    let entry = pair(key.as_bytes(), value.as_bytes())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "a NUL in an environment variable"))?;
    let prefix = [key.as_bytes(), b"="].concat();
    let mut store = lock(store);
    match store.iter().position(|e| e.as_bytes().starts_with(&prefix)) {
        Some(i) => store[i] = entry,
        None => store.push(entry),
    }
    Ok(())
}

/// What a request carries: the snapshot once frozen, else the process's environment.
pub fn block() -> Vec<CString> {
    match FROZEN.get() {
        Some(store) => lock(store).clone(),
        None => live(),
    }
}

/// A variable as children see it.
pub fn get(key: &str) -> Option<OsString> {
    let prefix = [key.as_bytes(), b"="].concat();
    let value = |e: &CString| {
        e.as_bytes().strip_prefix(prefix.as_slice()).map(|v| OsString::from_vec(v.to_vec()))
    };
    match FROZEN.get() {
        Some(store) => lock(store).iter().find_map(value),
        None => std::env::var_os(key),
    }
}
