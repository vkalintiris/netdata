//! nRPC, ported from `src/nrpc/`: the per-host function registry (`nrpc-registry.c`) of what plugins, streaming
//! children and the daemon register with `FUNCTION`, keyed by the sanitized name; a method's availability (its
//! serving thread, the host's epoch); the `FUNCTION_DEL` journal for a parent; the catalog traversals
//! (`nrpc-catalog.c`); and the calls (`call`, `nrpc-calls.c`).

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use netdata_agent_log::{Priority, Source as LogSource, nd_log};
use netdata_agent_text::sanitize::nrpc_sanitize_name;

pub mod access;
pub mod call;
pub mod catalog;
pub mod lifetime;
pub mod reply;
pub mod serving;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

use call::Request;
use reply::{Payload, Reply};
use serving::Serving;

/// `NRPC_PRIORITY_DEFAULT`.
pub const PRIORITY_DEFAULT: i32 = 100;
/// `NRPC_VERSION_DEFAULT`.
pub const VERSION_DEFAULT: u32 = 0;
/// `PLUGINS_FUNCTIONS_TIMEOUT_DEFAULT`: a call's timeout when a child's request gives none.
pub const TIMEOUT_DEFAULT: i32 = 10;
/// `PLUGINSD_LINE_MAX`: the longest name C registers (it `fatal()`s beyond, D135.4).
pub const NAME_MAX: usize = 15_487;
/// `MAX_FUNCTION_LENGTH`: a command is looked up within this many bytes.
pub const FUNCTION_MAX: usize = NAME_MAX - 512;

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

/// `nrpc_handler_cb_t` with its data: what executes a method.
#[derive(Clone)]
pub enum Handler {
    /// A transport to the method's plugin or child (`pluginsd_nrpc_handler()`).
    Transport(Arc<dyn Transport>),
    /// `nrpc_method_register_builtin()`: a daemon-implemented synchronous method.
    Builtin(Builtin),
}

impl std::fmt::Debug for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Handler::Transport(_) => "Transport",
            Handler::Builtin(_) => "Builtin",
        })
    }
}

impl Handler {
    /// The same handler and data (`handler` and `handler_data` compared).
    pub fn same(&self, other: &Handler) -> bool {
        match (self, other) {
            (Handler::Transport(a), Handler::Transport(b)) => Arc::ptr_eq(a, b),
            (Handler::Builtin(a), Handler::Builtin(b)) => std::ptr::fn_addr_eq(*a, *b),
            _ => false,
        }
    }
}

/// A transport's dispatch (`pluginsd_execute_function_cb()`): it MUST hand the request's `done` its result exactly
/// once, on success and failure alike, possibly after it returns; the code it returns only says whether the call was
/// accepted.
pub trait Transport: Send + Sync {
    fn dispatch(&self, req: Request) -> u16;
}

/// `nrpc_builtin_handler_cb_t`: writes the answer, returns its code.
pub type Builtin = fn(reply: &mut Reply, function: &[u8], payload: Option<&Payload>, source: &[u8]) -> u16;

/// `struct nrpc_method_desc`: a registration.
#[derive(Debug, Clone)]
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
    pub handler: Handler,
}

/// `struct nrpc_method`: one registration, immutable; a re-registration replaces it.
#[derive(Debug)]
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
    pub handler: Handler,
    /// The thread that registered it.
    pub serving: Arc<Serving>,
    /// The host's epoch when it was registered.
    pub epoch: u32,
}

impl Method {
    /// `nrpc_method_equal()`.
    fn same_as(&self, other: &Method) -> bool {
        self.sync == other.sync
            && self.flags == other.flags
            && self.source == other.source
            && self.access == other.access
            && self.help == other.help
            && self.tags == other.tags
            && self.timeout_s == other.timeout_s
            && self.priority == other.priority
            && self.version == other.version
            && self.handler.same(&other.handler)
            && self.epoch == other.epoch
            && Arc::ptr_eq(&self.serving, &other.serving)
    }

    /// `nrpc_method_is_available_at()`: its thread serves it and the host's epoch is the one it was registered in (an
    /// archived host's registry is not disarmed but emptied, and refuses registrations: `Registry::destroy()`).
    fn available_at(&self, epoch: u32) -> bool {
        self.serving.running() && self.epoch == epoch
    }
}

/// What `Registry::unregister()` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unregistered {
    /// Removed; `manifest`: it was in the Cloud's manifest (neither DynCfg nor restricted).
    Removed { manifest: bool },
    NotFound,
    /// Refused, with C's warning for the caller to write.
    Refused(String),
}

/// Methods by key, in registration order.
pub type Methods = Vec<(Vec<u8>, Arc<Method>)>;

/// `NRPC_CATALOG_FILTER`: which available methods a traversal visits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// Users and the Cloud: no DynCfg, nothing restricted.
    User,
    /// A parent's re-list: no DynCfg (counted instead).
    StreamGlobal,
}

/// `nrpc_method_name_is_dyncfg()`: `config` alone or followed by whitespace.
pub fn name_is_dyncfg(name: &[u8]) -> bool {
    match name.strip_prefix(b"config") {
        Some(rest) => rest.first().is_none_or(|&c| is_space(c)),
        None => false,
    }
}

/// C's `isspace()`.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
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

/// The key of a name: `nrpc_sanitize_name()` over the whole name (a hex expansion that does not fit is dropped).
fn key(name: &[u8]) -> Vec<u8> {
    nrpc_sanitize_name(name, name.len() + 1)
}

/// `nrpc_sanitize_name_dupz()`: a command sanitized with room for hex expansions, within `FUNCTION_MAX`. As C's, only
/// the output is bounded (by the first `FUNCTION_MAX` bytes' length): the input is read until the output is full, so
/// a run of spaces past `FUNCTION_MAX` collapses and what follows it still counts.
pub fn sanitize_command(cmd: &[u8]) -> Vec<u8> {
    let len = cmd.len().min(FUNCTION_MAX);
    nrpc_sanitize_name(cmd, (len * 2).min(FUNCTION_MAX) + 1)
}

/// `struct nrpc_registry`: the functions of one host in registration order (a re-registration keeps its place), the
/// host's epoch, and the names removed since its parent last heard.
#[derive(Debug, Default)]
pub struct Registry {
    methods: Mutex<Methods>,
    /// `OBJECT_STATE`'s id: bumped each time the host is activated again.
    epoch: AtomicU32,
    /// `pending_dels`: a set, in insertion order.
    pending_dels: Mutex<Vec<Vec<u8>>>,
    /// The label C's records give the registry (`nrpc_owner_str()`: its host's handle in hex), set by its host.
    owner: OnceLock<String>,
    /// `nrpc_registry_destroy()` ran and no `nrpc_registry_init()` since: the host has no registry (an archived host),
    /// as C's `nrpc_registry_acquire()` failing. Changed only under the methods' lock, which a registration holds, so
    /// a destroyed registry stays empty: lookups answer C's 404, the views and the deletion queue are empty.
    destroyed: AtomicBool,
}

/// `nrpc_method_register()` on a host without a registry (unknown or archived): C's record, and nothing registered.
fn not_registering(name: &[u8]) {
    nd_log!(
        LogSource::Daemon,
        Priority::Debug,
        "NRPC: not registering method '{}': the given host has no function registry",
        String::from_utf8_lossy(name)
    );
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Registry {
    fn methods(&self) -> MutexGuard<'_, Methods> {
        lock(&self.methods)
    }

    /// `object_state_activate()` and its kin: a new epoch, which retires every method registered before it.
    pub fn activate(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }

    /// The host's epoch.
    pub fn epoch(&self) -> u32 {
        self.epoch.load(Ordering::Acquire)
    }

    /// `nrpc_registry_destroy()` of a host archived: every function gone, the queue too, and no registry until
    /// `init()`.
    pub fn destroy(&self) {
        let methods = {
            let mut methods = self.methods();
            self.destroyed.store(true, Ordering::Release);
            std::mem::take(&mut *methods)
        };
        lock(&self.pending_dels).clear();
        drop(methods);
    }

    /// `nrpc_registry_init()`: a host created live, or an archived one connected again; a registry still there is
    /// kept, a destroyed one starts empty.
    pub fn init(&self) {
        let mut methods = self.methods();
        if self.destroyed.load(Ordering::Acquire) {
            methods.clear();
            lock(&self.pending_dels).clear();
            self.destroyed.store(false, Ordering::Release);
        }
    }

    /// The registry's label in C's records (`nrpc_owner_str()`), once its host is placed.
    pub fn set_owner(&self, key: String) {
        let _ = self.owner.set(key);
    }

    /// Whether `nrpc_registry_acquire()` would find the host's registry.
    pub fn exists(&self) -> bool {
        !self.destroyed.load(Ordering::Acquire)
    }

    /// `nrpc_method_register()` from the registering thread (its serving handle, the host's epoch). `Err` carries
    /// C's warning for the caller to write when it refuses; `host` names the owner there.
    pub fn register(&self, host: &str, desc: &MethodDesc) -> Result<(), String> {
        if !self.exists() {
            not_registering(desc.name);
            return Ok(());
        }
        let tags = if desc.tags.is_empty() { &b"top"[..] } else { desc.tags };
        if desc.name.len() > NAME_MAX {
            // C fatal()s here (D135.4)
            return Err(format!(
                "NRPC: refusing to register method on host '{host}': its name is {} bytes, more than {NAME_MAX}",
                desc.name.len()
            ));
        }
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
            priority: if desc.priority == 0 { PRIORITY_DEFAULT } else { desc.priority },
            version: desc.version,
            access: desc.access,
            sync: desc.sync,
            source: desc.source,
            flags: flags_for(&key, tags),
            handler: desc.handler.clone(),
            serving: serving::current(),
            epoch: self.epoch(),
        });
        let displaced = {
            let mut methods = self.methods();
            if !self.exists() {
                // destroyed since the check above
                drop(methods);
                not_registering(desc.name);
                return Ok(());
            }
            match methods.iter_mut().find(|(k, _)| *k == key) {
                Some((_, slot)) => Some(std::mem::replace(slot, Arc::clone(&method))),
                None => {
                    methods.push((key.clone(), method.clone()));
                    None
                }
            }
        };
        if displaced.is_some_and(|old| !old.same_as(&method)) {
            // C names the registry by its owner's handle here, the host by name elsewhere
            let owner = self.owner.get().map_or(host, String::as_str);
            nd_log!(
                LogSource::Daemon,
                Priority::Debug,
                "NRPC: method '{}' of host {owner} re-registered with changes",
                String::from_utf8_lossy(&key)
            );
        }
        Ok(())
    }

    /// `nrpc_method_unregister()`. A plugin removes only what its own serving thread registered; only the daemon
    /// removes dynamic-configuration functions. A removal is queued for the parent when `wants_del` (the host has
    /// a sender) and it is not DynCfg.
    pub fn unregister(&self, name: &[u8], source: Source, wants_del: bool) -> Unregistered {
        if name.is_empty() {
            return Unregistered::NotFound;
        }
        let key = key(&name[..name.len().min(NAME_MAX)]);
        let mut methods = self.methods();
        let Some(i) = methods.iter().position(|(k, _)| *k == key) else {
            return Unregistered::NotFound;
        };
        let method = Arc::clone(&methods[i].1);
        if source == Source::Plugin && serving::is_current(&method.serving) != Some(true) {
            drop(methods);
            return Unregistered::Refused(format!(
                "NRPC: refusing to unregister method '{}' - serving-thread mismatch (registered by another serving thread, \
                 unregister requested by {})",
                String::from_utf8_lossy(name),
                if serving::is_current(&method.serving).is_some() {
                    "current serving thread"
                } else {
                    "a thread with no serving handle"
                }
            ));
        }
        let is_dyncfg = method.flags & FLAG_DYNCFG != 0;
        if source != Source::Daemon && is_dyncfg {
            drop(methods);
            return Unregistered::Refused(format!(
                "NRPC: refusing to unregister dyncfg method '{}' via FUNCTION_DEL",
                String::from_utf8_lossy(name)
            ));
        }
        methods.remove(i);
        drop(methods);
        if !is_dyncfg && wants_del {
            let mut dels = lock(&self.pending_dels);
            if !dels.contains(&key) {
                dels.push(key);
            }
        }
        Unregistered::Removed { manifest: method.flags & (FLAG_DYNCFG | FLAG_RESTRICTED) == 0 }
    }

    /// `nrpc_registry_find()`: the method a sanitized command calls: its whole text, then without its last word,
    /// and so on, the first available match winning over a longer unavailable one. `Err` is C's code and text.
    pub fn find(&self, host: &str, command: &[u8]) -> Result<Arc<Method>, (u16, &'static str)> {
        let mut name = &command[..command.len().min(FUNCTION_MAX)];
        let epoch = self.epoch();
        let mut found = false;
        while !name.is_empty() {
            let method = self.methods().iter().find(|(k, _)| k.as_slice() == name).map(|(_, m)| Arc::clone(m));
            if let Some(method) = method {
                found = true;
                if method.available_at(epoch) {
                    return Ok(method);
                }
                nd_log!(
                    LogSource::Daemon,
                    Priority::Debug,
                    "NRPC: method '{}' is not available. host '{host}', serving = {{ tid: {}, running: {} }}, epoch {{ \
                     owner: {epoch}, stamped: {} }}",
                    String::from_utf8_lossy(command),
                    method.serving.tid(),
                    if method.serving.running() { "yes" } else { "no" },
                    method.epoch
                );
            }
            let end = name.iter().rposition(|&c| is_space(c)).map_or(0, |i| i + 1);
            name = &name[..end];
            let end = name.iter().rposition(|&c| !is_space(c)).map_or(0, |i| i + 1);
            name = &name[..end];
        }
        // a method is removed under the lock its lookup takes, so C's "unregistered" tombstone never shows (D147)
        Err(if found {
            (503, "The plugin that registered this feature, is not currently running.")
        } else {
            (404, "This feature is not available on this host at this time.")
        })
    }

    /// `nrpc_method_available()`: an exact name, available now.
    pub fn available(&self, name: &[u8]) -> bool {
        let epoch = self.epoch();
        self.methods().iter().any(|(k, m)| k.as_slice() == name && m.available_at(epoch))
    }

    /// `registry_foreach()`: the available methods `filter` shows, in registration order, and how many DynCfg
    /// methods were met (for `StreamGlobal`).
    pub fn visible(&self, filter: Filter) -> (Methods, usize) {
        let epoch = self.epoch();
        let mut dyncfg = 0;
        let shown = self
            .methods()
            .iter()
            .filter(|(_, m)| m.available_at(epoch))
            .filter(|(_, m)| {
                if m.flags & FLAG_DYNCFG != 0 {
                    dyncfg += usize::from(filter == Filter::StreamGlobal);
                    return false;
                }
                filter == Filter::StreamGlobal || m.flags & FLAG_RESTRICTED == 0
            })
            .map(|(k, m)| (k.clone(), Arc::clone(m)))
            .collect();
        (shown, dyncfg)
    }

    /// The `FUNCTION_DEL` queue, emptied: the names removed since the last re-list (none without a registry, which
    /// an unregistration racing the destroy may have queued after it).
    pub fn take_pending_dels(&self) -> Vec<Vec<u8>> {
        let dels = std::mem::take(&mut *lock(&self.pending_dels));
        if self.exists() { dels } else { Vec::new() }
    }

    /// The function registered under a name, available or not.
    pub fn get(&self, name: &[u8]) -> Option<Arc<Method>> {
        let key = key(name);
        self.methods().iter().find(|(k, _)| *k == key).map(|(_, m)| Arc::clone(m))
    }

    /// Every registration, available or not, in registration order.
    pub fn all(&self) -> Methods {
        self.methods().clone()
    }
}

#[cfg(test)]
mod tests;
