//! Health: alerts (C: `src/health/`).
//!
//! The crate grows with milestone 9. So far it holds the configuration path: the `health.d` reader
//! (`health_config.c`), the prototype store and its validation (`health_prototypes.c`), the JSON a rule's hash is
//! made of (`health_dyncfg.c`) and the hash. `tests/oracle/` runs C's own reader, store and hash over
//! `tests/corpus/` and the stock files, and C's unit tables, into the vectors under `tests/vectors/` that this
//! code is written against.

#![forbid(unsafe_code)]

pub mod config;
pub mod expr;
pub mod hash;
pub mod json;
pub mod keywords;
pub mod prototype;
pub mod readfile;
pub mod store;
pub mod tables;

use std::sync::{Arc, RwLock, RwLockReadGuard};

use netdata_agent_inicfg::paths::recursive_config_double_dir_load;
use netdata_agent_log::netdata_log_error_errno;

use config::HealthConfig;
use keywords::lossy;
use prototype::{Prototypes, Rule};

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
}

impl Health {
    /// `health_plugin_init()` up to the load: an empty store.
    pub fn init(config: HealthConfig, store: StoreSink) -> Arc<Health> {
        Arc::new(Health { config, prototypes: RwLock::new(Prototypes::default()), store })
    }

    pub fn config(&self) -> &HealthConfig {
        &self.config
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
    /// Each rule is then hashed and stored alone, and only afterwards takes the default exec and recipient.
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
