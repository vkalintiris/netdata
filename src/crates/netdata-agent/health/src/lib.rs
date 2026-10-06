//! Health: alerts (C: `src/health/`).
//!
//! It holds:
//! - the configuration path: the `health.d` reader (`health_config.c`), the prototype store and its validation
//!   (`health_prototypes.c`), the JSON a rule's hash is made of (`health_dyncfg.c`) and the hash;
//! - linking: which rule goes to which chart ([`matching`]), a host's alerts ([`alerts`], `rrdcalc.c`), and the
//!   steps of a host's health pass that link them ([`link`]), with a streaming child's detach and return;
//! - the variables an alert can name ([`variable`], `health_variable.c`) and what the web API shows of both
//!   ([`api`]);
//! - the evaluation loop: a host's pass ([`pass`], `health_event_loop.c`) with the database lookup ([`lookup`]),
//!   the alert log in memory ([`entry`], `health_log.c`) and its records in the health log;
//! - the alert log's tables ([`sql`], `sqlite_health.c`) and notifications ([`notify`], `health_notifications.c`);
//! - the silencers ([`silencers`], `health_silencers.c`) and the reload of the configuration;
//! - DynCfg's health nodes ([`dyncfg`], `health_dyncfg.c`): an alert's payload, the template and the jobs, and
//!   what a change does to the hosts' alerts;
//! - the badge ([`badge`], `web_buffer_svg.c`).
//!
//! The pass reaches the daemon through [`pass::Env`]: the chart index, the database lookup, the alert log's tables,
//! the metadata thread's queue and the spawn of a notification's command ([`notify`], `health_notifications.c`).
//!
//! `tests/oracle/` runs C's own reader, store, hash, matcher and per-host pass over `tests/corpus/` and the stock
//! files, and C's unit tables, into the vectors under `tests/vectors/` that this code is written against;
//! `tests/vectors/variables/` holds answers recorded from the running C agent.

#![forbid(unsafe_code)]

pub mod alert;
pub mod alerts;
pub mod api;
pub mod badge;
pub mod config;
pub mod dyncfg;
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
pub mod notify;
pub mod pass;
pub mod prototype;
pub mod readfile;
pub mod silencers;
pub mod sql;
pub mod store;
pub mod template;
#[cfg(test)]
pub(crate) mod testing;
pub mod tables;
pub mod variable;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard};

use netdata_agent_dyncfg::model::SourceType;
use netdata_agent_inicfg::paths::recursive_config_double_dir_load;
use netdata_agent_log::netdata_log_error_errno;

use alerts::HostAlerts;
use config::HealthConfig;
use keywords::lossy;
use notify::Executing;
use pass::Env;
use prototype::{Prototypes, Rule};
use silencers::Silencers;

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
    /// `alarm_notifications_in_progress`: the started notifications of logged entries, of every host, in the order
    /// they started, until HEALTH waits for each. A leaf lock: nothing is called while it is held.
    executing: Mutex<VecDeque<Executing>>,
    /// `silencers`: which alerts are disabled or silenced, for every host.
    silencers: Silencers,
    /// `health_globals.prototypes.registering`: on while the rules are registered with DynCfg at a start or a
    /// reload, when a DynCfg rule that comes back is not pushed to the Cloud.
    registering: AtomicBool,
}

impl Health {
    /// `health_plugin_init()` up to the load: an empty store.
    pub fn init(config: HealthConfig, store: StoreSink) -> Arc<Health> {
        let prototypes = RwLock::new(Prototypes::default());
        let silencers = Silencers::new(config.silencers_filename.clone());
        Arc::new(Health {
            config,
            prototypes,
            store,
            hosts: Mutex::default(),
            executing: Mutex::default(),
            silencers,
            registering: AtomicBool::new(false),
        })
    }

    pub fn config(&self) -> &HealthConfig {
        &self.config
    }

    pub fn silencers(&self) -> &Silencers {
        &self.silencers
    }

    /// Taken to find or insert a host's alerts, never held across anything else.
    fn hosts(&self) -> MutexGuard<'_, HashMap<String, Arc<HostAlerts>>> {
        self.hosts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn executing(&self) -> MutexGuard<'_, VecDeque<Executing>> {
        self.executing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether no thread holds the queue of running notifications now.
    #[cfg(test)]
    pub(crate) fn executing_is_free(&self) -> bool {
        self.executing.try_lock().is_ok()
    }

    /// `wait_for_all_notifications_to_finish_before_allowing_health_to_be_cleaned_up()`, which HEALTH calls after
    /// the hosts of an iteration: each running notification is waited for, oldest first, and its entry gets what
    /// the wait found. It stops, with the rest left running, as soon as the service does.
    pub fn wait_for_notifications(&self, env: &dyn Env) {
        loop {
            if self.executing().is_empty() || !env.service_running() {
                return;
            }
            // HEALTH alone takes items out to wait for them; a host's cleanup may have taken this one meanwhile
            let Some(item) = self.executing().pop_front() else {
                return;
            };
            let timeout_s = self.config.notification_execution_timeout_s;
            let code = notify::wait_for_execution(&item.name, item.execution, timeout_s, env);
            if let Some(alerts) = item.alerts.upgrade() {
                alerts.execution_ended(item.unique_id, code);
            }
        }
    }

    pub fn prototypes(&self) -> RwLockReadGuard<'_, Prototypes> {
        self.prototypes.read().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `dictionary_flush()` of the store, as a reload starts.
    pub fn flush(&self) {
        self.prototypes.write().unwrap_or_else(|poisoned| poisoned.into_inner()).flush();
    }

    /// `health_reload_prototypes()`: the rules' DynCfg nodes removed, the store emptied, every `*.conf` file of the
    /// user tree read, and every stock file no user file shadows; then the template and the rules' jobs registered,
    /// which brings the saved DynCfg jobs back. `ctx` is `None` where no DynCfg is (a test of the reader).
    pub fn reload_prototypes(&self, dirs: &ConfigDirs, ctx: Option<&dyncfg::Ctx<'_>>) {
        if let Some(ctx) = ctx {
            self.dyncfg_unregister_all(ctx);
        }
        self.flush();
        recursive_config_double_dir_load(&dirs.user, dirs.stock.as_deref(), b"", &mut |filename, stock| {
            readfile::health_readfile(self, filename, stock);
        });
        if let Some(ctx) = ctx {
            self.dyncfg_register_all(ctx);
        }
    }

    /// `health_plugin_reload()`, which SIGUSR2 and `reload-health` ask for, with health on or off: the rules read
    /// again and registered ([`Health::reload_prototypes`]), then every alert of every host whose health is enabled
    /// and ran deleted and linked again from the new rules.
    pub fn plugin_reload(&self, dirs: &ConfigDirs, ctx: &dyncfg::Ctx<'_>) {
        self.reload_prototypes(dirs, Some(ctx));
        self.apply_prototypes_to_all_hosts(ctx);
    }

    /// `health_prototype_add()`: the rules of one name, as one file entity or one DynCfg payload gives them. Every
    /// rule is validated first; the error is C's reason for the first one that cannot stand, and nothing is added.
    /// Each rule is then hashed and stored alone; its label texts are compiled; and only afterwards it takes the
    /// default exec and recipient.
    pub fn add(&self, rules: Vec<Rule>) -> Result<(), &'static str> {
        self.add_rules(rules, None)
    }

    /// [`Health::add`] of a DynCfg payload's rules: one whose hash the Cloud has no copy of is sent to it.
    pub(crate) fn add_from_dyncfg(&self, rules: Vec<Rule>, cloud: &dyn dyncfg::Cloud) -> Result<(), &'static str> {
        self.add_rules(rules, Some(cloud))
    }

    fn add_rules(&self, mut rules: Vec<Rule>, cloud: Option<&dyn dyncfg::Cloud>) -> Result<(), &'static str> {
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
            // health_prototype_hash_id()'s end: the Cloud is asked only for a DynCfg rule outside a registration
            if rule.config.source_type == SourceType::Dyncfg
                && !self.registering.load(Ordering::Relaxed)
                && let Some(cloud) = cloud
                && !cloud.has(&rule.config.hash_id)
            {
                cloud.send_configuration(&rule.config.hash_id);
            }

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
