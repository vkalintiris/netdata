//! The per-host function registry, ported from `src/nrpc/nrpc-registry.c`: what plugins, streaming children and the
//! daemon register with `FUNCTION`, keyed by the sanitized name.
//!
//! Calls, availability (serving handles, host epochs), the `FUNCTION_DEL` journal for a parent and the catalogs
//! come with the subsystems that use them (functions API, stream sender, ACLK); `knowledge/plugins-d.md` §5 in the
//! status repository.

#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use netdata_agent_text::sanitize::nrpc_sanitize_name;

pub mod access;

/// `NRPC_PRIORITY_DEFAULT`.
pub const PRIORITY_DEFAULT: i32 = 100;
/// `NRPC_VERSION_DEFAULT`.
pub const VERSION_DEFAULT: u32 = 0;

/// `NRPC_METHOD_FLAG_RESTRICTED`: hidden from users (`__` prefix or a `hidden` tag).
pub const FLAG_RESTRICTED: u32 = 1 << 0;
/// `NRPC_METHOD_FLAG_DYNCFG`: a dynamic-configuration function (`config`, `config <id>`).
pub const FLAG_DYNCFG: u32 = 1 << 1;

/// `NRPC_SOURCE`: who registered a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Plugin,
    Stream,
    Daemon,
}

/// `struct nrpc_method_desc`: a registration.
#[derive(Debug, Clone, Copy)]
pub struct MethodDesc<'a> {
    pub name: &'a [u8],
    pub help: &'a [u8],
    /// Empty means `top`.
    pub tags: &'a [u8],
    pub timeout_s: i32,
    /// 0 means `PRIORITY_DEFAULT`.
    pub priority: i32,
    pub version: u32,
    /// `HTTP_ACCESS` bits (`access`).
    pub access: u32,
    pub sync: bool,
    pub source: Source,
}

/// `struct nrpc_method`: one registration, immutable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method {
    pub help: Vec<u8>,
    pub tags: Vec<u8>,
    pub timeout_s: i32,
    pub priority: i32,
    pub version: u32,
    pub access: u32,
    pub sync: bool,
    pub source: Source,
    /// `FLAG_RESTRICTED` or `FLAG_DYNCFG`.
    pub flags: u32,
}

/// What `Registry::unregister()` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unregistered {
    Removed,
    NotFound,
    /// Refused, with the warning C logs.
    Refused(String),
}

/// `nrpc_method_name_is_dyncfg()`: `config` alone or followed by whitespace.
pub fn name_is_dyncfg(name: &[u8]) -> bool {
    match name.strip_prefix(b"config") {
        Some(rest) => rest
            .first()
            .is_none_or(|&c| matches!(c, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')),
        None => false,
    }
}

/// `nrpc_method_flags_for()`.
fn flags_for(name: &[u8], tags: &[u8]) -> u32 {
    if name_is_dyncfg(name) {
        FLAG_DYNCFG
    } else if name.starts_with(b"__") || tags.windows(6).any(|w| w == b"hidden") {
        FLAG_RESTRICTED
    } else {
        0
    }
}

/// The key of a name: `nrpc_sanitize_name()` over the whole name.
fn key(name: &[u8]) -> Vec<u8> {
    nrpc_sanitize_name(name, name.len() + 1)
}

/// `registry->dict`: the functions of one host in registration order; a re-registration keeps its place.
#[derive(Debug, Default)]
pub struct Registry {
    methods: Mutex<Vec<(Vec<u8>, Arc<Method>)>>,
}

impl Registry {
    fn lock(&self) -> MutexGuard<'_, Vec<(Vec<u8>, Arc<Method>)>> {
        self.methods.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `nrpc_method_register()`. `Err` carries the warning C logs when it refuses (`host` names the owner there).
    pub fn register(&self, host: &str, desc: &MethodDesc) -> Result<(), String> {
        let tags = if desc.tags.is_empty() {
            &b"top"[..]
        } else {
            desc.tags
        };
        let key = key(desc.name);
        if key.is_empty() {
            return Err(format!(
                "NRPC: refusing to register method '{}' on host '{host}': the name sanitizes to an empty string",
                String::from_utf8_lossy(desc.name)
            ));
        }
        // Local plugins configure through the DYNCFG protocol; a child's single `config` proxy is legitimate.
        if desc.source == Source::Plugin && name_is_dyncfg(&key) {
            return Err(format!(
                "NRPC: 'host:{host}' attempted to register reserved dynamic-configuration method '{}' from a plugin. Ignoring it.",
                String::from_utf8_lossy(&key)
            ));
        }
        let method = Arc::new(Method {
            help: desc.help.to_vec(),
            tags: tags.to_vec(),
            timeout_s: desc.timeout_s,
            priority: if desc.priority == 0 {
                PRIORITY_DEFAULT
            } else {
                desc.priority
            },
            version: desc.version,
            access: desc.access,
            sync: desc.sync,
            source: desc.source,
            flags: flags_for(&key, tags),
        });
        let mut methods = self.lock();
        match methods.iter_mut().find(|(k, _)| *k == key) {
            Some((_, slot)) => *slot = method,
            None => methods.push((key, method)),
        }
        Ok(())
    }

    /// `nrpc_method_unregister()`. Only the daemon removes dynamic-configuration functions.
    ///
    /// C also refuses a plugin removing a function another plugin thread registered; that check arrives with the
    /// plugins.d port, which brings serving handles.
    pub fn unregister(&self, name: &[u8], source: Source) -> Unregistered {
        if name.is_empty() {
            return Unregistered::NotFound;
        }
        let key = key(name);
        let mut methods = self.lock();
        let Some(i) = methods.iter().position(|(k, _)| *k == key) else {
            return Unregistered::NotFound;
        };
        if source != Source::Daemon && methods[i].1.flags & FLAG_DYNCFG != 0 {
            return Unregistered::Refused(format!(
                "NRPC: refusing to unregister dyncfg method '{}' via FUNCTION_DEL",
                String::from_utf8_lossy(name)
            ));
        }
        methods.remove(i);
        Unregistered::Removed
    }

    /// The function registered under a name.
    pub fn get(&self, name: &[u8]) -> Option<Arc<Method>> {
        let key = key(name);
        self.lock()
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, m)| Arc::clone(m))
    }

    /// Every function, in registration order.
    pub fn all(&self) -> Vec<(Vec<u8>, Arc<Method>)> {
        self.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(name: &'static [u8], source: Source) -> MethodDesc<'static> {
        MethodDesc {
            name,
            help: b"help",
            tags: b"",
            timeout_s: 10,
            priority: 0,
            version: 0,
            access: 0,
            sync: false,
            source,
        }
    }

    #[test]
    fn registration_normalizes_and_replaces_in_place() {
        let r = Registry::default();
        r.register("h", &desc(b"processes", Source::Stream))
            .unwrap();
        r.register(
            "h",
            &MethodDesc {
                tags: b"top,hidden",
                ..desc(b"b", Source::Stream)
            },
        )
        .unwrap();
        r.register(
            "h",
            &MethodDesc {
                priority: 5,
                ..desc(b"processes", Source::Stream)
            },
        )
        .unwrap();
        let all = r.all();
        let names: Vec<_> = all.iter().map(|(k, _)| k.as_slice()).collect();
        assert_eq!(names, [&b"processes"[..], b"b"]);
        let p = r.get(b"processes").unwrap();
        assert_eq!(
            (p.tags.as_slice(), p.priority, p.flags),
            (&b"top"[..], 5, 0)
        );
        assert_eq!(r.get(b"b").unwrap().flags, FLAG_RESTRICTED);
        assert_eq!(
            r.get(b"b").unwrap().priority,
            PRIORITY_DEFAULT,
            "priority 0 is the default"
        );
    }

    #[test]
    fn dyncfg_names_are_reserved() {
        assert!(name_is_dyncfg(b"config"));
        assert!(name_is_dyncfg(b"config go.d:nginx"));
        assert!(!name_is_dyncfg(b"configure"));
        let r = Registry::default();
        assert!(r.register("h", &desc(b"config", Source::Plugin)).is_err());
        r.register("h", &desc(b"config", Source::Stream)).unwrap();
        assert_eq!(r.get(b"config").unwrap().flags, FLAG_DYNCFG);
        assert!(matches!(
            r.unregister(b"config", Source::Stream),
            Unregistered::Refused(_)
        ));
        assert_eq!(
            r.unregister(b"config", Source::Daemon),
            Unregistered::Removed
        );
        assert_eq!(
            r.unregister(b"config", Source::Daemon),
            Unregistered::NotFound
        );
        // A name of underscores sanitizes to nothing.
        assert!(r.register("h", &desc(b"__", Source::Stream)).is_err());
    }
}
