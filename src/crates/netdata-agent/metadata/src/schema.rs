//! The SQL text of `netdata-meta.db` and `context-meta.db` (`src/database/sqlite/sqlite_metadata.c`,
//! `sqlite_context.c`, `sqlite_db_migration.c`), copied from C byte for byte: SQLite keeps a table's `CREATE` text
//! in the file, and C's double-quoted literals rely on the default DQS setting.

/// `database_config`: the tables, triggers and indexes of `netdata-meta.db`.
pub const META_CONFIG: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS host(host_id BLOB PRIMARY KEY, hostname TEXT NOT NULL, registry_hostname TEXT NOT NULL default 'unknown', update_every INT NOT NULL default 1, os TEXT NOT NULL default 'unknown', timezone TEXT NOT NULL default 'unknown', tags TEXT NOT NULL default '',hops INT NOT NULL DEFAULT 0,memory_mode INT DEFAULT 0, abbrev_timezone TEXT DEFAULT '', utc_offset INT NOT NULL DEFAULT 0,program_name TEXT NOT NULL DEFAULT 'unknown', program_version TEXT NOT NULL DEFAULT 'unknown', entries INT NOT NULL DEFAULT 0,health_enabled INT NOT NULL DEFAULT 0, last_connected INT NOT NULL DEFAULT 0)",
    "CREATE TABLE IF NOT EXISTS chart(chart_id blob PRIMARY KEY, host_id blob, type text, id text, name text, family text, context text, title text, unit text, plugin text, module text, priority int, update_every int, chart_type int, memory_mode int, history_entries)",
    "CREATE TABLE IF NOT EXISTS dimension(dim_id blob PRIMARY KEY, chart_id blob, id text, name text, multiplier int, divisor int , algorithm int, options text)",
    "CREATE TABLE IF NOT EXISTS metadata_migration(filename text, file_size, date_created int)",
    "CREATE TABLE IF NOT EXISTS chart_label(chart_id blob, source_type int, label_key text, label_value text, date_created int, PRIMARY KEY (chart_id, label_key))",
    "CREATE TRIGGER IF NOT EXISTS del_chart_label AFTER DELETE ON chart BEGIN DELETE FROM chart_label WHERE chart_id = old.chart_id; END",
    "CREATE TRIGGER IF NOT EXISTS del_chart AFTER DELETE ON dimension FOR EACH ROW BEGIN  DELETE FROM chart WHERE chart_id = OLD.chart_id   AND NOT EXISTS (SELECT 1 FROM dimension WHERE chart_id = OLD.chart_id);END",
    "CREATE TABLE IF NOT EXISTS node_instance (host_id blob PRIMARY KEY, claim_id, node_id, date_created)",
    "CREATE TABLE IF NOT EXISTS alert_hash(hash_id blob PRIMARY KEY, date_updated int, alarm text, template text, on_key text, class text, component text, type text, os text, hosts text, lookup text, every text, units text, calc text, families text, plugin text, module text, charts text, green text, red text, warn text, crit text, exec text, to_key text, info text, delay text, options text, repeat text, host_labels text, p_db_lookup_dimensions text, p_db_lookup_method text, p_db_lookup_options int, p_db_lookup_after int, p_db_lookup_before int, p_update_every int, source text, chart_labels text, summary text, time_group_condition INT, time_group_value DOUBLE, dims_group INT, data_source INT)",
    "CREATE TABLE IF NOT EXISTS host_info(host_id blob, system_key text NOT NULL, system_value text NOT NULL, date_created INT, PRIMARY KEY(host_id, system_key))",
    "CREATE TABLE IF NOT EXISTS host_label(host_id blob, source_type int, label_key text NOT NULL, label_value text NOT NULL, date_created INT, PRIMARY KEY (host_id, label_key))",
    "CREATE TRIGGER IF NOT EXISTS ins_host AFTER INSERT ON host BEGIN INSERT INTO node_instance (host_id, date_created) SELECT new.host_id, unixepoch() WHERE new.host_id NOT IN (SELECT host_id FROM node_instance); END",
    "CREATE TABLE IF NOT EXISTS health_log (health_log_id INTEGER PRIMARY KEY, host_id blob, alarm_id int, config_hash_id blob, name text, chart text, family text, recipient text, units text, exec text, chart_context text, last_transition_id blob, chart_name text, UNIQUE (host_id, alarm_id))",
    "CREATE TABLE IF NOT EXISTS health_log_detail (health_log_id int, unique_id int, alarm_id int, alarm_event_id int, updated_by_id int, updates_id int, when_key int, duration int, non_clear_duration int, flags int, exec_run_timestamp int, delay_up_to_timestamp int, info text, exec_code int, new_status real, old_status real, delay int, new_value double, old_value double, last_repeat int, transition_id blob, global_id int, summary text)",
    "CREATE INDEX IF NOT EXISTS ind_d2 on dimension (chart_id)",
    "CREATE INDEX IF NOT EXISTS ind_c3 on chart (host_id)",
    "CREATE INDEX IF NOT EXISTS health_log_ind_1 ON health_log (host_id)",
    "CREATE INDEX IF NOT EXISTS health_log_d_ind_2 ON health_log_detail (global_id)",
    "CREATE INDEX IF NOT EXISTS health_log_d_ind_3 ON health_log_detail (transition_id)",
    "CREATE INDEX IF NOT EXISTS health_log_d_ind_9 ON health_log_detail (unique_id DESC, health_log_id)",
    "CREATE INDEX IF NOT EXISTS health_log_d_ind_6 on health_log_detail (health_log_id, when_key)",
    "CREATE TABLE IF NOT EXISTS agent_event_log (id INTEGER PRIMARY KEY, version TEXT, event_type INT, value, date_created INT)",
    "CREATE INDEX IF NOT EXISTS idx_agent_event_log1 on agent_event_log (event_type)",
    "CREATE TABLE IF NOT EXISTS alert_queue  (host_id BLOB, health_log_id INT, unique_id INT, alarm_id INT, status INT, date_scheduled INT,  UNIQUE(host_id, health_log_id, alarm_id))",
    "CREATE INDEX IF NOT EXISTS ind_alert_queue1 ON alert_queue(host_id, date_scheduled)",
    "CREATE TABLE IF NOT EXISTS alert_version (health_log_id INTEGER PRIMARY KEY, unique_id INT, status INT, version INT, date_submitted INT)",
    "CREATE TABLE IF NOT EXISTS aclk_queue (sequence_id INTEGER PRIMARY KEY, host_id blob, health_log_id INT, unique_id INT, date_created INT,  UNIQUE(host_id, health_log_id))",
    "CREATE TABLE IF NOT EXISTS alert_hash_cloud (hash_id BLOB PRIMARY KEY)",
    "CREATE TABLE IF NOT EXISTS ctx_metadata_cleanup (id INTEGER PRIMARY KEY, host_id BLOB, context TEXT NOT NULL, date_created INT NOT NULL, UNIQUE (host_id, context))",
];

/// `database_cleanup`: orphan rows and retired indexes, removed on every start.
pub const META_CLEANUP: &[&str] = &[
    "DELETE FROM host WHERE host_id NOT IN (SELECT host_id FROM chart)",
    "DELETE FROM node_instance WHERE host_id NOT IN (SELECT host_id FROM host)",
    "DELETE FROM host_info WHERE host_id NOT IN (SELECT host_id FROM host)",
    "DELETE FROM host_label WHERE host_id NOT IN (SELECT host_id FROM host)",
    "DELETE FROM ctx_metadata_cleanup WHERE host_id NOT IN (SELECT host_id FROM host)",
    "DROP TRIGGER IF EXISTS tr_dim_del",
    "DROP INDEX IF EXISTS ind_d1",
    "DROP INDEX IF EXISTS ind_c1",
    "DROP INDEX IF EXISTS ind_c2",
    "DROP INDEX IF EXISTS alert_hash_index",
    "DROP INDEX IF EXISTS health_log_d_ind_4",
    "DROP INDEX IF EXISTS health_log_d_ind_1",
    "DROP INDEX IF EXISTS health_log_d_ind_5",
    "DROP INDEX IF EXISTS health_log_d_ind_7",
    "DROP INDEX IF EXISTS health_log_d_ind_8",
    "DELETE FROM alert_hash_cloud WHERE hash_id NOT IN (SELECT hash_id FROM alert_hash)",
];

/// `database_context_config`.
pub const CONTEXT_CONFIG: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS context (host_id BLOB, id TEXT NOT NULL, version INT NOT NULL, title TEXT NOT NULL, chart_type TEXT NOT NULL, unit TEXT NOT NULL, priority INT NOT NULL, first_time_t INT NOT NULL, last_time_t INT NOT NULL, deleted INT NOT NULL, family TEXT, PRIMARY KEY (host_id, id))",
];

/// `database_context_cleanup`, a `VACUUM` included: on every start.
pub const CONTEXT_CLEANUP: &[&str] = &[
    "DROP TRIGGER IF EXISTS del_context1",
    "DROP TABLE IF EXISTS context_metadata_cleanup",
    "VACUUM",
];

/// `database_migrate_v1_v2`.
pub const MIGRATE_V1_V2: &[&str] = &["ALTER TABLE host ADD hops INTEGER NOT NULL DEFAULT 0"];

/// `database_migrate_v2_v3`.
pub const MIGRATE_V2_V3: &[&str] = &[
    "ALTER TABLE host ADD memory_mode INT NOT NULL DEFAULT 0",
    "ALTER TABLE host ADD abbrev_timezone TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE host ADD utc_offset INT NOT NULL DEFAULT 0",
    "ALTER TABLE host ADD program_name TEXT NOT NULL DEFAULT 'unknown'",
    "ALTER TABLE host ADD program_version TEXT NOT NULL DEFAULT 'unknown'",
    "ALTER TABLE host ADD entries INT NOT NULL DEFAULT 0",
    "ALTER TABLE host ADD health_enabled INT NOT NULL DEFAULT 0",
];

/// `database_migrate_v4_v5`.
pub const MIGRATE_V4_V5: &[&str] = &[
    "DROP TABLE IF EXISTS chart_active",
    "DROP TABLE IF EXISTS dimension_active",
    "DROP TABLE IF EXISTS chart_hash",
    "DROP TABLE IF EXISTS chart_hash_map",
    "DROP VIEW IF EXISTS v_chart_hash",
];

/// `database_migrate_v5_v6`.
pub const MIGRATE_V5_V6: &[&str] = &[
    "DROP TRIGGER IF EXISTS tr_dim_del",
    "DROP TABLE IF EXISTS dimension_delete",
];

/// `database_migrate_v9_v10`.
pub const MIGRATE_V9_V10: &[&str] = &["ALTER TABLE alert_hash ADD chart_labels TEXT"];

/// `database_migrate_v10_v11`.
pub const MIGRATE_V10_V11: &[&str] = &["ALTER TABLE health_log ADD chart_name TEXT"];

/// `database_migrate_v11_v12`.
pub const MIGRATE_V11_V12: &[&str] = &[
    "ALTER TABLE health_log_detail ADD summary TEXT",
    "ALTER TABLE alert_hash ADD summary TEXT",
];

/// `database_migrate_v12_v13_detail`.
pub const MIGRATE_V12_V13_DETAIL: &[&str] = &["ALTER TABLE health_log_detail ADD summary TEXT"];

/// `database_migrate_v12_v13_hash`.
pub const MIGRATE_V12_V13_HASH: &[&str] = &["ALTER TABLE alert_hash ADD summary TEXT"];

/// `database_migrate_v13_v14`.
pub const MIGRATE_V13_V14: &[&str] =
    &["ALTER TABLE host ADD last_connected INT NOT NULL DEFAULT 0"];

/// `database_migrate_v16_v17`.
pub const MIGRATE_V16_V17: &[&str] = &[
    "ALTER TABLE alert_hash ADD time_group_condition INT",
    "ALTER TABLE alert_hash ADD time_group_value DOUBLE",
    "ALTER TABLE alert_hash ADD dims_group INT",
    "ALTER TABLE alert_hash ADD data_source INT",
];

/// `database_migrate_v17_v18`.
pub const MIGRATE_V17_V18: &[&str] = &[
    "ALTER TABLE alert_hash ADD time_group_condition INT",
    "ALTER TABLE alert_hash ADD time_group_value DOUBLE",
    "ALTER TABLE alert_hash ADD dims_group INT",
    "ALTER TABLE alert_hash ADD data_source INT",
];
