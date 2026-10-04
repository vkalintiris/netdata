// SPDX-License-Identifier: GPL-3.0-or-later
//
// Golden vector generator for the netdata-agent-health crate's evaluation loop: C's own per-host pass
// (health_event_loop_for_host() of src/health/health_event_loop.c), alert log (health_log.c), alert instances
// (rrdcalc.c) and variable lookup (health_variable.c), run over hand-built hosts and charts as a scenario file
// says, with everything around them stubbed (health-loop-stubs.c).
//
//   gen-loop-vectors tables <directory>
//       delay.tsv    <delay> <multiplier, the float's bits> <maximum> <result>: health_delay_apply_multiplier()
//       units.tsv    <value, the double's bits or nan> <units> <text>: format_value_and_unit() as health_log.c
//                    calls it for an entry's value texts (a 100-byte buffer, precision -1)
//
//   gen-loop-vectors scenario <loop.tsv to append to> <scenario file> <a file for the records>
//       One process per scenario. Rows are <scenario> <step> <kind> ..., a step being one `pass`, `unlink`,
//       `apply`, `store`, `restart`, `cleanup` or `alarm-log` directive:
//         do         the directive
//         next_run   after a pass: the pass's next_run
//         host       pending flags (i = initialization, r = label recheck), health_transitions,
//                    health_last_processed_id, next_log_id, next_alarm_id, pending_transitions, delay_up_to, then
//                    the status snapshot: valid, clear, warning, critical, undefined, uninitialized, its
//                    generation (two more at every publication), then the charts with a pending flag as
//                    <chart>=<flags> separated by spaces (`-` for none)
//         alert      per alert in the host's dictionary order: key, chart, id, next_event_id, status, old_status,
//                    value, old_value, last_status_change_value, run_flags, last_updated, next_update,
//                    last_status_change, db_after, db_before, delay_up_to_timestamp, delay_last, delay_up_current,
//                    delay_down_current, last_repeat, times_repeat, summary, info
//         published  the same alert's published snapshot: key, status, value, run_flags, last_updated,
//                    next_update, last_status_change, last_status_change_value, global_id, db_after, db_before,
//                    delay_up_to_timestamp, delay_last, last_repeat, times_repeat, last_transition_id (as the
//                    count the stubs gave it, 0 for none)
//         log        the unique ids of the memory log's entries, newest first, separated by spaces (`-` for none)
//         entry      per entry of the memory log that is new or differs from its last row: unique_id, alarm_id,
//                    alarm_event_id, old_status, new_status, when, duration, non_clear_duration, delay,
//                    delay_up_to_timestamp, flags, updated_by_id, updates_id, old_value, new_value,
//                    old_value_string, new_value_string, global_id, exec_run_timestamp, exec_code, last_repeat,
//                    name, chart, chart_context, chart_name, units, summary, info, classification, component,
//                    type, exec, recipient, source, config hash, transition_id (as its count), pending_save_count
//         call       what the step called of what is stubbed, in call order (health-loop-stubs.c writes them)
//         record     each log record the step wrote, as C's logfmt line with the record's time and the thread id
//                    blanked; the dates in it are UTC, and a transition id is the UUID the stubs counted out
//         sql        with a real database, every row of health_log, health_log_detail, alert_queue and aclk_queue
//                    in rowid order: the table, then `column=value` per column (an integer as it is, a real with
//                    17 digits, a text quoted and escaped, a blob as x'hex', NULL)
//         body       an `alarm-log` step: the JSON sql_health_alarm_log2json() writes
//
// A scenario file holds a directive per line (`#` starts a comment). Seconds are offsets from T0 = 2000000000.
//   rules <path>                       reads a health.d file (relative to the crate's directory, where this runs)
//   database <0|1|real>                the alert log's load: C's result on an empty table (1, default), or none;
//                                      `real`: C's own sqlite_health.c over a new SQLite file with C's schema:
//                                      the load, the save, the alarm id lookup and a rule's alert_hash row are
//                                      C's statements from then on (say it before `rules`)
//   sql <statement>                    a statement on the real database (what another life of the agent left)
//   retention <seconds>                `[health] health log retention`: read by a host's first pass
//   log-max <n>                        `[health] in memory max health log entries`: read by a host's first pass
//   aclk-config <0|1>                  the host has its ACLK sync configuration (a save then fills alert_queue)
//   hostlabel <name> <value>           the value is the rest of the line
//   chart <id> <name> <context> <family> <units> <update every>     `-` for no family (the chart then takes its
//                                      type, as in the daemon) and for empty units. The chart gets the two
//                                      labels every chart of the daemon has, of the plugin `loop.plugin` and
//                                      the module `loop`
//   label <chart> <name> <value>       the value is the rest of the line
//   dim <chart> <id> <name> <stored value>
//   var host|<chart> <name> <value>
//   live <chart> <0|1>                 collected at every pass: last collection and last entry follow the clock
//   collected <chart> <counter_done> <second|0>
//   entries <chart> <first second|0> <last second|0>
//   obsolete <chart> <0|1>
//   lookup <chart> <code> <value|nan> <null 0|1>
//   gate <0|1>                         the host may run health (rrdhost_should_run_health())
//   gate-for <n>                       the host may run health for n more looks at it, then it may not
//   free-at-gate <chart> <n>           at the n-th look at the gate from here the chart leaves the host's index;
//   free-at-lookup <chart>             or when its lookup is asked for. Its alerts stay, as in the daemon while
//                                      the chart's delete callback waits for the pass; an `unlink` is that callback
//   running <0|1>                      the health service runs, or is stopping
//   saved <0|1>                        the SQL save marks an entry as saved, as C's insert does (default: not)
//   queue <0|1>                        the metadata queue takes an asynchronous save (default: it refuses, and the
//                                      save is made at once on the HEALTH thread)
//   thread health|other                the thread the following steps run on (default: HEALTH)
//   sql-alarm <chart> <name> <id> <next event id>   the alert log's table knows this alarm
//   store                              the metadata thread's store job: every queued save, in arrival order
//   restart                            the agent stops and starts on the same database: the queue's saves are
//                                      dropped, the alerts and the memory log go without an entry or a save, and
//                                      the host is one whose health never ran, its charts new; the next pass loads
//   cleanup                            the hourly cleanup of the host: sql_health_alarm_log_cleanup(), then
//                                      health_alarm_log_cleanup()
//   alarm-log <after> [chart]          /api/v1/alarm_log's body (`after` is a unique id, as the request gives it)
//   running-for <n>                    the service runs for n more looks at it, then it is stopping
//   exiting                            the agent's exit has begun (it cannot be undone)
//   delay-up-to <second|0>             the host's health is postponed until then (what a connecting child gets)
//   pending host-init|host-recheck|chart-init <chart>|chart-recheck <chart>
//   clock <second> [microseconds]
//   pass <now> [hibernate]             one call of the pass
//   unlink <chart>                     rrdcalc_unlink_and_delete_all_rrdset_alerts()
//   apply                              health_apply_prototypes_to_host()
// `unlink`, `apply`, `store`, `restart`, `cleanup` and `alarm-log` are steps too.
//
// Field encoding: see health-oracle.h. A double is `nan` or its bits in hex.

#include "health-loop-oracle.h"
#include "health/rrdvar.h"
#include "database/sqlite/sqlite_health.h"

char *format_value_and_unit(char *value_string, size_t value_string_len, NETDATA_DOUBLE value, const char *units, int precision);

#define T0 2000000000L

static void die(const char *what, const char *detail) {
    fprintf(stdout, "%s: %s\n", what, detail ? detail : "");
    exit(1);
}

static void put_double(FILE *f, NETDATA_DOUBLE v) {
    char text[32];
    oracle_double(text, sizeof(text), v);
    fputs(text, f);
}

// ------------------------------------------------------------------------------------------------
// the tables

static void tables(const char *dir) {
    char path[4096];

    snprintf(path, sizeof(path), "%s/delay.tsv", dir);
    FILE *f = fopen(path, "w");
    if(!f) die("cannot create", path);
    fprintf(f, "# generated by tests/oracle/gen-loop-vectors.c from the C implementation; do not edit\n");
    fprintf(f, "# columns: delay, multiplier (the float's bits), maximum, result\n");
    static const int delays[] = { 0, 1, 2, 3, 4, 5, 7, 16, 59, 60, 120, 3600, 86400, 16777216, 16777217, 1073741824,
                                  2000000000, INT_MAX, -1, -5, -2000000000, INT_MIN };
    static const float multipliers[] = { 0.0f, 0.5f, 0.999f, 1.0f, 1.0000001f, 1.5f, 2.0f, 2.5f, 3.3333333f, 10.0f,
                                         1e10f, -1.0f, -2.5f };
    static const int maxima[] = { 0, 1, 10, 20, 3600, 86400, INT_MAX, -1, INT_MIN };
    for(size_t d = 0; d < sizeof(delays) / sizeof(delays[0]); d++)
        for(size_t m = 0; m < sizeof(multipliers) / sizeof(multipliers[0]); m++)
            for(size_t x = 0; x < sizeof(maxima) / sizeof(maxima[0]); x++) {
                uint32_t bits;
                memcpy(&bits, &multipliers[m], sizeof(bits));
                fprintf(f, "%d\t%08x\t%d\t%d\n", delays[d], (unsigned)bits, maxima[x],
                        health_delay_apply_multiplier(delays[d], multipliers[m], maxima[x]));
            }
    if(ferror(f) || fclose(f) != 0) die("cannot write", path);

    snprintf(path, sizeof(path), "%s/units.tsv", dir);
    f = fopen(path, "w");
    if(!f) die("cannot create", path);
    fprintf(f, "# generated by tests/oracle/gen-loop-vectors.c from the C implementation; do not edit\n");
    fprintf(f, "# columns: value (the double's bits, or nan), units, text\n");
    static const char *units[] = {
        "seconds", "seconds ago", "minutes", "minutes ago", "hours", "hours ago", "on/off", "on-off", "onoff",
        "up/down", "up-down", "updown", "ok/error", "ok-error", "okerror", "ok/failed", "ok-failed", "okfailed",
        "empty", "null", "percentage", "percent", "pcent",
        "", "%", "things", "MiB", "requests/s", "Seconds", "seconds  ago", "ms", "a unit with spaces",
    };
    static const NETDATA_DOUBLE values[] = {
        0.0, -0.0, NAN, INFINITY, -INFINITY,
        0.00001, 0.0001, 0.00012345, 0.001, 0.0099, 0.01, 0.0999, 0.1, 0.4, 0.5, 0.6, 0.99, 0.999, 0.9999, 1.0,
        1.4, 1.5, 1.23456789, 9.99, 9.994, 9.995, 9.9999, 10.0, 10.5, 59.0, 59.5, 60.0, 61.0, 99.4, 99.5, 99.94,
        99.95, 99.99, 100.0, 100.5, 119.0, 120.0, 125.0, 999.4, 999.5, 999.94, 999.95, 999.99, 1000.0, 1000.5,
        1439.0, 1440.0, 1441.0, 3599.0, 3600.0, 3601.0, 3700.0, 86399.0, 86400.0, 86401.0, 90061.0, 172800.0,
        172801.0, 1e6, 1.5e6, 1e9, 1e12, 1e15, 1e18, 1e20, 18446744073709551615.0, 1e300,
        -0.00012345, -0.5, -1.0, -1.5, -9.995, -59.0, -60.0, -99.95, -999.95, -1000.0, -3600.0, -86400.0, -90061.0,
        -1e9, -1e18,
    };
    for(size_t u = 0; u < sizeof(units) / sizeof(units[0]); u++)
        for(size_t v = 0; v < sizeof(values) / sizeof(values[0]); v++) {
            char buf[100 + 1];
            char *text = format_value_and_unit(buf, 100, values[v], units[u], -1);
            put_double(f, values[v]);
            fputc('\t', f);
            oracle_esc(f, units[u]);
            fputc('\t', f);
            oracle_esc(f, text);
            fputc('\n', f);
        }
    if(ferror(f) || fclose(f) != 0) die("cannot write", path);
}

// ------------------------------------------------------------------------------------------------
// a scenario

static RRDHOST host;
static ONEWAYALLOC *owa;
static FILE *out;
static const char *scenario;
static size_t step;
static char *calls_text;
static size_t calls_size, calls_done;
static int records_fd = -1;
static off_t records_read;

static void row(const char *kind) {
    oracle_esc(out, scenario);
    fprintf(out, "\t%zu\t%s", step, kind);
}

static void field(FILE *f, const char *s) {
    fputc('\t', f);
    oracle_esc(f, s);
}

static void field_string(FILE *f, STRING *s) {
    field(f, s ? string2str(s) : NULL);
}

static void field_double(FILE *f, NETDATA_DOUBLE v) {
    fputc('\t', f);
    put_double(f, v);
}

// the last row written of each entry, by unique id: an entry gets a row again only when it changed
static struct entry_row {
    uint32_t unique_id;
    char *text;
} *entry_rows;
static size_t entry_rows_used, entry_rows_size;

static bool entry_row_changed(uint32_t unique_id, char *text) {
    for(size_t i = 0; i < entry_rows_used; i++)
        if(entry_rows[i].unique_id == unique_id) {
            if(strcmp(entry_rows[i].text, text) == 0) {
                free(text);
                return false;
            }
            free(entry_rows[i].text);
            entry_rows[i].text = text;
            return true;
        }
    if(entry_rows_used == entry_rows_size) {
        entry_rows_size = entry_rows_size ? entry_rows_size * 2 : 64;
        entry_rows = reallocz(entry_rows, entry_rows_size * sizeof(*entry_rows));
    }
    entry_rows[entry_rows_used++] = (struct entry_row){ .unique_id = unique_id, .text = text };
    return true;
}

static void sql_row(const char *table, const char *fields) {
    row("sql");
    fprintf(out, "\t%s%s\n", table, fields);
}

static RRDSET *chart_find(const char *id) {
    RRDSET *st = dictionary_get(host.rrdset_root_index, id);
    if(!st) die("unknown chart", id);
    return st;
}

// the log records written since the last step: every line of the capture file, its time and the thread id blanked
static void records(void) {
    fflush(stderr);
    off_t end = lseek(records_fd, 0, SEEK_END);
    if(end <= records_read)
        return;
    size_t len = (size_t)(end - records_read);
    char *text = mallocz(len + 1);
    if(pread(records_fd, text, len, records_read) != (ssize_t)len) die("cannot read the records", NULL);
    text[len] = '\0';
    records_read = end;

    char *save = NULL;
    for(char *line = strtok_r(text, "\n", &save); line; line = strtok_r(NULL, "\n", &save)) {
        // blank the value of `time=` (the line starts with it) and of ` tid=`
        static const char *blanked[] = { "time=", " tid=" };
        for(size_t b = 0; b < 2; b++) {
            char *at = (b == 0) ? ((strncmp(line, blanked[0], 5) == 0) ? line : NULL) : strstr(line, blanked[b]);
            if(!at)
                continue;
            char *value = at + strlen(blanked[b]);
            char *value_end = value;
            while(*value_end && *value_end != ' ')
                value_end++;
            memmove(value, value_end, strlen(value_end) + 1);
        }
        row("record");
        field(out, line);
        fputc('\n', out);
    }
    freez(text);
}

static void dump(const char *directive) {
    row("do");
    field(out, directive);
    fputc('\n', out);

    // the calls of the step, as the stubs recorded them
    fflush(oracle_calls);
    if(calls_size > calls_done) {
        char *save = NULL;
        char *copy = strdupz(calls_text + calls_done);
        for(char *line = strtok_r(copy, "\n", &save); line; line = strtok_r(NULL, "\n", &save)) {
            row("call");
            fprintf(out, "\t%s\n", line);
        }
        freez(copy);
        calls_done = calls_size;
    }

    records();

    RRDHOST_FLAGS flags = rrdhost_flag_get(&host);
    row("host");
    fprintf(out, "\t%s%s\t%zu\t%u\t%u\t%u\t%d\t%ld\t%u\t%u\t%u\t%u\t%u\t%u\t%llu\t",
            (flags & RRDHOST_FLAG_PENDING_HEALTH_INITIALIZATION) ? "i" : "",
            (flags & RRDHOST_FLAG_PENDING_LABEL_RECHECK) ? "r" : "",
            host.health_transitions, host.health_last_processed_id, host.health_log.next_log_id,
            host.health_log.next_alarm_id, (int)host.health.pending_transitions, (long)host.health.delay_up_to,
            (unsigned)host.health.alert_status_snapshot.valid, host.health.alert_status_snapshot.counts.clear,
            host.health.alert_status_snapshot.counts.warning, host.health.alert_status_snapshot.counts.critical,
            host.health.alert_status_snapshot.counts.undefined,
            host.health.alert_status_snapshot.counts.uninitialized,
            (unsigned long long)host.health.alert_status_snapshot.generation);
    bool pending = false;
    for(size_t i = 0; i < oracle.charts_used; i++) {
        RRDSET_FLAGS chart_flags = rrdset_flag_get(oracle.charts[i].st);
        if(!(chart_flags & (RRDSET_FLAG_PENDING_HEALTH_INITIALIZATION | RRDSET_FLAG_PENDING_LABEL_RECHECK)))
            continue;
        fprintf(out, "%s%s=%s%s", pending ? " " : "", string2str(oracle.charts[i].st->id),
                (chart_flags & RRDSET_FLAG_PENDING_HEALTH_INITIALIZATION) ? "i" : "",
                (chart_flags & RRDSET_FLAG_PENDING_LABEL_RECHECK) ? "r" : "");
        pending = true;
    }
    fprintf(out, "%s\n", pending ? "" : "-");

    if(host.rrdcalc_root_index) {
        RRDCALC *rc;
        foreach_rrdcalc_in_rrdhost_read(&host, rc) {
            row("alert");
            field(out, rc_dfe.name);
            field_string(out, rc->chart);
            fprintf(out, "\t%u\t%u\t%s\t%s", rc->id, rc->next_event_id, rrdcalc_status2string(rc->status),
                    rrdcalc_status2string(rc->old_status));
            field_double(out, rc->value);
            field_double(out, rc->old_value);
            field_double(out, rc->last_status_change_value);
            fprintf(out, "\t%08x\t%ld\t%ld\t%ld\t%ld\t%ld\t%ld\t%d\t%d\t%d\t%ld\t%u", (unsigned)rc->run_flags,
                    (long)rc->last_updated, (long)rc->next_update, (long)rc->last_status_change, (long)rc->db_after,
                    (long)rc->db_before, (long)rc->delay_up_to_timestamp, rc->delay_last, rc->delay_up_current,
                    rc->delay_down_current, (long)rc->last_repeat, rc->times_repeat);
            field_string(out, rc->summary);
            field_string(out, rc->info);
            fputc('\n', out);

            RRDCALC_RUNTIME_SNAPSHOT snap;
            rrdcalc_runtime_snapshot_get(rc, &snap);
            row("published");
            field(out, rc_dfe.name);
            fprintf(out, "\t%s", rrdcalc_status2string(snap.status));
            field_double(out, snap.value);
            fprintf(out, "\t%08x\t%ld\t%ld\t%ld", (unsigned)snap.run_flags, (long)snap.last_updated,
                    (long)snap.next_update, (long)snap.last_status_change);
            field_double(out, snap.last_status_change_value);
            fprintf(out, "\t%llu\t%ld\t%ld\t%ld\t%d\t%ld\t%u\t%llu\n", (unsigned long long)snap.global_id,
                    (long)snap.db_after, (long)snap.db_before, (long)snap.delay_up_to_timestamp, snap.delay_last,
                    (long)snap.last_repeat, snap.times_repeat, oracle_uuid_rank(snap.last_transition_id));
        }
        foreach_rrdcalc_in_rrdhost_done(rc);
    }

    row("log");
    fputc('\t', out);
    if(!host.health_log.alarms)
        fputc('-', out);
    for(ALARM_ENTRY *ae = host.health_log.alarms; ae; ae = ae->next)
        fprintf(out, "%s%u", ae == host.health_log.alarms ? "" : " ", ae->unique_id);
    fputc('\n', out);

    for(ALARM_ENTRY *ae = host.health_log.alarms; ae; ae = ae->next) {
        char *text = NULL;
        size_t size = 0;
        FILE *f = open_memstream(&text, &size);
        fprintf(f, "\t%u\t%u\t%u\t%s\t%s\t%ld\t%ld\t%ld\t%d\t%ld\t%08x\t%u\t%u", ae->unique_id, ae->alarm_id,
                ae->alarm_event_id, rrdcalc_status2string(ae->old_status), rrdcalc_status2string(ae->new_status),
                (long)ae->when, (long)ae->duration, (long)ae->non_clear_duration, ae->delay,
                (long)ae->delay_up_to_timestamp, (unsigned)ae->flags, ae->updated_by_id, ae->updates_id);
        field_double(f, ae->old_value);
        field_double(f, ae->new_value);
        field_string(f, ae->old_value_string);
        field_string(f, ae->new_value_string);
        fprintf(f, "\t%llu\t%ld\t%d\t%ld", (unsigned long long)ae->global_id, (long)ae->exec_run_timestamp,
                ae->exec_code, (long)ae->last_repeat);
        field_string(f, ae->name);
        field_string(f, ae->chart);
        field_string(f, ae->chart_context);
        field_string(f, ae->chart_name);
        field_string(f, ae->units);
        field_string(f, ae->summary);
        field_string(f, ae->info);
        field_string(f, ae->classification);
        field_string(f, ae->component);
        field_string(f, ae->type);
        field_string(f, ae->exec);
        field_string(f, ae->recipient);
        field_string(f, ae->source);
        fputc('\t', f);
        for(size_t i = 0; i < sizeof(ae->config_hash_id); i++)
            fprintf(f, "%02x", (unsigned)((const unsigned char *)&ae->config_hash_id)[i]);
        fprintf(f, "\t%llu\t%d", oracle_uuid_rank(ae->transition_id),
                (int)__atomic_load_n(&ae->pending_save_count, __ATOMIC_RELAXED));
        fclose(f);

        // the text is the row's fields, each led by its tab
        uint32_t unique_id = ae->unique_id;
        if(entry_row_changed(unique_id, text)) {
            row("entry");
            for(size_t i = 0; i < entry_rows_used; i++)
                if(entry_rows[i].unique_id == unique_id)
                    fprintf(out, "%s\n", entry_rows[i].text);
        }
    }

    if(oracle.sql_real)
        oracle_sql_rows(sql_row);

    step++;
}

// An agent that stops and starts again on the same database. The exit drops the saves the queue still holds; the
// alerts and the memory log go without an entry or a save (the exit has begun); the new process has a host whose
// health never ran, and every chart is new to it.
static void restart(void) {
    oracle.queued_used = 0;
    exit_initiated_add(EXIT_REASON_SIGTERM);
    rrdcalc_delete_all(&host);
    health_alarm_log_free(&host);
    for(size_t i = 0; i < oracle.deferred_used; i++) {
        __atomic_store_n(&oracle.deferred[i]->pending_save_count, 0, __ATOMIC_RELAXED);
        health_alarm_log_free_one_nochecks_nounlink(oracle.deferred[i]);
    }
    oracle.deferred_used = 0;
    // the new process has no exit reason (exit_initiated_set() only adds one)
    exit_initiated_init();

    rrdhost_flag_clear(&host, RRDHOST_FLAG_INITIALIZED_HEALTH | RRDHOST_FLAG_PENDING_LABEL_RECHECK);
    host.health_log.next_log_id = 0;
    host.health_log.next_alarm_id = 0;
    host.health_max_unique_id = 0;
    host.health_max_alarm_id = 0;
    host.health_last_processed_id = 0;
    host.health_transitions = 0;
    host.health.delay_up_to = 0;
    __atomic_store_n(&host.health.pending_transitions, 0, __ATOMIC_RELAXED);
    memset(&host.health.alert_status_snapshot, 0, sizeof(host.health.alert_status_snapshot));
    for(size_t i = 0; i < oracle.charts_used; i++) {
        if(oracle.charts[i].freed)
            continue;
        rrdset_flag_clear(oracle.charts[i].st, RRDSET_FLAG_PENDING_LABEL_RECHECK);
        rrdset_flag_set(oracle.charts[i].st, RRDSET_FLAG_PENDING_HEALTH_INITIALIZATION);
    }
    rrdhost_flag_set(&host, RRDHOST_FLAG_PENDING_HEALTH_INITIALIZATION);
}

static time_t second(const char *text) {
    // 0 stays 0 (no time); anything else is an offset from T0
    long offset = strtol(text, NULL, 10);
    return (strcmp(text, "0") == 0) ? 0 : (time_t)(T0 + offset);
}

static const char *text_or_empty(const char *s) {
    return (strcmp(s, "-") == 0) ? "" : s;
}

static char *word(char **rest, const char *directive) {
    char *w = *rest ? strsep(rest, " ") : NULL;
    if(!w || !*w) die("a missing argument", directive);
    return w;
}

static void run_scenario(const char *out_path, const char *scenario_path, const char *records_path) {
    out = fopen(out_path, "a");
    if(!out) die("cannot append to", out_path);

    // a real database, when the scenario asks for one, is a new file beside the records
    char sql_path[4096];
    snprintf(sql_path, sizeof(sql_path), "%s.db", records_path);

    // the scenario's name: the file's name without its directory and extension
    char name[256];
    const char *base = strrchr(scenario_path, '/');
    snprintf(name, sizeof(name), "%s", base ? base + 1 : scenario_path);
    char *dot = strrchr(name, '.');
    if(dot) *dot = '\0';
    scenario = name;

    // C's records go to stderr: into a file this program reads back after each step
    records_fd = open(records_path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    if(records_fd < 0 || dup2(records_fd, STDERR_FILENO) < 0) die("cannot capture the records in", records_path);

    oracle_calls = open_memstream(&calls_text, &calls_size);

    // the clock is the scenario's from the first line on
    oracle.clock_s = T0;
    oracle.clock_usec = 123456;

    nd_log_limits_unlimited();
    nd_log_set_priority_level("debug");
    string_init();
    time_grouping_init();
    rrdlabels_aral_init(false);
    health_init_prototypes();
    health_alarm_entry_aral_init();
    health_globals.config.default_exec = string_strdupz("/oracle/plugins.d/alarm-notify.sh");
    health_globals.config.default_recipient = string_strdupz("root");
    health_globals.config.enabled_alerts = simple_pattern_create("*", NULL, SIMPLE_PATTERN_EXACT, true);
    is_health_thread = true;

    host.hostname = string_strdupz("oracle-host");
    host.registry_hostname = string_strdupz("oracle-host");
    snprintf(host.machine_guid, sizeof(host.machine_guid), "11111111-2222-4333-8444-555555555555");
    if(uuid_parse(host.machine_guid, host.host_id.uuid) != 0) die("cannot parse", host.machine_guid);
    host.health.enabled = true;
    host.rrdlabels = rrdlabels_create();
    host.rrdvars = rrdvariables_create();
    host.rrdset_root_index = dictionary_create_advanced(DICT_OPTION_DONT_OVERWRITE_VALUE | DICT_OPTION_FIXED_SIZE,
                                                        NULL, sizeof(RRDSET));
    rrdcalc_rrdhost_index_init(&host);
    localhost = &host;
    owa = onewayalloc_create(0);

    FILE *input = fopen(scenario_path, "r");
    if(!input) die("cannot read", scenario_path);

    char *line = NULL;
    size_t size = 0;
    ssize_t len;
    while((len = getline(&line, &size, input)) > 0) {
        if(line[len - 1] == '\n')
            line[len - 1] = '\0';
        if(!*line || *line == '#')
            continue;

        char *whole = strdupz(line);
        char *rest = line;
        char *directive = strsep(&rest, " ");
        // C's records carry the thread's last error number: none of this program's own making
        errno = 0;

        if(strcmp(directive, "rules") == 0) {
            if(health_readfile(word(&rest, whole), NULL, false) != 1) die("cannot read the rules of", whole);
        }
        else if(strcmp(directive, "database") == 0) {
            char *how = word(&rest, whole);
            if(strcmp(how, "real") == 0) {
                oracle.database = true;
                oracle_sql_open(sql_path);
            }
            else
                oracle.database = atoi(how) != 0;
        }
        else if(strcmp(directive, "sql") == 0) {
            if(!rest || !*rest) die("a missing argument", whole);
            oracle_sql_exec(rest);
        }
        else if(strcmp(directive, "retention") == 0)
            health_globals.config.health_log_retention_s = (uint32_t)strtoul(word(&rest, whole), NULL, 10);
        else if(strcmp(directive, "log-max") == 0)
            health_globals.config.health_log_entries_max = (uint32_t)strtoul(word(&rest, whole), NULL, 10);
        else if(strcmp(directive, "aclk-config") == 0)
            __atomic_store_n(&host.aclk_host_config, atoi(word(&rest, whole)) ? (void *)&host : NULL,
                             __ATOMIC_RELEASE);
        else if(strcmp(directive, "hostlabel") == 0) {
            char *label = word(&rest, whole);
            if(!rest || !*rest) die("a missing argument", whole);
            rrdlabels_add(host.rrdlabels, label, rest, RRDLABEL_SRC_CONFIG);
        }
        else if(strcmp(directive, "chart") == 0) {
            if(oracle.charts_used == ORACLE_CHARTS_MAX) die("too many charts", whole);
            char *id = word(&rest, whole);
            RRDSET *st = dictionary_set(host.rrdset_root_index, id, NULL, sizeof(RRDSET));
            st->id = string_strdupz(id);
            st->name = string_strdupz(word(&rest, whole));
            st->context = string_strdupz(word(&rest, whole));
            // rrdset_insert_callback(): a chart without a family takes its type
            char *family = word(&rest, whole);
            if(strcmp(family, "-") == 0)
                st->family = string_strndupz(id, strcspn(id, "."));
            else
                st->family = string_strdupz(family);
            st->units = string_strdupz(text_or_empty(word(&rest, whole)));
            st->update_every = atoi(word(&rest, whole));
            st->rrdlabels = rrdlabels_create();
            // rrdset_update_permanent_labels()
            st->plugin_name = string_strdupz("loop.plugin");
            st->module_name = string_strdupz("loop");
            rrdlabels_add(st->rrdlabels, "_collect_plugin", rrdset_plugin_name(st),
                          RRDLABEL_SRC_AUTO | RRDLABEL_FLAG_DONT_DELETE);
            rrdlabels_add(st->rrdlabels, "_collect_module", rrdset_module_name(st),
                          RRDLABEL_SRC_AUTO | RRDLABEL_FLAG_DONT_DELETE);
            st->rrdvars = rrdvariables_create();
            st->rrdhost = &host;
            rw_spinlock_init(&st->alerts.spinlock);
            st->rrddim_root_index = dictionary_create_advanced(
                DICT_OPTION_DONT_OVERWRITE_VALUE | DICT_OPTION_FIXED_SIZE, NULL, sizeof(RRDDIM));
            // collected, with data that began two days ago, until the scenario says otherwise
            st->counter_done = 2;
            st->last_collected_time.tv_sec = oracle.clock_s;
            oracle.charts[oracle.charts_used++] = (struct oracle_chart){
                .st = st, .live = true, .first_entry_s = T0 - 2 * 86400, .last_entry_s = oracle.clock_s,
                .lookup_code = 200, .lookup_value = NAN, .lookup_null = 1,
            };
            // what a new chart asks of health (rrdset_insert_callback())
            rrdset_flag_set(st, RRDSET_FLAG_PENDING_HEALTH_INITIALIZATION);
            rrdhost_flag_set(&host, RRDHOST_FLAG_PENDING_HEALTH_INITIALIZATION);
        }
        else if(strcmp(directive, "label") == 0) {
            RRDSET *st = chart_find(word(&rest, whole));
            char *label = word(&rest, whole);
            if(!rest || !*rest) die("a missing argument", whole);
            rrdlabels_add(st->rrdlabels, label, rest, RRDLABEL_SRC_CONFIG);
        }
        else if(strcmp(directive, "dim") == 0) {
            RRDSET *st = chart_find(word(&rest, whole));
            char *id = word(&rest, whole);
            RRDDIM *rd = dictionary_set(st->rrddim_root_index, id, NULL, sizeof(RRDDIM));
            rd->id = string_strdupz(id);
            rd->name = string_strdupz(word(&rest, whole));
            rd->rrdset = st;
            rd->collector.last_stored_value = str2ndd(word(&rest, whole), NULL);
        }
        else if(strcmp(directive, "var") == 0) {
            char *where = word(&rest, whole);
            char *var = word(&rest, whole);
            NETDATA_DOUBLE value = str2ndd(word(&rest, whole), NULL);
            if(strcmp(where, "host") == 0)
                rrdvar_host_variable_set(&host, rrdvar_host_variable_add_and_acquire(&host, var), value);
            else {
                RRDSET *st = chart_find(where);
                rrdvar_chart_variable_set(st, rrdvar_chart_variable_add_and_acquire(st, var), value);
            }
        }
        else if(strcmp(directive, "live") == 0) {
            RRDSET *st = chart_find(word(&rest, whole));
            oracle_chart(st)->live = atoi(word(&rest, whole)) != 0;
        }
        else if(strcmp(directive, "collected") == 0) {
            RRDSET *st = chart_find(word(&rest, whole));
            st->counter_done = (size_t)atoi(word(&rest, whole));
            st->last_collected_time.tv_sec = second(word(&rest, whole));
        }
        else if(strcmp(directive, "entries") == 0) {
            struct oracle_chart *script = oracle_chart(chart_find(word(&rest, whole)));
            script->first_entry_s = second(word(&rest, whole));
            script->last_entry_s = second(word(&rest, whole));
        }
        else if(strcmp(directive, "obsolete") == 0) {
            RRDSET *st = chart_find(word(&rest, whole));
            if(atoi(word(&rest, whole)))
                rrdset_flag_set(st, RRDSET_FLAG_OBSOLETE);
            else
                rrdset_flag_clear(st, RRDSET_FLAG_OBSOLETE);
        }
        else if(strcmp(directive, "lookup") == 0) {
            struct oracle_chart *script = oracle_chart(chart_find(word(&rest, whole)));
            script->lookup_code = atoi(word(&rest, whole));
            char *value = word(&rest, whole);
            script->lookup_value = (strcmp(value, "nan") == 0) ? NAN : str2ndd(value, NULL);
            script->lookup_null = atoi(word(&rest, whole));
        }
        else if(strcmp(directive, "gate") == 0)
            oracle.gate = atoi(word(&rest, whole)) != 0;
        else if(strcmp(directive, "gate-for") == 0)
            oracle.gate_for = (size_t)atoi(word(&rest, whole));
        else if(strcmp(directive, "free-at-gate") == 0) {
            struct oracle_chart *script = oracle_chart(chart_find(word(&rest, whole)));
            script->free_at_gate = (size_t)atoi(word(&rest, whole));
        }
        else if(strcmp(directive, "free-at-lookup") == 0)
            oracle_chart(chart_find(word(&rest, whole)))->free_at_lookup = true;
        else if(strcmp(directive, "running") == 0)
            oracle.running = atoi(word(&rest, whole)) != 0;
        else if(strcmp(directive, "running-for") == 0)
            oracle.running_for = (size_t)atoi(word(&rest, whole));
        else if(strcmp(directive, "exiting") == 0)
            exit_initiated_add(EXIT_REASON_SIGTERM);
        else if(strcmp(directive, "delay-up-to") == 0)
            host.health.delay_up_to = second(word(&rest, whole));
        else if(strcmp(directive, "saved") == 0)
            oracle.save_sets_saved = atoi(word(&rest, whole)) != 0;
        else if(strcmp(directive, "queue") == 0)
            oracle.queue_accepts = atoi(word(&rest, whole)) != 0;
        else if(strcmp(directive, "thread") == 0) {
            char *which = word(&rest, whole);
            if(strcmp(which, "health") != 0 && strcmp(which, "other") != 0) die("an unknown thread", whole);
            is_health_thread = strcmp(which, "health") == 0;
        }
        else if(strcmp(directive, "sql-alarm") == 0) {
            if(oracle.sql_alarms_used == ORACLE_SQL_ALARMS_MAX) die("too many alarms in the table", whole);
            struct oracle_sql_alarm *known = &oracle.sql_alarms[oracle.sql_alarms_used++];
            snprintf(known->chart, sizeof(known->chart), "%s", word(&rest, whole));
            snprintf(known->name, sizeof(known->name), "%s", word(&rest, whole));
            known->alarm_id = (uint32_t)strtoul(word(&rest, whole), NULL, 10);
            known->next_event_id = (uint32_t)strtoul(word(&rest, whole), NULL, 10);
        }
        else if(strcmp(directive, "pending") == 0) {
            char *what = word(&rest, whole);
            if(strcmp(what, "host-init") == 0)
                rrdhost_flag_set(&host, RRDHOST_FLAG_PENDING_HEALTH_INITIALIZATION);
            else if(strcmp(what, "host-recheck") == 0)
                rrdhost_flag_set(&host, RRDHOST_FLAG_PENDING_LABEL_RECHECK);
            else if(strcmp(what, "chart-init") == 0)
                rrdset_flag_set(chart_find(word(&rest, whole)), RRDSET_FLAG_PENDING_HEALTH_INITIALIZATION);
            else if(strcmp(what, "chart-recheck") == 0)
                rrdset_flag_set(chart_find(word(&rest, whole)), RRDSET_FLAG_PENDING_LABEL_RECHECK);
            else
                die("unknown pending flag", whole);
        }
        else if(strcmp(directive, "clock") == 0) {
            oracle.clock_s = T0 + strtol(word(&rest, whole), NULL, 10);
            oracle.clock_usec = (rest && *rest) ? (usec_t)strtoul(rest, NULL, 10) : 123456;
        }
        else if(strcmp(directive, "pass") == 0) {
            time_t now = T0 + strtol(word(&rest, whole), NULL, 10);
            bool hibernate = rest && strcmp(rest, "hibernate") == 0;
            if(rest && *rest && !hibernate) die("an unknown word after the pass's second", whole);
            for(size_t i = 0; i < oracle.charts_used; i++)
                if(oracle.charts[i].live) {
                    oracle.charts[i].st->last_collected_time.tv_sec = oracle.clock_s;
                    oracle.charts[i].last_entry_s = oracle.clock_s;
                }
            time_t next_run = now + health_globals.config.run_at_least_every_seconds;
            oracle_health_event_loop_for_host(&host, hibernate, now, &next_run, owa);
            row("next_run");
            fprintf(out, "\t%ld\n", (long)next_run);
            dump(whole);
        }
        else if(strcmp(directive, "unlink") == 0) {
            rrdcalc_unlink_and_delete_all_rrdset_alerts(chart_find(word(&rest, whole)));
            dump(whole);
        }
        else if(strcmp(directive, "apply") == 0) {
            health_apply_prototypes_to_host(&host);
            dump(whole);
        }
        else if(strcmp(directive, "store") == 0) {
            oracle_store();
            dump(whole);
        }
        else if(strcmp(directive, "restart") == 0) {
            restart();
            dump(whole);
        }
        else if(strcmp(directive, "cleanup") == 0) {
            sql_health_alarm_log_cleanup(&host);
            health_alarm_log_cleanup(&host);
            dump(whole);
        }
        else if(strcmp(directive, "alarm-log") == 0) {
            time_t after = (time_t)strtoul(word(&rest, whole), NULL, 0);
            BUFFER *wb = buffer_create(0, NULL);
            sql_health_alarm_log2json(&host, wb, after, (rest && *rest) ? rest : NULL);
            row("body");
            field(out, buffer_tostring(wb));
            fputc('\n', out);
            buffer_free(wb);
            dump(whole);
        }
        else
            die("unknown directive", whole);

        freez(whole);
    }
    free(line);
    fclose(input);
    if(ferror(out) || fclose(out) != 0) die("cannot write", out_path);
}

int main(int argc, char **argv) {
    if(argc == 3 && strcmp(argv[1], "tables") == 0) {
        tables(argv[2]);
        return 0;
    }
    if(argc == 5 && strcmp(argv[1], "scenario") == 0) {
        run_scenario(argv[2], argv[3], argv[4]);
        return 0;
    }
    fprintf(stdout, "usage: %s tables <directory> | scenario <loop.tsv> <scenario file> <records file>\n", argv[0]);
    return 1;
}
