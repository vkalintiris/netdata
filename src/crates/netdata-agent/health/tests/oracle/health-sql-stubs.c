// SPDX-License-Identifier: GPL-3.0-or-later
//
// The alert log's SQL for the loop's program (gen-loop-vectors.c): C's own src/database/sqlite/sqlite_health.c and
// sqlite_functions.c are linked, with the build's SQLite, and run over a real database file when a scenario says
// `database real`. This file gives them what the daemon would:
//   - db_meta: opened here on a new file with C's schema (the array database_config[] of sqlite_metadata.c, cut out
//     of that file as text by gen-health-vectors.sh) and the SQL function now_usec() of sqlite_metadata.c;
//   - metadata_execute_store_statement(): C queues a rule's alert_hash statement for the metadata thread's next
//     store job; here it is stepped at once;
//   - the wall clock of SQL's UNIXEPOCH(): SQLite reads it with gettimeofday(), which the program defines beside
//     clock_gettime(), so every time a statement writes is the scenario's;
//   - analytics, pulse, the host's timezone, the metadata database's own init: what the two files reference and no
//     scenario reaches, or constants.
//
// sqlite_health.c is compiled with four of its functions renamed (c_sql_health_alarm_log_save, _load,
// c_sql_get_alarm_id, c_sql_alert_store_config): the stubs of those names in health-loop-stubs.c and
// health-oracle-stubs.c record the call and, with a real database, hand it to C's function.

#include "health-loop-oracle.h"
#include "database/sqlite/sqlite_functions.h"
#include "database/sqlite/sqlite_health.h"
#include "database/sqlite/sqlite_metadata.h"
#include "daemon/analytics.h"
#include <sys/time.h>

sqlite3 *db_meta = NULL;
sqlite3 *db_context_meta = NULL;
RRD_DB_MODE default_rrd_memory_mode = RRD_DB_MODE_DBENGINE;
struct analytics_data analytics_data;

// C's schema of the metadata database: sqlite_metadata.c, cut out by the script
extern const char *database_config[];

void analytics_set_data_str(char **name, const char *value) {
    (void)name;
    (void)value;
}

void pulse_sqlite3_query_completed(bool success, bool busy, bool locked) {
    (void)success;
    (void)busy;
    (void)locked;
}

void pulse_sqlite3_row_completed(void) {
}

// C: src/database/rrdhost.c. The alarm log's JSON prints the host's offset and abbreviated timezone.
RRDHOST_TZ rrdhost_tz_get(RRDHOST *host) {
    (void)host;
    return (RRDHOST_TZ){ .timezone = strdupz("Etc/UTC"), .abbrev_timezone = strdupz("UTC"), .utc_offset = 0 };
}

void rrdhost_tz_free(RRDHOST_TZ *tz) {
    freez(tz->timezone);
    freez(tz->abbrev_timezone);
}

// C: sqlite_metadata.c. Only `-W sqlite-alert-cleanup` opens the database through it: no scenario does.
int sql_init_meta_database(db_check_action_type_t rebuild, int memory) {
    (void)rebuild;
    (void)memory;
    return 1;
}

// C: sqlite_metadata.c, metadata_execute_store_statement(): the statement is queued, and the next store job steps
// and finalizes it.
void metadata_execute_store_statement(sqlite3_stmt *stmt) {
    if(!stmt)
        return;
    int rc = sqlite3_step_monitored(stmt);
    if(rc != SQLITE_DONE) {
        fprintf(stdout, "a queued statement failed, rc = %d\n", rc);
        exit(1);
    }
    sqlite3_finalize(stmt);
}

// C: sqlite_metadata.c, sqlite_now_usec(), without its one-nanosecond sleep for a non-zero argument
static void oracle_now_usec(sqlite3_context *context, int argc, sqlite3_value **argv) {
    (void)argv;
    if(argc != 1) {
        sqlite3_result_null(context);
        return;
    }
    sqlite3_result_int64(context, (sqlite_int64)now_realtime_usec());
}

// health-oracle-stubs.c calls this for every rule a file gives: with a real database C's own function stores the
// rule's alert_hash row
void oracle_loop_store_config(RRD_ALERT_PROTOTYPE *ap) {
    if(oracle.sql_real)
        c_sql_alert_store_config(ap);
}

void oracle_sql_open(const char *path) {
    // a new database: the file an earlier scenario left goes, with its write-ahead log and its index
    static const char *suffixes[] = { "", "-wal", "-shm" };
    for(size_t i = 0; i < sizeof(suffixes) / sizeof(suffixes[0]); i++) {
        char file[4096 + 8];
        snprintf(file, sizeof(file), "%s%s", path, suffixes[i]);
        unlink(file);
    }
    if(sqlite3_open(path, &db_meta) != SQLITE_OK) {
        fprintf(stdout, "cannot open %s: %s\n", path, sqlite3_errmsg(db_meta));
        exit(1);
    }
    // the daemon's journal mode (sqlite_functions.c, configure_sqlite_database()): with a rollback journal SQLite
    // looks for a hot journal at every read, and the failed look leaves an error number that C's next record of
    // the thread would print; the daemon's records have none
    if(sqlite3_exec(db_meta, "PRAGMA journal_mode=WAL", NULL, NULL, NULL) != SQLITE_OK) {
        fprintf(stdout, "cannot set the journal mode of %s: %s\n", path, sqlite3_errmsg(db_meta));
        exit(1);
    }
    for(size_t i = 0; database_config[i]; i++) {
        char *error = NULL;
        if(sqlite3_exec(db_meta, database_config[i], NULL, NULL, &error) != SQLITE_OK) {
            fprintf(stdout, "the schema's statement %zu failed: %s\n", i, error ? error : "");
            exit(1);
        }
    }
    if(sqlite3_create_function(db_meta, "now_usec", 1, SQLITE_ANY, 0, oracle_now_usec, 0, 0) != SQLITE_OK) {
        fprintf(stdout, "cannot register now_usec()\n");
        exit(1);
    }
    oracle.sql_real = true;
}

void oracle_sql_exec(const char *statement) {
    char *error = NULL;
    if(!db_meta || sqlite3_exec(db_meta, statement, NULL, NULL, &error) != SQLITE_OK) {
        fprintf(stdout, "the statement failed: %s: %s\n", statement, error ? error : "no database");
        exit(1);
    }
}

// Every row of the alert log's tables, in rowid order: one call per row with the table and the row as
// `column=value` fields led by tabs (an integer as it is, a real with 17 digits, a text escaped, a blob as hex,
// NULL as `NULL`).
void oracle_sql_rows(void (*each)(const char *table, const char *fields)) {
    static const char *tables[] = { "health_log", "health_log_detail", "alert_queue", "aclk_queue" };
    if(!db_meta)
        return;
    for(size_t t = 0; t < sizeof(tables) / sizeof(tables[0]); t++) {
        char query[128];
        snprintf(query, sizeof(query), "SELECT * FROM %s ORDER BY rowid", tables[t]);
        sqlite3_stmt *stmt = NULL;
        if(sqlite3_prepare_v2(db_meta, query, -1, &stmt, NULL) != SQLITE_OK) {
            fprintf(stdout, "cannot read %s: %s\n", tables[t], sqlite3_errmsg(db_meta));
            exit(1);
        }
        while(sqlite3_step(stmt) == SQLITE_ROW) {
            char *text = NULL;
            size_t size = 0;
            FILE *f = open_memstream(&text, &size);
            for(int c = 0; c < sqlite3_column_count(stmt); c++) {
                fprintf(f, "\t%s=", sqlite3_column_name(stmt, c));
                switch(sqlite3_column_type(stmt, c)) {
                    case SQLITE_NULL:
                        fputs("NULL", f);
                        break;
                    case SQLITE_INTEGER:
                        fprintf(f, "%lld", (long long)sqlite3_column_int64(stmt, c));
                        break;
                    case SQLITE_FLOAT:
                        fprintf(f, "%.17g", sqlite3_column_double(stmt, c));
                        break;
                    case SQLITE_BLOB: {
                        const unsigned char *blob = sqlite3_column_blob(stmt, c);
                        fputs("x'", f);
                        for(int b = 0; b < sqlite3_column_bytes(stmt, c); b++)
                            fprintf(f, "%02x", (unsigned)blob[b]);
                        fputc('\'', f);
                        break;
                    }
                    default:
                        fputc('"', f);
                        oracle_esc(f, (const char *)sqlite3_column_text(stmt, c));
                        fputc('"', f);
                        break;
                }
            }
            fclose(f);
            each(tables[t], text);
            free(text);
        }
        sqlite3_finalize(stmt);
    }
}

// SQLite's wall clock (UNIXEPOCH() in a statement): the scenario's, as clock_gettime() in health-loop-stubs.c
int gettimeofday(struct timeval *restrict tv, void *restrict tz) {
    (void)tz;
    if(oracle.clock_s) {
        tv->tv_sec = oracle.clock_s;
        tv->tv_usec = (suseconds_t)oracle.clock_usec;
        return 0;
    }
    struct timespec ts;
    if(clock_gettime(CLOCK_REALTIME, &ts) != 0)
        return -1;
    tv->tv_sec = ts.tv_sec;
    tv->tv_usec = (suseconds_t)(ts.tv_nsec / 1000);
    return 0;
}
