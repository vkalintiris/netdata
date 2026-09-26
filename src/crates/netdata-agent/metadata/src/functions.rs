//! The SQL functions C registers on `netdata-meta.db` (`src/database/sqlite/sqlite_metadata.c`): `u2h` (never called
//! by C's SQL), `now_usec` and `uuid_random` (used by the v8→v9 migration).

use netdata_agent_log::netdata_log_error;
use netdata_agent_text::parse::uuid_parse_flexi;
use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::ValueRef;

/// Registers the three functions; a failure is recorded and the database is used without it.
pub fn register(conn: &Connection) {
    let text = FunctionFlags::SQLITE_UTF8;
    // `sqlite_uuid_parse()`: NULL on any parse failure (C returns an uninitialised blob for some; brief A3)
    let u2h = conn.create_scalar_function(
        "u2h",
        1,
        text | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let bytes = match ctx.get_raw(0) {
                ValueRef::Text(t) | ValueRef::Blob(t) => t.to_vec(),
                ValueRef::Integer(i) => i.to_string().into_bytes(),
                ValueRef::Real(r) => r.to_string().into_bytes(),
                ValueRef::Null => Vec::new(),
            };
            Ok(uuid_parse_flexi(&bytes).map(|u| u.to_vec()))
        },
    );
    if u2h.is_err() {
        netdata_log_error!("Failed to register internal u2h function");
    }
    // `sqlite_now_usec()`: a non-zero argument sleeps a nanosecond first, so consecutive calls differ
    let now_usec = conn.create_scalar_function("now_usec", 1, text, |ctx| {
        if ctx.get::<i64>(0).unwrap_or(0) != 0 {
            std::thread::sleep(std::time::Duration::from_nanos(1));
        }
        Ok(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_micros() as i64))
    });
    if now_usec.is_err() {
        netdata_log_error!("Failed to register internal now_usec function");
    }
    let uuid_random = conn.create_scalar_function("uuid_random", 0, text, |_| {
        Ok(uuid::Uuid::new_v4().as_bytes().to_vec())
    });
    if uuid_random.is_err() {
        netdata_log_error!("Failed to register internal uuid_random function");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_functions_answer_as_cs() {
        let conn = Connection::open_in_memory().unwrap();
        register(&conn);
        let blob = |sql: &str| {
            conn.query_row(sql, [], |r| r.get::<_, Option<Vec<u8>>>(0))
                .unwrap()
        };
        assert_eq!(
            blob("SELECT u2h('5a1e0000-0000-4000-8000-0000000000aa')").unwrap(),
            [
                0x5a, 0x1e, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 0xaa
            ]
        );
        assert_eq!(blob("SELECT u2h('not a uuid')"), None);
        assert_eq!(blob("SELECT u2h(NULL)"), None);
        assert_eq!(blob("SELECT uuid_random()").unwrap().len(), 16);
        let (a, b): (i64, i64) = conn
            .query_row("SELECT now_usec(1), now_usec(1)", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert!(a > 1_700_000_000_000_000 && b >= a);
        assert!(
            conn.query_row("SELECT now_usec()", [], |r| r.get::<_, i64>(0))
                .is_err()
        );
    }
}
