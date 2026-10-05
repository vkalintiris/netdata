// SPDX-License-Identifier: GPL-3.0-or-later
//
// Shared by gen-loop-vectors.c and health-loop-stubs.c: the script the stubs follow and the calls they record.

#ifndef HEALTH_LOOP_ORACLE_H
#define HEALTH_LOOP_ORACLE_H

#include "health-oracle.h"
#include "health/health-alert-entry.h"

// the static per-host pass of src/health/health_event_loop.c, reached through health-loop-splice.inc
void oracle_health_event_loop_for_host(RRDHOST *host, bool apply_hibernation_delay, time_t now, time_t *next_run, ONEWAYALLOC *owa);

// what health_notifications.c and health_log.c call for a notification's command: the spawn, the wait for it in
// slices, its kill, its pid; and the monotonic clock the wait's deadline is counted on
POPEN_INSTANCE *oracle_spawn_popen_run(const char *cmd);
SPAWN_TIMEDWAIT_RESULT oracle_spawn_popen_timedwait(POPEN_INSTANCE *pi, int timeout_ms, int *code);
int oracle_spawn_popen_kill(POPEN_INSTANCE *pi, int timeout_ms);
pid_t oracle_spawn_popen_pid(POPEN_INSTANCE *pi);
usec_t oracle_now_monotonic_usec(void);

// what health_log.c calls for an entry's transition id: the n-th call gives the UUID whose last eight bytes are n
void oracle_uuid_generate_random(nd_uuid_t out);
// that n of a UUID (0 for the nil one)
unsigned long long oracle_uuid_rank(const nd_uuid_t id);

// health_event_loop.c defines it; health_event_loop() sets it for the thread the pass runs on
extern __thread bool is_health_thread;

// what a scenario says of one chart, beside the chart's own fields
struct oracle_chart {
    RRDSET *st;
    bool live;                      // collected at every pass: its last collection and last entry follow the clock
    time_t first_entry_s;           // rrdset_first_entry_s()
    time_t last_entry_s;            // rrdset_last_entry_s()
    int lookup_code;                // rrdset2value_api_v1_with_owa(): 200, 400 or 500
    NETDATA_DOUBLE lookup_value;
    int lookup_null;
    bool freed;                     // out of the host's chart index; its alerts stay until an `unlink`
    size_t free_at_gate;            // when not 0: freed at that many more looks at the gate
    bool free_at_lookup;            // freed when its lookup is asked for
};

#define ORACLE_CHARTS_MAX 32
#define ORACLE_QUEUE_MAX 256
#define ORACLE_SQL_ALARMS_MAX 8
#define ORACLE_EXEC_RULES_MAX 16
#define ORACLE_DYNCFG_MAX 16

// what a notification's command does once it is spawned
enum oracle_exec_kind {
    ORACLE_EXEC_EXIT,               // it runs for `slices` slices of the wait, then exits with `code`
    ORACLE_EXEC_FAIL,               // the spawn fails: spawn_popen_run() gives NULL
    ORACLE_EXEC_ERROR,              // it runs for `slices` slices, then the wait itself breaks
    ORACLE_EXEC_HANG,               // it never exits
};

// a scenario's `exec`: the commands of the entries of that alert (`*`: any) with that new status (`*`: any)
struct oracle_exec_rule {
    char alert[128];
    char status[32];
    enum oracle_exec_kind kind;
    size_t slices;
    int code;
};


// a save the metadata queue took: C keeps the host and a pointer to the live entry
struct oracle_queued {
    RRDHOST *host;
    ALARM_ENTRY *ae;
};

// what the alert log's table knows of one alarm: sql_get_alarm_id() asks by chart and name, whatever the rule's hash
struct oracle_sql_alarm {
    char chart[128];
    char name[128];
    uint32_t alarm_id;
    uint32_t next_event_id;
};

// what a file of the configuration core's (`<varlib>/config/<id>.dyncfg`) holds of one job of health's template: a
// scenario's `dyncfg-saved`
struct oracle_dyncfg_saved {
    char name[128];
    char *payload;
    size_t len;
};

struct oracle_script {
    time_t clock_s;                 // the wall clock every reader of CLOCK_REALTIME in the process gets
    usec_t clock_usec;              // the microseconds inside that second
    bool gate;                      // rrdhost_should_run_health()
    size_t gate_for;                // when not 0: that many more looks find the gate open, then it is closed
    bool running;                   // service_running()
    size_t running_for;             // when not 0: that many more looks find the service running, then it is stopping
    bool database;                  // sql_health_alarm_log_load(): C's result on an empty table, or no database
    bool sql_real;                  // the load, the save and the alarm id lookup are C's own, over a real file
    bool queue_accepts;             // metadata_queue_ae_save(): the queue takes the save, or refuses it
    struct oracle_queued queued[ORACLE_QUEUE_MAX];  // the saves the queue took, in arrival order, until a store
    size_t queued_used;
    ALARM_ENTRY *deferred[ORACLE_QUEUE_MAX];        // entries freed while a save of theirs was queued
    size_t deferred_used;
    bool save_sets_saved;           // sql_health_alarm_log_save(): the entry is marked SAVED, as C's insert does
    struct oracle_sql_alarm sql_alarms[ORACLE_SQL_ALARMS_MAX];  // sql_get_alarm_id(): the alarms the table knows
    size_t sql_alarms_used;
    uint64_t uuids;                 // the random UUIDs given out so far: the next one is this count, plus one
    struct oracle_exec_rule exec_rules[ORACLE_EXEC_RULES_MAX];  // the last rule that matches a command decides
    size_t exec_rules_used;
    int last_executed_ret;          // sql_health_get_last_executed_event() without a real database: -1 the
    RRDCALC_STATUS last_executed_status;    // question failed, 0 no executed event, 1 one with this status
    usec_t monotonic_usec;          // the monotonic clock of health_notifications.c: a slice that ends with the
                                    // command still running moves it on by the slice
    int pids;                       // the commands spawned so far: the next one's pid is 1001 plus this
    size_t commands_running;        // spawned and neither exited nor killed yet
    size_t asked, spawned, saved;   // how often the table was asked for the last executed event, a command was
                                    // spawned (or failed to), an entry was saved: the decision table reads them
    bool hash_not_sent;             // alert_hash_has_transitioned(): the Cloud was not sent this rule yet (default: it
                                    // was, so health pushes nothing)
    struct oracle_dyncfg_saved dyncfg_saved[ORACLE_DYNCFG_MAX];     // the saved jobs, in the core's node order
    size_t dyncfg_saved_used;
    char dyncfg_user_disabled[ORACLE_DYNCFG_MAX][160];              // the ids a saved file says the user disabled
    size_t dyncfg_user_disabled_used;
    struct oracle_chart charts[ORACLE_CHARTS_MAX];
    size_t charts_used;
};

extern struct oracle_script oracle;

// the chart's script (it must have been added by the scenario)
struct oracle_chart *oracle_chart(RRDSET *st);

// the metadata thread's store job, as far as the alert log goes: every queued save, in arrival order
void oracle_store(void);

// whether the service runs, without the look that `running-for` counts
bool oracle_running_peek(void);

// C's own functions of sqlite_health.c, compiled under these names (health-sql-stubs.c)
int c_sql_health_get_last_executed_event(RRDHOST *host, ALARM_ENTRY *ae, RRDCALC_STATUS *last_executed_status);
void c_sql_health_alarm_log_save(RRDHOST *host, ALARM_ENTRY *ae);
void c_sql_health_alarm_log_load(RRDHOST *host);
uint32_t c_sql_get_alarm_id(RRDHOST *host, STRING *chart, STRING *name, uint32_t *next_event_id);
void c_sql_alert_store_config(RRD_ALERT_PROTOTYPE *ap);

// a real metadata database on a new file with C's schema; a statement on it; its alert log tables' rows
void oracle_sql_open(const char *path);
void oracle_sql_exec(const char *statement);
void oracle_sql_rows(void (*each)(const char *table, const char *fields));

// a double as the vectors hold it: `nan`, or its bits in hex
void oracle_double(char *dst, size_t size, NETDATA_DOUBLE v);

// one call the pass made into what is stubbed, in call order: a `call` row of the trace
extern FILE *oracle_calls;
void oracle_call(const char *fmt, ...) PRINTFLIKE(1, 2);

#endif
