//! Health: alerts (C: `src/health/`).
//!
//! The crate grows with milestone 9. So far it holds:
//! - the configuration path: the `health.d` reader (`health_config.c`), the prototype store and its validation
//!   (`health_prototypes.c`), the JSON a rule's hash is made of (`health_dyncfg.c`) and the hash;
//! - linking: which rule goes to which chart ([`matching`]), a host's alerts ([`alerts`], `rrdcalc.c`), and the
//!   steps of a host's health pass that link them ([`link`]);
//! - the variables an alert can name ([`variable`], `health_variable.c`) and what the web API shows of both
//!   ([`api`]);
//! - the evaluation loop: a host's pass ([`pass`], `health_event_loop.c`) with the database lookup ([`lookup`]),
//!   the alert log in memory ([`entry`], `health_log.c`) and its records in the health log.
//!
//! The pass reaches the daemon through [`pass::Env`]: saving an entry and notifying about one are calls made at
//! C's places that do nothing yet; the alert log's tables and the notifications follow.
//!
//! `tests/oracle/` runs C's own reader, store, hash, matcher and per-host pass over `tests/corpus/` and the stock
//! files, and C's unit tables, into the vectors under `tests/vectors/` that this code is written against;
//! `tests/vectors/variables/` holds answers recorded from the running C agent.

#![forbid(unsafe_code)]

pub mod alert;
pub mod alerts;
pub mod api;
pub mod config;
pub mod entry;
pub mod expr;
pub mod hash;
mod journal;
pub mod json;
pub mod keywords;
pub mod link;
mod log;
pub mod lookup;
pub mod matching;
pub mod pass;
pub mod prototype;
pub mod readfile;
pub mod store;
pub mod template;
#[cfg(test)]
pub(crate) mod testing;
pub mod tables;
pub mod variable;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};

use netdata_agent_inicfg::paths::recursive_config_double_dir_load;
use netdata_agent_log::netdata_log_error_errno;

use alerts::HostAlerts;
use config::HealthConfig;
use keywords::lossy;
use prototype::{Prototypes, Rule};

/// The wall clock in seconds, read where C reads it (`now_realtime_sec()`); tests pass their own.
pub type Clock<'a> = &'a dyn Fn() -> i64;

/// Where an accepted rule goes to be stored: C's `sql_alert_store_config()`, called once per rule right after
/// its hash is made and before the default exec and recipient are filled.
pub type StoreSink = Box<dyn Fn(&Rule) + Send + Sync>;

/// The two `health.d` trees: `[directories]` `health config` and `stock health config`. There is no stock tree when
/// `[health]` `enable stock health configuration` is off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDirs {
    pub user: Vec<u8>,
    pub stock: Option<Vec<u8>>,
}

/// The health plugin's state (`health_globals`): its configuration and the prototype store.
pub struct Health {
    config: HealthConfig,
    prototypes: RwLock<Prototypes>,
    store: StoreSink,
    /// Each host's alerts, by machine GUID, from the host's first health pass on.
    hosts: Mutex<HashMap<String, Arc<HostAlerts>>>,
    /// Whether the agent has its metadata database: a host's alert log is then loaded at its first pass, which
    /// seeds its ids.
    database: bool,
}

impl Health {
    /// `health_plugin_init()` up to the load: an empty store.
    pub fn init(config: HealthConfig, store: StoreSink, database: bool) -> Arc<Health> {
        let prototypes = RwLock::new(Prototypes::default());
        Arc::new(Health { config, prototypes, store, hosts: Mutex::default(), database })
    }

    pub fn config(&self) -> &HealthConfig {
        &self.config
    }

    /// Taken to find or insert a host's alerts, never held across anything else.
    fn hosts(&self) -> MutexGuard<'_, HashMap<String, Arc<HostAlerts>>> {
        self.hosts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn prototypes(&self) -> RwLockReadGuard<'_, Prototypes> {
        self.prototypes.read().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `dictionary_flush()` of the store, as a reload starts.
    pub fn flush(&self) {
        self.prototypes.write().unwrap_or_else(|poisoned| poisoned.into_inner()).flush();
    }

    /// `health_reload_prototypes()`: the store emptied, then every `*.conf` file of the user tree read, and every
    /// stock file no user file shadows. C removes the prototypes' DynCfg nodes before and registers them after;
    /// those come with DynCfg's health nodes.
    pub fn reload_prototypes(&self, dirs: &ConfigDirs) {
        self.flush();
        recursive_config_double_dir_load(&dirs.user, dirs.stock.as_deref(), b"", &mut |filename, stock| {
            readfile::health_readfile(self, filename, stock);
        });
    }

    /// `health_prototype_add()`: the rules of one name, as one file entity or one DynCfg payload gives them. Every
    /// rule is validated first; the error is C's reason for the first one that cannot stand, and nothing is added.
    /// Each rule is then hashed and stored alone; its label texts are compiled; and only afterwards it takes the
    /// default exec and recipient.
    pub fn add(&self, mut rules: Vec<Rule>) -> Result<(), &'static str> {
        let name = rules.first().and_then(|rule| rule.config.name.clone());
        for (index, rule) in rules.iter().enumerate() {
            if let Some(reason) = rule.validate() {
                netdata_log_error_errno!(
                    "HEALTH: alert '{}' rule {index} is invalid: {reason}. Source: {}",
                    lossy(name.as_deref().unwrap_or(b"")),
                    lossy(rule.config.source.as_deref().unwrap_or(b""))
                );
                return Err(reason);
            }
        }

        let mut enabled = false;
        for rule in &mut rules {
            enabled |= rule.r#match.enabled;
            if rule.config.name.is_none() {
                rule.config.name.clone_from(&name);
            }

            let json = json::prototype_to_json(rule.config.name.as_deref().unwrap_or(b""), &[rule], true);
            rule.config.hash_id = hash::hash_id(&json);
            (self.store)(rule);

            // health_prototype_activate_match_patterns()
            rule.r#match.host_labels_pattern = rule.r#match.host_labels.as_deref().and_then(matching::label_patterns);
            rule.r#match.chart_labels_pattern = rule.r#match.chart_labels.as_deref().and_then(matching::label_patterns);

            if rule.config.exec.is_none() && !self.config.default_exec.is_empty() {
                rule.config.exec = Some(self.config.default_exec.clone());
            }
            if rule.config.recipient.is_none() && !self.config.default_recipient.is_empty() {
                rule.config.recipient = Some(self.config.default_recipient.clone());
            }
        }

        let mut prototypes = self.prototypes.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        prototypes.set(name.as_deref().unwrap_or(b""), rules, enabled);
        Ok(())
    }
}
