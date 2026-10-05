// SPDX-License-Identifier: GPL-3.0-or-later
//
// Link stubs so that C's per-host health pass runs outside the netdata daemon, over hand-built hosts and charts.
//
// Real, compiled from the reference tree: what health-oracle-stubs.c lists, plus src/health/health_log.c,
// rrdcalc.c, health_notifications.c, health_variable.c, rrdvar.c, src/web/api/v1/api_v1_badge/web_buffer_svg.c
// (health_log.c formats an entry's value texts with it) and a copy of health_event_loop.c with
// health-loop-splice.inc appended. health-oracle-stubs.c is compiled with HEALTH_ORACLE_LOOP: what rrdcalc.c and
// health_event_loop.c define themselves is left out of it.
//
// Stubbed here, each with what the scenario scripts of it:
//   - the gate (rrdhost_should_run_health) and service_running, which is also false once the exit began, as C's;
//   - the chart index: a plain dictionary whose values are the hand-built RRDSETs. A chart the scenario freed is
//     not found in it any more, while its alerts stay: the state between the daemon's delete of a chart from its
//     index and the delete callback that unlinks the chart's alerts, which waits while a pass walks the alerts;
//   - a chart's first and last entry, and the database lookup (rrdset2value_api_v1_with_owa), which records its
//     arguments and answers what the scenario says;
//   - the management key the silencers' request handler asks for (the silencers themselves are C's own);
//   - SQLite: the load of the alert log (C's ids for an empty table, or none; C's load also looks at the running
//     service once and logs a record, which come with the alert log's tables), the save (it marks the entry SAVED
//     or not), the alarm id lookup (the alarms a scenario says the table knows), the alarm's last executed event
//     (what a scenario says, none by default); the ACLK queue; the sending of a host variable to a parent;
//   - the metadata queue and its thread's store job: the queue refuses a save (it is then made at once, on the
//     HEALTH thread only) or takes it, keeping the host and the live entry as C does; a scenario's `store` plays
//     the job's step for the alert log (src/database/sqlite/sqlite_metadata.c, store_alert_transitions(): each
//     queued entry saved as it stands by then, both pending counters taken back whatever the save did), not on the
//     HEALTH thread; an entry freed while a save of its is queued is kept until a store finds it without one;
//   - a notification's command, by #defines for health_notifications.c and health_log.c: spawn_popen_run() records
//     the command line C prepared and starts nothing; what the command then does (it exits with a code after some
//     slices of the wait, the spawn fails, the wait breaks, it never exits) is the scenario's `exec`;
//     spawn_popen_timedwait(), spawn_popen_kill() and spawn_popen_pid() follow it. C's own health_send_notification()
//     and health_alarm_wait_for_execution() run over them;
//   - the monotonic clock, by a #define for health_notifications.c: it stands still but for each slice of a wait
//     that ends with the command still running, which moves it on by the slice (and leaves ETIMEDOUT in errno, as
//     the spawn server's timed wait does);
//   - the walk over a context's charts for the variable lookup;
//   - an entry's transition id, by a #define for health_log.c: instead of a random UUID, the count of the UUIDs
//     given out so far, so that the trace shows which entry got which and what an alert publishes of it;
//   - the wall clock: the program defines clock_gettime(), so every reader of CLOCK_REALTIME, in the health objects
//     and in libnetdata alike, gets the scenario's clock. (A --wrap of now_realtime_sec does nothing here: the
//     function is inlined by the LTO link.)

//
// The `call` rows of the trace, written here in call order (seconds and ids as numbers, flags as 8 hex digits, a
// double as `nan` or its bits):
//   load <database 0|1>
//   sql_get_alarm_id <chart> <name> <the alarm id the table has, 0 for none> <its next event id>
//   queue <unique id> <accepted 0|1>            the metadata queue was offered an entry's save
//   queue_deletion <unique id>                  an entry was freed with a save of its still queued
//   save <unique id> <the entry's flags> <alarm id> <event id> <old status> <new status> <when> <delay up to>
//        <duration> <non-clear duration> <delay> <last repeat> <old value> <new value> <transition id, as its
//        count> <exec_run_timestamp> <exec_code> <updated_by_id>
//                                               the SQL save, before it marks the entry: the entry as it stands
//   last_executed <unique id> <-1|0|1> <the status, `-` without one>   the table was asked for the alarm's last
//                                               executed event, for that entry
//   spawn <pid, 0 when the spawn failed> <the command line, escaped as a field>
//   monotonic                                   the wait read the monotonic clock
//   timedwait <pid> <milliseconds> running|error|exited [<code>]
//   kill <pid> <milliseconds>
//   lookup <chart> <dimensions> <points> <after> <before> <method> <group options> <resampling> <options>
//          <timeout> <tier> <query source> <priority> <the scripted code>
//   commit_alert_transitions
//   process_alert_pending_queue

#include "health-loop-oracle.h"
#include <sys/syscall.h>

struct oracle_script oracle = {
    .gate = true,
    .running = true,
    .database = true,
};

FILE *oracle_calls = NULL;

void oracle_call(const char *fmt, ...) {
    if(!oracle_calls)
        return;
    va_list args;
    va_start(args, fmt);
    vfprintf(oracle_calls, fmt, args);
    fputc('\n', oracle_calls);
    va_end(args);
}

void oracle_double(char *dst, size_t size, NETDATA_DOUBLE v) {
    if(isnan(v)) {
        snprintf(dst, size, "nan");
        return;
    }
    double d = (double)v;
    uint64_t bits;
    memcpy(&bits, &d, sizeof(bits));
    snprintf(dst, size, "%016llx", (unsigned long long)bits);
}

struct oracle_chart *oracle_chart(RRDSET *st) {
    for(size_t i = 0; i < oracle.charts_used; i++)
        if(oracle.charts[i].st == st)
            return &oracle.charts[i];
    fprintf(stdout, "a chart without a script: %s\n", string2str(st->id));
    exit(1);
}

// ------------------------------------------------------------------------------------------------
// the gate, the service, the plugin's init

// C: src/database/rrdhost.c, rrdhost_should_run_health()
bool rrdhost_should_run_health(RRDHOST *host) {
    (void)host;
    for(size_t i = 0; i < oracle.charts_used; i++)
        if(oracle.charts[i].free_at_gate && --oracle.charts[i].free_at_gate == 0)
            oracle.charts[i].freed = true;
    if(oracle.gate_for) {
        if(--oracle.gate_for == 0)
            oracle.gate = false;
        return true;
    }
    return oracle.gate;
}

bool oracle_running_peek(void) {
    return !exit_initiated_get() && (oracle.running_for || oracle.running);
}

// C: src/daemon/daemon-service.c, service_running(): the thread is not cancelled and the exit has not begun.
bool service_running(SERVICE_TYPE service) {
    (void)service;
    if(exit_initiated_get())
        return false;
    if(oracle.running_for) {
        if(--oracle.running_for == 0)
            oracle.running = false;
        return true;
    }
    return oracle.running;
}

// C: src/health/health.c. The program does its two relevant steps itself (the entry ARAL, the prototype store).
void health_plugin_init(void) {
}

// ------------------------------------------------------------------------------------------------
// charts

// C: src/database/rrdset-index-id.c. C leaves an obsolete chart out when include_obsolete is false. A chart the
// scenario freed is not in the index.
RRDSET_ACQUIRED *rrdset_find_and_acquire(RRDHOST *host, const char *id, bool include_obsolete) {
    if(!host->rrdset_root_index)
        return NULL;
    const DICTIONARY_ITEM *item = dictionary_get_and_acquire_item(host->rrdset_root_index, id);
    if(item) {
        RRDSET *st = dictionary_acquired_item_value(item);
        if(oracle_chart(st)->freed || (!include_obsolete && rrdset_flag_check(st, RRDSET_FLAG_OBSOLETE))) {
            dictionary_acquired_item_release(host->rrdset_root_index, item);
            return NULL;
        }
    }
    return (RRDSET_ACQUIRED *)item;
}

RRDSET *rrdset_acquired_to_rrdset(RRDSET_ACQUIRED *rsa) {
    if(!rsa)
        return NULL;
    return (RRDSET *)dictionary_acquired_item_value((const DICTIONARY_ITEM *)rsa);
}

void rrdset_acquired_release(RRDSET_ACQUIRED *rsa) {
    if(!rsa)
        return;
    RRDSET *st = rrdset_acquired_to_rrdset(rsa);
    dictionary_acquired_item_release(st->rrdhost->rrdset_root_index, (const DICTIONARY_ITEM *)rsa);
}

time_t rrdset_first_entry_s(RRDSET *st) {
    return oracle_chart(st)->first_entry_s;
}

time_t rrdset_last_entry_s(RRDSET *st) {
    return oracle_chart(st)->last_entry_s;
}

// C: src/database/contexts/rrdcontext.c. Every chart of the host with that context, in the chart index's order.
int rrdcontext_foreach_instance_with_rrdset_in_context(RRDHOST *host, const char *context, int (*callback)(RRDSET *st, void *data), void *data) {
    if(!host || !context || !*context || !callback || !host->rrdset_root_index)
        return -1;

    int ret = 0, found = 0;
    RRDSET *st;
    dfe_start_read(host->rrdset_root_index, st) {
        if(strcmp(string2str(st->context), context) != 0)
            continue;
        found = 1;
        int r = callback(st, data);
        if(r >= 0)
            ret += r;
        else {
            ret = r;
            break;
        }
    }
    dfe_done(st);
    return found ? ret : -1;
}

// ------------------------------------------------------------------------------------------------
// the lookup

// C: src/web/api/formatters/rrd2json.c. 500 leaves the value and the window untouched and sets the null flag; 400
// zeroes the window and sets the null flag; 200 writes the window, the value and the null flag. The window of a 200
// is made up from the clock and the two arguments: it shows that the alert takes it, not what a query answers.
int rrdset2value_api_v1_with_owa(
    ONEWAYALLOC *owa, RRDSET *st, BUFFER *wb, NETDATA_DOUBLE *n, const char *dimensions, size_t points,
    time_t after, time_t before, RRDR_TIME_GROUPING group_method, const char *group_options,
    time_t resampling_time, uint32_t options, time_t *db_after, time_t *db_before, size_t *db_points_read,
    size_t *db_points_per_tier, size_t *result_points_generated, int *value_is_null, NETDATA_DOUBLE *anomaly_rate,
    time_t timeout, size_t tier, QUERY_SOURCE query_source, STORAGE_PRIORITY priority) {
    (void)owa; (void)wb; (void)db_points_read; (void)db_points_per_tier; (void)result_points_generated;
    (void)anomaly_rate;
    struct oracle_chart *script = oracle_chart(st);

    oracle_call("lookup\t%s\t%s\t%zu\t%ld\t%ld\t%s\t%s\t%ld\t%08x\t%ld\t%zu\t%d\t%d\t%d",
                string2str(st->id), dimensions ? dimensions : "\\x00", points, (long)after, (long)before,
                time_grouping_id2txt(group_method), group_options ? group_options : "\\x00", (long)resampling_time,
                (unsigned)options, (long)timeout, tier, (int)query_source, (int)priority, script->lookup_code);

    if(script->free_at_lookup) {
        script->free_at_lookup = false;
        script->freed = true;
    }

    if(script->lookup_code == 500) {
        if(value_is_null) *value_is_null = 1;
        return 500;
    }
    if(script->lookup_code == 400) {
        if(db_after) *db_after = 0;
        if(db_before) *db_before = 0;
        if(value_is_null) *value_is_null = 1;
        return 400;
    }

    if(db_after) *db_after = oracle.clock_s + after + before;
    if(db_before) *db_before = oracle.clock_s + before;
    *n = script->lookup_value;
    if(value_is_null) *value_is_null = script->lookup_null;
    return 200;
}

// ------------------------------------------------------------------------------------------------
// the silencers

// C's own src/health/health_silencers.c is linked in. Its request handler compares the request's token with the
// management key, which api_v1_manage.c reads from the key file in the daemon.
char *api_secret = "oracle-key";

// ------------------------------------------------------------------------------------------------
// DynCfg, and the Cloud's copy of a rule
//
// C's own src/health/health_dyncfg.c is linked in; the core it registers its nodes with (src/daemon/dyncfg) is not.
// Each call health makes to the core is a `call` row with what health handed over.
//
// A MODEL, not C's code: what the core sends back to health's callback right after a registration, written here from
// src/daemon/dyncfg/dyncfg.c (dyncfg_add_low_level(), dyncfg_send_updates()) for the nodes health has:
//   - the template: `add <name>` with the saved payload for every job a saved file has (a scenario's `dyncfg-saved`),
//     in their order;
//   - a job: `disable` when a saved file says the user disabled it (`dyncfg-user-disabled`), when it is registered
//     disabled, or when the user disabled the template; else `enable`. Then, for a job that is not registered as a
//     DynCfg one and has a saved payload: `update` with that payload.
// Each is a `call` row `echo <id> <command> <name> <payload>` before the callback runs and `echoed <id> <command>
// <code> <body>` after it; what the callback itself calls lies between the two. The rows judge health given these
// calls; that the core makes these calls, in this order, is for the checks against the running agent.

#define DYNCFG_TEMPLATE_ID "health:alert:prototype"

static void call_field(const char *s) {
    fputc('\t', oracle_calls);
    oracle_esc(oracle_calls, s);
}

static void call_field_bytes(const void *bytes, size_t len) {
    fputc('\t', oracle_calls);
    oracle_esc_bytes(oracle_calls, bytes, len);
}

static bool dyncfg_user_disabled(const char *id) {
    for(size_t i = 0; i < oracle.dyncfg_user_disabled_used; i++)
        if(strcmp(oracle.dyncfg_user_disabled[i], id) == 0)
            return true;
    return false;
}

static void dyncfg_echo(const struct dyncfg_add_inline_spec *spec, const char *id, DYNCFG_CMDS cmd, const char *name,
                        const struct oracle_dyncfg_saved *saved) {
    // the id is health's own text, freed when its registration returns
    char *id_copy = strdupz(id);
    fputs("echo", oracle_calls);
    call_field(id_copy);
    call_field(dyncfg_id2cmd_one(cmd));
    call_field(name ? name : "-");
    if(saved)
        call_field_bytes(saved->payload, saved->len);
    else
        call_field("-");
    fputc('\n', oracle_calls);

    BUFFER *payload = NULL;
    if(saved) {
        payload = buffer_create(0, NULL);
        buffer_memcat(payload, saved->payload, saved->len);
    }
    BUFFER *result = buffer_create(0, NULL);
    usec_t stop_monotonic_ut = 0;
    bool cancelled = false;
    int code = spec->cb("oracle-echo", id_copy, cmd, name, payload, &stop_monotonic_ut, &cancelled, result,
                        HTTP_ACCESS_ALL, "oracle", spec->data);

    fputs("echoed", oracle_calls);
    call_field(id_copy);
    call_field(dyncfg_id2cmd_one(cmd));
    fprintf(oracle_calls, "\t%d", code);
    call_field_bytes(buffer_tostring(result), buffer_strlen(result));
    fputc('\n', oracle_calls);
    buffer_free(result);
    buffer_free(payload);
    freez(id_copy);
}

// C: src/daemon/dyncfg/dyncfg-inline.c. `dyncfg_add <id> <path> <type> <status> <source type> <source> <commands>`
bool dyncfg_add(const struct dyncfg_add_inline_spec *spec) {
    if(!oracle_calls)
        return true;

    BUFFER *cmds = buffer_create(0, NULL);
    dyncfg_cmds2buffer(spec->cmds, cmds);
    fputs("dyncfg_add", oracle_calls);
    call_field(spec->id);
    call_field(spec->path);
    call_field(dyncfg_id2type(spec->type));
    call_field(dyncfg_id2status(spec->status));
    call_field(dyncfg_id2source_type(spec->source_type));
    call_field(spec->source);
    call_field(buffer_tostring(cmds));
    fputc('\n', oracle_calls);
    buffer_free(cmds);

    // the model of the core, from here on
    if(spec->type == DYNCFG_TYPE_TEMPLATE) {
        for(size_t i = 0; i < oracle.dyncfg_saved_used; i++)
            dyncfg_echo(spec, spec->id, DYNCFG_CMD_ADD, oracle.dyncfg_saved[i].name, &oracle.dyncfg_saved[i]);
        return true;
    }

    char *id = strdupz(spec->id);
    DYNCFG_SOURCE_TYPE source_type = spec->source_type;
    bool disable = dyncfg_user_disabled(id) || spec->status == DYNCFG_STATUS_DISABLED ||
                   dyncfg_user_disabled(DYNCFG_TEMPLATE_ID);
    dyncfg_echo(spec, id, disable ? DYNCFG_CMD_DISABLE : DYNCFG_CMD_ENABLE, NULL, NULL);
    if(source_type != DYNCFG_SOURCE_TYPE_DYNCFG && strncmp(id, DYNCFG_TEMPLATE_ID ":", sizeof(DYNCFG_TEMPLATE_ID)) == 0)
        for(size_t i = 0; i < oracle.dyncfg_saved_used; i++)
            if(strcmp(oracle.dyncfg_saved[i].name, id + sizeof(DYNCFG_TEMPLATE_ID)) == 0)
                dyncfg_echo(spec, id, DYNCFG_CMD_UPDATE, NULL, &oracle.dyncfg_saved[i]);
    freez(id);
    return true;
}

// `dyncfg_del <id>`
void dyncfg_del(RRDHOST *host, const char *id) {
    (void)host;
    if(oracle_calls) {
        fputs("dyncfg_del", oracle_calls);
        call_field(id);
        fputc('\n', oracle_calls);
    }
}

// `dyncfg_status <id> <status>`
void dyncfg_status(RRDHOST *host, const char *id, DYNCFG_STATUS status) {
    (void)host;
    if(oracle_calls) {
        fputs("dyncfg_status", oracle_calls);
        call_field(id);
        call_field(dyncfg_id2status(status));
        fputc('\n', oracle_calls);
    }
}

// C: src/database/sqlite/sqlite_aclk_alert.c: a rule's configuration queued for the Cloud.
// `aclk_send_alert_configuration <hash>`
void aclk_send_alert_configuration(char *config_hash) {
    oracle_call("aclk_send_alert_configuration\t%s", config_hash);
}

// C: sqlite_aclk_alert.c: whether `alert_hash_cloud` has the rule's hash, as the scenario says.
// `alert_hash_has_transitioned <hash> <answer 0|1>`
bool alert_hash_has_transitioned(nd_uuid_t *hash_id) {
    char hash[UUID_STR_LEN];
    uuid_unparse_lower(*hash_id, hash);
    oracle_call("alert_hash_has_transitioned\t%s\t%d", hash, oracle.hash_not_sent ? 0 : 1);
    return !oracle.hash_not_sent;
}

// ------------------------------------------------------------------------------------------------
// SQLite, the metadata queue, ACLK, pulse

// C: src/database/sqlite/sqlite_health.c, sql_health_alarm_log_load(). Without a database it returns at once. With
// one, on an empty table: both maxima are get_uint32_id() when 0; next_log_id = max_unique + 1; next_alarm_id =
// max_alarm + 1 when it is 0 or not above the maximum.
void sql_health_alarm_log_load(RRDHOST *host) {
    oracle_call("load\t%d", oracle.database ? 1 : 0);
    if(oracle.sql_real) {
        c_sql_health_alarm_log_load(host);
        return;
    }
    if(!oracle.database)
        return;

    if(!host->health_max_unique_id)
        host->health_max_unique_id = get_uint32_id();
    if(!host->health_max_alarm_id)
        host->health_max_alarm_id = get_uint32_id();

    host->health_log.next_log_id = host->health_max_unique_id + 1;
    if(!host->health_log.next_alarm_id || host->health_log.next_alarm_id <= host->health_max_alarm_id)
        host->health_log.next_alarm_id = host->health_max_alarm_id + 1;
}

// C: sqlite_health.c, sql_health_alarm_log_save(): insert or update; the insert marks the entry SAVED.
void sql_health_alarm_log_save(RRDHOST *host, ALARM_ENTRY *ae) {
    // a repeat's entry is in no log: this row is all the trace has of it
    char old_value[32], new_value[32];
    oracle_double(old_value, sizeof(old_value), ae->old_value);
    oracle_double(new_value, sizeof(new_value), ae->new_value);
    oracle_call("save\t%u\t%08x\t%u\t%u\t%s\t%s\t%ld\t%ld\t%ld\t%ld\t%d\t%ld\t%s\t%s\t%llu\t%ld\t%d\t%u",
                ae->unique_id, (unsigned)ae->flags, ae->alarm_id, ae->alarm_event_id,
                rrdcalc_status2string(ae->old_status), rrdcalc_status2string(ae->new_status), (long)ae->when,
                (long)ae->delay_up_to_timestamp, (long)ae->duration, (long)ae->non_clear_duration, ae->delay,
                (long)ae->last_repeat, old_value, new_value, oracle_uuid_rank(ae->transition_id),
                (long)ae->exec_run_timestamp, ae->exec_code, ae->updated_by_id);
    oracle.saved++;
    if(oracle.sql_real)
        c_sql_health_alarm_log_save(host, ae);
    else if(oracle.save_sets_saved)
        ae->flags |= HEALTH_ENTRY_FLAG_SAVED;
}

// C: sqlite_health.c, sql_get_alarm_id(): the alarm id and the next event id of (host, chart, name) in the table,
// whatever the rule's hash.
uint32_t sql_get_alarm_id(RRDHOST *host, STRING *chart, STRING *name, uint32_t *next_event_id) {
    if(oracle.sql_real) {
        uint32_t found = c_sql_get_alarm_id(host, chart, name, next_event_id);
        oracle_call("sql_get_alarm_id\t%s\t%s\t%u\t%u", string2str(chart), string2str(name), found,
                    found ? *next_event_id : 0);
        return found;
    }
    uint32_t alarm_id = 0, next = 0;
    for(size_t i = 0; i < oracle.sql_alarms_used; i++)
        if(strcmp(oracle.sql_alarms[i].chart, string2str(chart)) == 0 &&
           strcmp(oracle.sql_alarms[i].name, string2str(name)) == 0) {
            alarm_id = oracle.sql_alarms[i].alarm_id;
            next = oracle.sql_alarms[i].next_event_id;
        }
    oracle_call("sql_get_alarm_id\t%s\t%s\t%u\t%u", string2str(chart), string2str(name), alarm_id, next);
    if(alarm_id)
        *next_event_id = next;
    return alarm_id;
}

// C: sqlite_health.c, sql_health_get_last_executed_event(): the status of the alarm's newest entry whose command
// was run, this entry aside: 1 with the status, 0 without such an entry, -1 when the question failed.
int sql_health_get_last_executed_event(RRDHOST *host, ALARM_ENTRY *ae, RRDCALC_STATUS *last_executed_status) {
    int ret;
    if(oracle.sql_real)
        ret = c_sql_health_get_last_executed_event(host, ae, last_executed_status);
    else {
        ret = oracle.last_executed_ret;
        if(ret == 1)
            *last_executed_status = oracle.last_executed_status;
    }
    oracle.asked++;
    oracle_call("last_executed\t%u\t%d\t%s", ae->unique_id, ret,
                ret == 1 ? rrdcalc_status2string(*last_executed_status) : "-");
    return ret;
}

// C: src/database/sqlite/sqlite_metadata.c, metadata_queue_ae_save(): the host's pending transitions and the entry's
// pending saves go up by one, then the command is queued; a refusal takes both back, and health_alarm_log_save()
// then saves at once on the health thread.
bool metadata_queue_ae_save(RRDHOST *host, ALARM_ENTRY *ae) {
    __atomic_add_fetch(&host->health.pending_transitions, 1, __ATOMIC_RELAXED);
    __atomic_add_fetch(&ae->pending_save_count, 1, __ATOMIC_RELAXED);
    oracle_call("queue\t%u\t%d", ae->unique_id, oracle.queue_accepts ? 1 : 0);
    if(!oracle.queue_accepts || oracle.queued_used == ORACLE_QUEUE_MAX) {
        __atomic_sub_fetch(&host->health.pending_transitions, 1, __ATOMIC_RELAXED);
        __atomic_sub_fetch(&ae->pending_save_count, 1, __ATOMIC_RELAXED);
        return false;
    }
    oracle.queued[oracle.queued_used++] = (struct oracle_queued){ .host = host, .ae = ae };
    return true;
}

// C: sqlite_metadata.c, metadata_queue_ae_deletion(): health_alarm_log_free_one_nochecks_nounlink() hands over an
// entry it cannot free yet; the metadata thread frees it after a store job that leaves it without a pending save.
void metadata_queue_ae_deletion(ALARM_ENTRY *ae) {
    oracle_call("queue_deletion\t%u", ae->unique_id);
    if(oracle.deferred_used == ORACLE_QUEUE_MAX) {
        fprintf(stdout, "too many deferred entries\n");
        exit(1);
    }
    oracle.deferred[oracle.deferred_used++] = ae;
}

// C: sqlite_metadata.c, store_alert_transitions(), the store job's step for the alert log, on a worker of the
// metadata thread: per queued pair, in arrival order, the live entry is saved and both counters are taken back,
// whatever the save did. Then what was handed over for freeing and has no pending save is freed.
void oracle_store(void) {
    bool health_thread = is_health_thread;
    is_health_thread = false;

    size_t queued = oracle.queued_used;
    oracle.queued_used = 0;
    for(size_t i = 0; i < queued; i++) {
        RRDHOST *host = oracle.queued[i].host;
        ALARM_ENTRY *ae = oracle.queued[i].ae;
        sql_health_alarm_log_save(host, ae);
        __atomic_add_fetch(&ae->pending_save_count, -1, __ATOMIC_RELAXED);
        __atomic_add_fetch(&host->health.pending_transitions, -1, __ATOMIC_RELAXED);
    }

    size_t kept = 0;
    for(size_t i = 0; i < oracle.deferred_used; i++) {
        ALARM_ENTRY *ae = oracle.deferred[i];
        if(__atomic_load_n(&ae->pending_save_count, __ATOMIC_RELAXED))
            oracle.deferred[kept++] = ae;
        else
            health_alarm_log_free_one_nochecks_nounlink(ae);
    }
    oracle.deferred_used = kept;

    is_health_thread = health_thread;
}

void commit_alert_transitions(RRDHOST *host) {
    (void)host;
    oracle_call("commit_alert_transitions");
}

// C: src/database/sqlite/sqlite_aclk_alert.c. Called at the end of a pass without pending transitions.
bool process_alert_pending_queue(RRDHOST *host) {
    (void)host;
    oracle_call("process_alert_pending_queue");
    return false;
}

// C: src/streaming/protocol/command-host-variables.c. rrdvar.c sends a changed host variable to the parent.
void stream_sender_send_this_host_variable_now(RRDHOST *host, const RRDVAR_ACQUIRED *rva) {
    (void)host;
    (void)rva;
}

void pulse_aral_register(ARAL *ar, const char *name) {
    (void)ar;
    (void)name;
}

// C: src/daemon/pulse/pulse-daemon-memory.c: the counters the notification's buffers are accounted on
struct netdata_buffers_statistics netdata_buffers_statistics = { 0 };

// ------------------------------------------------------------------------------------------------
// a notification's command: C's health_send_notification() and health_alarm_wait_for_execution() run over these

struct oracle_popen {
    pid_t pid;
    enum oracle_exec_kind kind;
    size_t slices;
    int code;
};

// The n-th argument of a command line as prepare_command() writes it (`exec 'a0' 'a1' ...`, a quote inside an
// argument as the four bytes '\''): false when the line has no such argument.
static bool command_argument(const char *cmd, size_t index, char *dst, size_t size) {
    const char *s = strchr(cmd, ' ');
    for(size_t i = 0; s && s[0] == ' ' && s[1] == '\''; i++) {
        s += 2;
        size_t n = 0;
        while(*s) {
            bool quote = strncmp(s, "'\\''", 4) == 0;
            if(*s == '\'' && !quote)
                break;
            if(i == index && n + 1 < size)
                dst[n++] = *s;
            s += quote ? 4 : 1;
        }
        if(*s != '\'')
            return false;
        s++;
        if(i == index) {
            dst[n] = '\0';
            return true;
        }
    }
    return false;
}

// the `spawn` row: the command line holds backslashes and any byte above the controls, so it is written as a field
static void spawn_call(pid_t pid, const char *cmd) {
    if(!oracle_calls)
        return;
    fprintf(oracle_calls, "spawn\t%d\t", (int)pid);
    oracle_esc(oracle_calls, cmd);
    fputc('\n', oracle_calls);
}

// C: src/libnetdata/spawn_server/spawn_popen.c, spawn_popen_run(): `/bin/sh -c <cmd>` through the main spawn
// server; NULL when the spawn fails.
POPEN_INSTANCE *oracle_spawn_popen_run(const char *cmd) {
    // the alert's name and the new status are the command's arguments 7 and 9
    char alert[128] = "", status[32] = "";
    command_argument(cmd, 7, alert, sizeof(alert));
    command_argument(cmd, 9, status, sizeof(status));

    struct oracle_exec_rule does = { .kind = ORACLE_EXEC_EXIT };
    for(size_t i = 0; i < oracle.exec_rules_used; i++) {
        struct oracle_exec_rule *rule = &oracle.exec_rules[i];
        if((strcmp(rule->alert, "*") == 0 || strcmp(rule->alert, alert) == 0) &&
           (strcmp(rule->status, "*") == 0 || strcmp(rule->status, status) == 0))
            does = *rule;
    }

    oracle.spawned++;
    if(does.kind == ORACLE_EXEC_FAIL) {
        spawn_call(0, cmd);
        return NULL;
    }
    struct oracle_popen *pi = callocz(1, sizeof(*pi));
    pi->pid = 1001 + oracle.pids++;
    pi->kind = does.kind;
    pi->slices = does.slices;
    pi->code = does.code;
    oracle.commands_running++;
    spawn_call(pi->pid, cmd);
    return (POPEN_INSTANCE *)pi;
}

// C: spawn_popen.c, spawn_popen_timedwait(): EXITED frees the instance and gives the code; RUNNING after the
// timeout (the wait on the status channel timed out: ETIMEDOUT); ERROR when the wait itself broke.
SPAWN_TIMEDWAIT_RESULT oracle_spawn_popen_timedwait(POPEN_INSTANCE *instance, int timeout_ms, int *code) {
    struct oracle_popen *pi = (struct oracle_popen *)instance;
    if(pi->kind == ORACLE_EXEC_HANG || pi->slices) {
        if(pi->slices)
            pi->slices--;
        oracle.monotonic_usec += (usec_t)timeout_ms * USEC_PER_MS;
        oracle_call("timedwait\t%d\t%d\trunning", (int)pi->pid, timeout_ms);
        errno = ETIMEDOUT;
        return SPAWN_TIMEDWAIT_RUNNING;
    }
    if(pi->kind == ORACLE_EXEC_ERROR) {
        oracle_call("timedwait\t%d\t%d\terror", (int)pi->pid, timeout_ms);
        errno = 0;
        return SPAWN_TIMEDWAIT_ERROR;
    }
    oracle_call("timedwait\t%d\t%d\texited\t%d", (int)pi->pid, timeout_ms, pi->code);
    *code = pi->code;
    freez(pi);
    oracle.commands_running--;
    return SPAWN_TIMEDWAIT_EXITED;
}

// C: spawn_popen.c, spawn_popen_kill(): the command is killed and reaped, the instance freed.
int oracle_spawn_popen_kill(POPEN_INSTANCE *instance, int timeout_ms) {
    struct oracle_popen *pi = (struct oracle_popen *)instance;
    oracle_call("kill\t%d\t%d", (int)pi->pid, timeout_ms);
    freez(pi);
    oracle.commands_running--;
    return -1;
}

pid_t oracle_spawn_popen_pid(POPEN_INSTANCE *instance) {
    return ((struct oracle_popen *)instance)->pid;
}

usec_t oracle_now_monotonic_usec(void) {
    oracle_call("monotonic");
    return oracle.monotonic_usec;
}

// ------------------------------------------------------------------------------------------------
// an entry's transition id

void oracle_uuid_generate_random(nd_uuid_t out) {
    uint64_t n = ++oracle.uuids;
    memset(out, 0, sizeof(nd_uuid_t));
    for(size_t i = 0; i < 8; i++)
        ((unsigned char *)out)[15 - i] = (unsigned char)(n >> (8 * i));
}

unsigned long long oracle_uuid_rank(const nd_uuid_t id) {
    unsigned long long n = 0;
    for(size_t i = 8; i < 16; i++)
        n = (n << 8) | ((const unsigned char *)id)[i];
    return n;
}

// ------------------------------------------------------------------------------------------------
// the clock

int clock_gettime(clockid_t clk_id, struct timespec *ts) {
    if(clk_id == CLOCK_REALTIME && oracle.clock_s) {
        ts->tv_sec = oracle.clock_s;
        ts->tv_nsec = (long)(oracle.clock_usec * 1000);
        return 0;
    }
    return (int)syscall(SYS_clock_gettime, clk_id, ts);
}
