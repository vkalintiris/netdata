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
};

#define ORACLE_CHARTS_MAX 32

struct oracle_script {
    time_t clock_s;                 // the wall clock every reader of CLOCK_REALTIME in the process gets
    usec_t clock_usec;              // the microseconds inside that second
    bool gate;                      // rrdhost_should_run_health()
    bool running;                   // service_running()
    size_t running_for;             // when not 0: that many more looks find the service running, then it is stopping
    bool database;                  // sql_health_alarm_log_load(): C's result on an empty table, or no database
    bool queue_accepts;             // metadata_queue_ae_save(): queued, or not (no scenario sets it yet)
    bool save_sets_saved;           // sql_health_alarm_log_save(): the entry is marked SAVED, as C's insert does
    uint32_t sql_alarm_id;          // sql_get_alarm_id(): 0 is "not in the table" (no scenario sets it yet)
    uint32_t sql_next_event_id;
    struct oracle_chart charts[ORACLE_CHARTS_MAX];
    size_t charts_used;
};

extern struct oracle_script oracle;

// the chart's script (it must have been added by the scenario)
struct oracle_chart *oracle_chart(RRDSET *st);

// a double as the vectors hold it: `nan`, or its bits in hex
void oracle_double(char *dst, size_t size, NETDATA_DOUBLE v);

// one call the pass made into what is stubbed, in call order: a `call` row of the trace
extern FILE *oracle_calls;
void oracle_call(const char *fmt, ...) PRINTFLIKE(1, 2);

#endif
