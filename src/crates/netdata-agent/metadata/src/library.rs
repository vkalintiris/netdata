//! `sqlite_library_init()` (`src/database/sqlite/sqlite_functions.c`): SQLite's heap limits and C's record. The
//! limits are process-wide, so PRAGMAs on a throwaway in-memory connection set them without the C API.

use netdata_agent_log::netdata_log_info;
use netdata_agent_text::size::size_to_string;
use rusqlite::Connection;

pub const HEAP_HARD_LIMIT: i64 = 256 * 1024 * 1024;
pub const HEAP_SOFT_LIMIT: i64 = 32 * 1024 * 1024;

/// `sqlite3_libversion()`.
pub fn version() -> &'static str {
    rusqlite::version()
}

/// Sets the heap limits and logs them as SQLite reads them back.
pub fn init() -> rusqlite::Result<()> {
    let conn = Connection::open_in_memory()?;
    let limit = |pragma: &str, value: i64| -> rusqlite::Result<i64> {
        conn.query_row(&format!("PRAGMA {pragma}={value}"), [], |r| r.get::<_, i64>(0))?;
        conn.query_row(&format!("PRAGMA {pragma}"), [], |r| r.get(0))
    };
    let hard = limit("hard_heap_limit", HEAP_HARD_LIMIT)?;
    let soft = limit("soft_heap_limit", HEAP_SOFT_LIMIT)?;
    let text = |bytes: i64| size_to_string(bytes.max(0) as u64, "B", true).unwrap_or_default();
    netdata_log_info!(
        "SQLITE: heap memory hard limit {}, soft limit {}",
        text(hard),
        text(soft)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `PRAGMA compile_options` of the C agent's SQLite, without the compiler's entry.
    const C_COMPILE_OPTIONS: &[&str] = &[
        "ATOMIC_INTRINSICS=1",
        "DEFAULT_AUTOVACUUM",
        "DEFAULT_CACHE_SIZE=-2000",
        "DEFAULT_FILE_FORMAT=4",
        "DEFAULT_JOURNAL_SIZE_LIMIT=-1",
        "DEFAULT_MMAP_SIZE=0",
        "DEFAULT_PAGE_SIZE=4096",
        "DEFAULT_PCACHE_INITSZ=20",
        "DEFAULT_RECURSIVE_TRIGGERS",
        "DEFAULT_SECTOR_SIZE=4096",
        "DEFAULT_SYNCHRONOUS=2",
        "DEFAULT_WAL_AUTOCHECKPOINT=1000",
        "DEFAULT_WAL_SYNCHRONOUS=2",
        "DEFAULT_WORKER_THREADS=0",
        "DIRECT_OVERFLOW_READ",
        "ENABLE_DBPAGE_VTAB",
        "ENABLE_DBSTAT_VTAB",
        "ENABLE_MEMORY_MANAGEMENT",
        "ENABLE_UPDATE_DELETE_LIMIT",
        "MALLOC_SOFT_LIMIT=1024",
        "MAX_ATTACHED=10",
        "MAX_COLUMN=2000",
        "MAX_COMPOUND_SELECT=500",
        "MAX_DEFAULT_PAGE_SIZE=8192",
        "MAX_EXPR_DEPTH=1000",
        "MAX_FUNCTION_ARG=1000",
        "MAX_LENGTH=1000000000",
        "MAX_LIKE_PATTERN_LENGTH=50000",
        "MAX_MMAP_SIZE=0x7fff0000",
        "MAX_PAGE_COUNT=0xfffffffe",
        "MAX_PAGE_SIZE=65536",
        "MAX_SQL_LENGTH=1000000000",
        "MAX_TRIGGER_DEPTH=1000",
        "MAX_VARIABLE_NUMBER=32766",
        "MAX_VDBE_OP=250000000",
        "MAX_WORKER_THREADS=8",
        "MUTEX_PTHREADS",
        "OMIT_LOAD_EXTENSION",
        "SYSTEM_MALLOC",
        "TEMP_STORE=1",
        "THREADSAFE=1",
    ];

    /// Check `metadata.sqlite-build`: the version, the compile options and the features C's SQL and files rely on.
    #[test]
    fn sqlite_is_built_as_c_builds_it() {
        assert_eq!(version(), "3.53.4");
        let conn = Connection::open_in_memory().unwrap();
        let mut stmt = conn.prepare("PRAGMA compile_options").unwrap();
        let options: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .filter(|o: &String| !o.starts_with("COMPILER="))
            .collect();
        assert_eq!(options, C_COMPILE_OPTIONS);
        conn.execute_batch(
            "CREATE TABLE t(a INTEGER, b TEXT); CREATE INDEX t_a ON t(a);
             INSERT INTO t VALUES (1, 'x'), (2, 'y'), (3, 'z');
             DELETE FROM t WHERE a > 0 ORDER BY a LIMIT 1;
             UPDATE t SET b = 'w' WHERE a > 0 LIMIT 1;",
        )
        .unwrap();
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2);
        for vtab in ["dbstat", "sqlite_dbpage"] {
            conn.query_row(&format!("SELECT count(*) FROM {vtab}"), [], |r| r.get::<_, i64>(0))
                .unwrap_or_else(|e| panic!("{vtab}: {e}"));
        }
        // C runs PRAGMA optimize=0x10002 on every start: without STAT4 it must not add sqlite_stat4
        conn.execute_batch("PRAGMA optimize=0x10002").unwrap();
        let stat4: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'sqlite_stat4'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stat4, 0);
        // double-quoted literals in C's SQL (appendix D §1.4) need the default DQS
        conn.query_row("SELECT INSTR('hidden,x', \"hidden\")", [], |r| r.get::<_, i64>(0))
            .unwrap();
    }

    #[test]
    fn the_heap_limits_are_set_and_logged_as_c_does() {
        let (result, records) = netdata_agent_log::capture(init);
        result.unwrap();
        let messages: Vec<_> = records.into_iter().filter_map(|r| r.message).collect();
        assert_eq!(messages, ["SQLITE: heap memory hard limit 256MiB, soft limit 32MiB"]);
    }
}
