//! Health's rows in `netdata-meta.db` (`sqlite_health.c`). So far the `alert_hash` row of an alert configuration.

use netdata_agent_log::netdata_log_error;
use rusqlite::ToSql;
use rusqlite::types::{ToSqlOutput, Value};

use crate::open::MetaDb;
use crate::read::End;
use crate::write::{Step, execute, text};

const SQL_STORE_ALERT_CONFIG_HASH: &str = "INSERT OR REPLACE INTO alert_hash (hash_id, date_updated, alarm, template, \
     on_key, class, component, type, lookup, every, units, calc, \
     green, red, warn, crit, exec, to_key, info, delay, options, repeat, host_labels, \
     p_db_lookup_dimensions, p_db_lookup_method, p_db_lookup_options, p_db_lookup_after, \
     p_db_lookup_before, p_update_every, source, chart_labels, summary, time_group_condition, \
     time_group_value, dims_group, data_source) \
     VALUES (@hash_id,UNIXEPOCH(),@alarm,@template,\
     @on_key,@class,@component,@type,@lookup,@every,@units,@calc,\
     @green,@red,@warn,@crit,@exec,@to_key,@info,@delay,@options,@repeat,@host_labels,\
     @p_db_lookup_dimensions,@p_db_lookup_method,@p_db_lookup_options,@p_db_lookup_after,\
     @p_db_lookup_before,@p_update_every,@source,@chart_labels,@summary, @time_group_condition, \
     @time_group_value, @dims_group, @data_source)";

/// The database lookup of an [`AlertHashRow`]: its five `p_db_lookup_*` columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertHashLookup {
    pub dimensions: Option<Vec<u8>>,
    pub method: &'static str,
    pub options: i32,
    pub after: i32,
    pub before: i32,
}

/// What `sql_alert_store_config()` binds for one alert configuration, taken when the rule is hashed: C binds the
/// statement then and steps it later, on the metadata thread. A `None` is a NULL.
#[derive(Debug, Clone, PartialEq)]
pub struct AlertHashRow {
    pub hash_id: [u8; 16],
    /// The rule's name: an alarm's here, a template's in `template`.
    pub alarm: Option<Vec<u8>>,
    pub template: Option<Vec<u8>>,
    pub on_key: Option<Vec<u8>>,
    pub class: Option<Vec<u8>>,
    pub component: Option<Vec<u8>>,
    pub r#type: Option<Vec<u8>>,
    /// `every` and `p_update_every`.
    pub update_every: i32,
    pub units: Option<Vec<u8>>,
    pub calc: Option<Vec<u8>>,
    pub warn: Option<Vec<u8>>,
    pub crit: Option<Vec<u8>>,
    pub exec: Option<Vec<u8>>,
    pub to_key: Option<Vec<u8>>,
    pub info: Option<Vec<u8>>,
    pub delay: Vec<u8>,
    pub options: Option<&'static str>,
    pub repeat: Option<String>,
    pub host_labels: Option<Vec<u8>>,
    /// NULL in all five columns without a lookup.
    pub lookup: Option<AlertHashLookup>,
    pub source: Option<Vec<u8>>,
    pub chart_labels: Option<Vec<u8>>,
    pub summary: Option<Vec<u8>>,
    pub time_group_condition: i32,
    /// A NaN is stored as NULL, by SQLite.
    pub time_group_value: f64,
    pub dims_group: i32,
    pub data_source: i32,
}

/// `SQLITE3_BIND_TRANSIENT_STRING_OR_NULL()`.
fn text_or_null(value: &Option<Vec<u8>>) -> ToSqlOutput<'_> {
    match value {
        Some(value) => text(value),
        None => ToSqlOutput::Owned(Value::Null),
    }
}

impl MetaDb {
    /// A queued `sql_alert_store_config()` statement, stepped (`execute_statement()`): true when stored; a failed
    /// step logs C's two records. The columns nothing sets (`os`, `hosts`, `families`, `plugin`, `module`, `charts`)
    /// stay NULL.
    pub fn store_alert_config(&self, row: &AlertHashRow) -> bool {
        const NULL: ToSqlOutput<'_> = ToSqlOutput::Owned(Value::Null);
        // `green` and `red` are bound as a NaN double, which SQLite stores as NULL
        let nan = f64::NAN;
        let lookup = row.lookup.as_ref();
        let params: [&dyn ToSql; 35] = [
            &&row.hash_id[..],
            &text_or_null(&row.alarm),
            &text_or_null(&row.template),
            &text_or_null(&row.on_key),
            &text_or_null(&row.class),
            &text_or_null(&row.component),
            &text_or_null(&row.r#type),
            &NULL, // lookup
            &row.update_every,
            &text_or_null(&row.units),
            &text_or_null(&row.calc),
            &nan,
            &nan,
            &text_or_null(&row.warn),
            &text_or_null(&row.crit),
            &text_or_null(&row.exec),
            &text_or_null(&row.to_key),
            &text_or_null(&row.info),
            &text(&row.delay),
            &row.options,
            &row.repeat,
            &text_or_null(&row.host_labels),
            &lookup.map_or(NULL, |lookup| text_or_null(&lookup.dimensions)),
            &lookup.map(|lookup| lookup.method),
            &lookup.map(|lookup| lookup.options),
            &lookup.map(|lookup| i64::from(lookup.after)),
            &lookup.map(|lookup| i64::from(lookup.before)),
            &row.update_every,
            &text_or_null(&row.source),
            &text_or_null(&row.chart_labels),
            &text_or_null(&row.summary),
            &row.time_group_condition,
            &row.time_group_value,
            &row.dims_group,
            &row.data_source,
        ];
        let c = self.lock();
        match execute(&c, SQL_STORE_ALERT_CONFIG_HASH, "sql_alert_store_config", &params) {
            Ok(()) => true,
            Err(Step::Prepare) => false,
            Err(Step::Failed(rc)) => {
                netdata_log_error!("Failed to execute sql statement, rc = {rc}");
                End::Finalize.failed(rc, "execute_statement");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::types::ValueRef;

    use super::*;
    use crate::open::SqliteSettings;

    fn db() -> (tempfile::TempDir, MetaDb) {
        let dir = tempfile::tempdir().unwrap();
        let meta = MetaDb::open(dir.path(), &SqliteSettings::default()).unwrap();
        (dir, meta)
    }

    /// The row of `hash_id`: each column as `name=typeof:value`, without `date_updated`.
    fn stored(meta: &MetaDb, hash_id: &[u8; 16]) -> Vec<String> {
        let c = meta.lock();
        let mut stmt = c.prepare("SELECT * FROM alert_hash WHERE hash_id = ?").unwrap();
        let names: Vec<String> = stmt.column_names().iter().map(|&name| name.to_owned()).collect();
        let mut rows = stmt.query([&hash_id[..]]).unwrap();
        let row = rows.next().unwrap().expect("the row");
        let columns = names
            .iter()
            .enumerate()
            .filter(|(_, name)| *name != "date_updated")
            .map(|(i, name)| match row.get_ref(i).unwrap() {
                ValueRef::Null => format!("{name}=null"),
                ValueRef::Integer(v) => format!("{name}=integer:{v}"),
                ValueRef::Real(v) => format!("{name}=real:{v}"),
                ValueRef::Text(t) => format!("{name}=text:{}", String::from_utf8_lossy(t)),
                ValueRef::Blob(b) => format!("{name}=blob:{}", b.len()),
            })
            .collect();
        assert!(rows.next().unwrap().is_none(), "one row per hash");
        columns
    }

    fn bare(hash_id: [u8; 16]) -> AlertHashRow {
        AlertHashRow {
            hash_id,
            alarm: None,
            template: Some(b"t".to_vec()),
            on_key: Some(b"system.cpu".to_vec()),
            class: None,
            component: None,
            r#type: None,
            update_every: 10,
            units: None,
            calc: None,
            warn: None,
            crit: None,
            exec: None,
            to_key: None,
            info: None,
            delay: b"multiplier 1.0 ".to_vec(),
            options: None,
            repeat: None,
            host_labels: None,
            lookup: None,
            source: None,
            chart_labels: None,
            summary: None,
            time_group_condition: 0,
            time_group_value: f64::NAN,
            dims_group: 0,
            data_source: 0,
        }
    }

    #[test]
    fn a_bare_row_is_nulls_but_for_what_c_always_binds() {
        let (_dir, meta) = db();
        let row = bare([1; 16]);
        assert!(meta.store_alert_config(&row));
        assert_eq!(
            stored(&meta, &row.hash_id),
            [
                "hash_id=blob:16",
                "alarm=null",
                "template=text:t",
                "on_key=text:system.cpu",
                "class=null",
                "component=null",
                "type=null",
                "os=null",
                "hosts=null",
                "lookup=null",
                // an integer bound into a text column
                "every=text:10",
                "units=null",
                "calc=null",
                "families=null",
                "plugin=null",
                "module=null",
                "charts=null",
                "green=null",
                "red=null",
                "warn=null",
                "crit=null",
                "exec=null",
                "to_key=null",
                "info=null",
                "delay=text:multiplier 1.0 ",
                "options=null",
                "repeat=null",
                "host_labels=null",
                "p_db_lookup_dimensions=null",
                "p_db_lookup_method=null",
                "p_db_lookup_options=null",
                "p_db_lookup_after=null",
                "p_db_lookup_before=null",
                "p_update_every=integer:10",
                "source=null",
                "chart_labels=null",
                "summary=null",
                "time_group_condition=integer:0",
                // a NaN
                "time_group_value=null",
                "dims_group=integer:0",
                "data_source=integer:0",
            ]
        );
    }

    /// Every column with a value of its own, so that no two binds can change places unseen.
    #[test]
    fn a_full_row_keeps_every_value_in_its_column_with_its_type() {
        let (_dir, meta) = db();
        let row = AlertHashRow {
            hash_id: [2; 16],
            alarm: Some(b"the alarm".to_vec()),
            template: None,
            on_key: Some(b"the on key".to_vec()),
            class: Some(b"the class".to_vec()),
            component: Some(b"the component".to_vec()),
            r#type: Some(b"the type".to_vec()),
            update_every: 61,
            units: Some(b"the units".to_vec()),
            calc: Some(b"the calc".to_vec()),
            warn: Some(b"the warn".to_vec()),
            crit: Some(b"the crit".to_vec()),
            exec: Some(b"the exec".to_vec()),
            to_key: Some(b"the recipient".to_vec()),
            // bytes that are no text are stored as they are
            info: Some(b"the info caf\xe9".to_vec()),
            delay: b"the delay".to_vec(),
            options: Some("the options"),
            repeat: Some("the repeat".to_owned()),
            host_labels: Some(b"the host labels".to_vec()),
            lookup: Some(AlertHashLookup {
                dimensions: Some(b"the dimensions".to_vec()),
                method: "the method",
                options: 32768,
                after: -600,
                before: -60,
            }),
            source: Some(b"the source".to_vec()),
            chart_labels: Some(b"the chart labels".to_vec()),
            summary: Some(b"the summary".to_vec()),
            time_group_condition: 2,
            time_group_value: 1.5,
            dims_group: 3,
            data_source: 1,
        };
        assert!(meta.store_alert_config(&row));
        let info = String::from_utf8_lossy(b"the info caf\xe9").into_owned();
        assert_eq!(
            stored(&meta, &row.hash_id),
            [
                "hash_id=blob:16",
                "alarm=text:the alarm",
                "template=null",
                "on_key=text:the on key",
                "class=text:the class",
                "component=text:the component",
                "type=text:the type",
                "os=null",
                "hosts=null",
                "lookup=null",
                "every=text:61",
                "units=text:the units",
                "calc=text:the calc",
                "families=null",
                "plugin=null",
                "module=null",
                "charts=null",
                "green=null",
                "red=null",
                "warn=text:the warn",
                "crit=text:the crit",
                "exec=text:the exec",
                "to_key=text:the recipient",
                &format!("info=text:{info}"),
                "delay=text:the delay",
                "options=text:the options",
                "repeat=text:the repeat",
                "host_labels=text:the host labels",
                "p_db_lookup_dimensions=text:the dimensions",
                "p_db_lookup_method=text:the method",
                "p_db_lookup_options=integer:32768",
                "p_db_lookup_after=integer:-600",
                "p_db_lookup_before=integer:-60",
                "p_update_every=integer:61",
                "source=text:the source",
                "chart_labels=text:the chart labels",
                "summary=text:the summary",
                "time_group_condition=integer:2",
                "time_group_value=real:1.5",
                "dims_group=integer:3",
                "data_source=integer:1",
            ]
        );
        // the bytes of the info are the ones given
        let c = meta.lock();
        let info: Vec<u8> = c
            .query_row("SELECT CAST(info AS BLOB) FROM alert_hash WHERE hash_id = ?", [&row.hash_id[..]], |r| r.get(0))
            .unwrap();
        assert_eq!(info, b"the info caf\xe9");
        drop(c);

        // a template, a lookup without dimensions, and a delay text that is empty and still a text
        let row = AlertHashRow {
            delay: Vec::new(),
            lookup: Some(AlertHashLookup { dimensions: None, method: "sum", options: 0, after: -60, before: 0 }),
            ..bare([3; 16])
        };
        assert!(meta.store_alert_config(&row));
        let columns = stored(&meta, &row.hash_id);
        for expected in [
            "alarm=null",
            "template=text:t",
            "delay=text:",
            "p_db_lookup_dimensions=null",
            "p_db_lookup_method=text:sum",
            "p_db_lookup_options=integer:0",
            "p_db_lookup_after=integer:-60",
            "p_db_lookup_before=integer:0",
        ] {
            assert!(columns.iter().any(|column| column == expected), "{expected} is not in {columns:?}");
        }
    }

    #[test]
    fn a_hash_stored_again_replaces_its_row() {
        let (_dir, meta) = db();
        assert!(meta.store_alert_config(&bare([4; 16])));
        assert!(meta.store_alert_config(&bare([5; 16])));
        let again = AlertHashRow { update_every: 20, ..bare([4; 16]) };
        assert!(meta.store_alert_config(&again));
        assert!(stored(&meta, &[4; 16]).contains(&"every=text:20".to_owned()));

        // the replaced row is a new one, after the others
        let c = meta.lock();
        let order: Vec<Vec<u8>> = c
            .prepare("SELECT hash_id FROM alert_hash ORDER BY rowid")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(order, [vec![5; 16], vec![4; 16]]);
    }
}
