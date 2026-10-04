// SPDX-License-Identifier: GPL-3.0-or-later
//
// Shared by gen-loop-vectors.c and health-loop-stubs.c: the script the stubs follow and the calls they record.

#ifndef HEALTH_LOOP_ORACLE_H
#define HEALTH_LOOP_ORACLE_H

#include "health-oracle.h"
#include "health/health-alert-entry.h"

// the static per-host pass of src/health/health_event_loop.c, reached through health-loop-splice.inc
void oracle_health_event_loop_for_host(RRDHOST *host, bool apply_hibernation_delay, time_t now, time_t *next_run, ONEWAYALLOC *owa);

// what the copies of C's sources call for a notification and for the wait after a repeat's
void oracle_health_send_notification(RRDHOST *host, ALARM_ENTRY *ae, struct health_raised_summary *hrm);
void oracle_health_alarm_wait_for_execution(ALARM_ENTRY *ae);

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

struct oracle_script {
    time_t clock_s;                 // the wall clock every reader of CLOCK_REALTIME in the process gets
    usec_t clock_usec;              // the microseconds inside that second
    bool gate;                      // rrdhost_should_run_health()
    size_t gate_for;                // when not 0: that many more looks find the gate open, then it is closed
    bool running;                   // service_running()
    size_t running_for;             // when not 0: that many more looks find the service running, then it is stopping
    bool database;                  // sql_health_alarm_log_load(): C's result on an empty table, or no database
    bool queue_accepts;             // metadata_queue_ae_save(): the queue takes the save, or refuses it
    struct oracle_queued queued[ORACLE_QUEUE_MAX];  // the saves the queue took, in arrival order, until a store
    size_t queued_used;
    ALARM_ENTRY *deferred[ORACLE_QUEUE_MAX];        // entries freed while a save of theirs was queued
    size_t deferred_used;
    bool save_sets_saved;           // sql_health_alarm_log_save(): the entry is marked SAVED, as C's insert does
    struct oracle_sql_alarm sql_alarms[ORACLE_SQL_ALARMS_MAX];  // sql_get_alarm_id(): the alarms the table knows
    size_t sql_alarms_used;
    uint64_t uuids;                 // the random UUIDs given out so far: the next one is this count, plus one
    struct oracle_chart charts[ORACLE_CHARTS_MAX];
    size_t charts_used;
};

extern struct oracle_script oracle;

// the chart's script (it must have been added by the scenario)
struct oracle_chart *oracle_chart(RRDSET *st);

// the metadata thread's store job, as far as the alert log goes: every queued save, in arrival order
void oracle_store(void);

// a double as the vectors hold it: `nan`, or its bits in hex
void oracle_double(char *dst, size_t size, NETDATA_DOUBLE v);

// one call the pass made into what is stubbed, in call order: a `call` row of the trace
extern FILE *oracle_calls;
void oracle_call(const char *fmt, ...) PRINTFLIKE(1, 2);

#endif
