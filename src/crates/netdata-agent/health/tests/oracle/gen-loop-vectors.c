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
//       edit.tsv     <a rule's source text> <the edit command>: health_edit_command_from_source()
//       sanitize.tsv <an argument's bytes> <fits 0|1> <the sanitized bytes>: sanitize_command_argument_string()
//                    into the 8192-byte buffer prepare_command() gives it
//
//   gen-loop-vectors decide <directory> <a file for the records>
//       decide.tsv   C's health_send_notification() over one hand-built entry per combination of new status, old
//                    status, the flags NO_CLEAR_NOTIFICATION, SILENCED, RUN_ONCE and IS_REPEATING, and what the
//                    table answers about the alarm's last executed event: <new status> <old status> <flags
//                    before> <the answer: fail, none or a status> <the table was asked 0|1> <a command was
//                    spawned 0|1> <flags after> <exec_run_timestamp set 0|1> <saves> <the record's message, `-`
//                    for none>
//
//   gen-loop-vectors silencers <directory> <a file for the records>
//       C's own health_silencers.c. Every case runs in a child process, so that a crash of C is a row too: the
//       case's first field (the file's bytes, the sequence, the state), then `signal <n>`.
//       silencers-file.tsv   <the file's bytes> <the state after health_silencers_init(), as health_silencers2json()
//                            prints it> <the records' messages>
//       manage.tsv           <sequence> <n> <the token: ok (the management key), bad (another text), - (none)> <the
//                            query> <the reply's code> <its content type is JSON 0|1> <its body> <the file is
//                            there 0|1> <the file's bytes> <the records' messages>: the n-th request of a
//                            sequence through web_client_api_request_v1_mgmt_health(). The file is removed before
//                            each request, so a file is what that request wrote; in the sequence `unwritable` the
//                            file's path leads through a regular file
//       silencers-match.tsv  <the requests that made the state, joined by ` ; `> <the alert's name> <its chart>
//                            <its chart's context, \x00 for an alert without a chart> <the hostname> <run flags
//                            before> <what health_silencers_check_silenced() answers: None, DISABLE or SILENCE>
//                            <what health_silencers_update_disabled_silenced() returns> <run flags after> <the
//                            records' messages>
//
//   gen-loop-vectors dyncfg <directory> <a file for the records>
//       C's own health_dyncfg.c over the case lists of tests/corpus/dyncfg/ (a case is a line that is no comment,
//       counted from 0; an empty line is the empty payload). Every case runs in a child process: `<n>`, then
//       `signal <n>`, is a case that killed C.
//       payload.tsv     payloads.txt through health_prototype_payload_parse() with the name `p_name`, in both
//                       modes: <n> <required|optional> parse <rules, 0 when it is refused> <the error text> <the
//                       chain's JSON as it is hashed, `-` when refused> <the records' messages>, then per rule
//                       <n> <mode> rule <its place> and the rule's fields as rules.tsv has them, up to the hash
//       userconfig.tsv  userconfig.txt through dyncfg_health_cb(): <n> <template|job: the node asked> <localhost's
//                       default command is set 0|1> <the answer: code, content type, cache, expiry, body, as an
//                       `answer` row has them> <the records' messages>. The job is `d_tpl` of base.conf, the name
//                       given to the template `u_name`
//       actions.tsv     actions.txt through dyncfg_health_cb(), each on the same store (base.conf, then the jobs
//                       `d_dyn` of one.json and `d_off` of off.json added) with a host whose health never ran:
//                       <n> <the answer> <the records' messages> <the calls, as `call` rows have them, joined by
//                       ` | `> <the store after it: name:enabled:rules per name>
//
//   gen-loop-vectors scenario <loop.tsv to append to> <scenario file> <a file for the records>
//       One process per scenario. Rows are <scenario> <step> <kind> ..., a step being one `pass`, `unlink`,
//       `apply`, `store`, `restart`, `cleanup`, `alarm-log`, `configs`, `transitions`, `configurations`, `wait`,
//       `load`, `manage`, `dyncfg`, `register`, `unregister`, `disconnect`, `reload` or `badge` directive:
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
//         list       a `load` step: the silencers' state as health_silencers2json() prints it
//         reply      a `manage` step: the reply's code, whether its content type is JSON, its body
//         badge      a `badge` step: what C's api_v1_badge() left on the web client: the code, the content type
//                    (svg, text), whether the body may be cached (c) or may not (n), the date and the expiry as
//                    seconds after the clock (`-`: not set), the header lines it added, the body
//         file       a `manage` step: whether the silencers' file is there, and its bytes
//         answer     a `dyncfg` step: what C's dyncfg_health_cb() answered: the code, the content type, whether the
//                    body may be cached (c), may not (n) or neither was said (-), the expiry as seconds after the
//                    clock (`-`: none), the body
//         stored     a `dyncfg`, `register` or `reload` step: how many names the rules' store holds, and the names in the
//                    store's order, separated by spaces (`-` for none)
//         store      then per name that is new or differs from its last row: the name, whether it is enabled, how
//                    many rules its chain has, and the chain as health_prototype_to_json() prints it for a GET
//         transition     a `transitions` step: per entry C's sql_alert_transitions() handed its callback, in
//                        C's order, the entry as the callback gets it, in the statement's column order: host_id,
//                        alarm_id (C's 32 bits), config_hash_id, alert_name, chart, chart_name, family, recipient,
//                        units, exec, chart_context, when_key, duration, non_clear_duration, flags (16 hex
//                        digits), delay_up_to_timestamp, info, exec_code, new_status, old_status, delay, new_value,
//                        old_value, last_repeat, transition_id, global_id (unsigned), classification, type,
//                        component, exec_run_timestamp, summary. An id is 32 hex digits, a text C's string
//                        (`\x00`: NULL), a value a double
//         transitions    then how many entries the callback got (a row C skips is in its record alone)
//         configuration  a `configurations` step: per rule C's sql_get_alert_configuration() handed its callback,
//                        in C's order, the rule as the callback gets it, in the statement's column order: the hash
//                        (32 hex digits), alarm, template, on_key, class, component, type, lookup, every, units,
//                        calc, families, green, red, warn, crit, exec, to_key, info, delay, options, repeat,
//                        host_labels, p_db_lookup_dimensions, p_db_lookup_method, p_db_lookup_options (unsigned),
//                        p_db_lookup_after, p_db_lookup_before, p_update_every, source, chart_labels, summary,
//                        time_group_condition, time_group_value (a double), dims_group, data_source
//         configurations then what the function returned: how many rules it handed out, -1 when it failed
// The silencers' file is a path beside the records; every row names it `{file}`.
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
//   lookup <chart> <code> <value|nan> <null 0|1> [absolute]     the last word: a 200's window was absolute (a
//                                      caller that gave its buffer, the badge, may then have it cached)
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
//   cleanup                            the hourly cleanup of the host: sql_health_alarm_log_cleanup(), the
//                                      daemon's one call (health_alarm_log_cleanup() alone without a database)
//   alarm-log <after> [chart]          /api/v1/alarm_log's body (`after` is a unique id, as the request gives it)
//   badge <query>                      /api/v1/badge.svg with that decoded query (the rest of the line), through
//                                      C's own api_v1_badge(); a chart value comes from the scripted lookup
//   configs                            /api/v2/alert_config's body for every rule of alert_hash, in the table's
//                                      order, then for a hash no rule has: a `config` row each, with the hash,
//                                      how many rules C's query found, and the body (none when it found none)
//   transitions window <after> <before> <context|-> <alert|-> [host ...]
//                                      C's sql_alert_transitions() as /api/v2/alert_transitions calls it without
//                                      a `transition`: the hosts in scope (their GUIDs, set into the dictionary in
//                                      this order), the window's two ends in seconds (offsets from T0, 0 stays 0;
//                                      `=<n>` is the number itself), the request's `contexts` text and its `alert`
//                                      text (`-`: none)
//   transitions id <text>              the same with the request's `transition` text (the rest of the line): C's
//                                      direct statement, over a dictionary without a host
//   configurations [hash ...]          C's sql_get_alert_configuration() as `options=config` calls it: the rules'
//                                      hashes (UUID texts, set into the dictionary in this order as their
//                                      lower-case texts: the same hash twice is one item, as in C's caller)
//   exec <alert|*> <status|*> exit <slices> <code> | fail | error <slices> | hang
//                                      what the notification command of that alert's entries with that new status
//                                      does: it exits with the code after that many slices of the wait, the spawn
//                                      fails, the wait breaks after that many slices, or it never exits. The last
//                                      matching line decides; without one a command exits with 0 at the first slice
//   timeout <seconds>                  `[health] notification execution timeout` (C's default is 120; 0: no limit)
//   last-executed none|fail|<STATUS>   without a real database: what the table answers about an alarm's last
//                                      executed event (default: none)
//   use-summary <0|1>                  `[health] use summary for notifications`: read by a host's first pass
//   default-exec <text|->              `[health] script to execute on alarm` (`-`: none): read by a host's first
//                                      pass, and by the rules read after it
//   auto-wait <0|1>                    a `pass` ends with the wait for the notifications in flight, as an iteration
//                                      of the daemon's loop does after its hosts (default 1)
//   wait                               that wait alone: wait_for_all_notifications_to_finish_before_allowing_
//                                      health_to_be_cleaned_up()
//   running-for <n>                    the service runs for n more looks at it, then it is stopping
//   exiting                            the agent's exit has begun (it cannot be undone)
//   delay-up-to <second|0>             the host's health is postponed until then (what a connecting child gets)
//   silencers-file <text|->            the silencers' file holds the rest of the line (`-`: there is no file)
//   load                               health_silencers_init(): the file's read at health's start
//   manage <ok|bad|-> [query]          a request to /api/v1/manage/health with the management key, another text
//                                      or no token: web_client_api_request_v1_mgmt_health() over the decoded query
//   dyncfg <id> <action> <name|-> <payload|->     C's dyncfg_health_cb(), as the configuration core calls it for a
//                                      user's request; the payload is the rest of the line, `@<path>` for a
//                                      file's bytes, or `=<id>` for what a GET of that node answers. It runs off
//                                      the HEALTH thread, as in the daemon; the hosts' index (the one host) is made
//                                      at a scenario's first such step
//   register                           health_dyncfg_register_all_prototypes() as health_reload_prototypes() calls
//                                      it, `registering` on: the calls to the core and what the model of the core
//                                      in health-loop-stubs.c sends back; off the HEALTH thread
//   unregister                         health_dyncfg_unregister_all_prototypes()
//   dyncfg-saved <name> <payload>      a file of the core's holds this job of health's template, with this payload
//                                      (as for `dyncfg`): the model replays it at the template's registration
//   dyncfg-user-disabled <id>          a file of the core's says the user disabled this node
//   cloud-has <0|1>                    whether alert_hash_cloud has a rule's hash (default 1: nothing is pushed)
//   enabled-alarms <pattern>           `[health] enabled alarms` (default `*`)
//   dyncfg-forget <name|id>            the core's files no longer hold that job's payload (a name), or that node
//                                      as disabled by the user (an id): what a user's remove or enable leaves
//   health-dirs <user dir> <stock dir|->   the two health.d trees a `reload` reads (relative to the crate's
//                                      directory); `-`: `[health] enable stock health configuration` is off
//   health-enabled <0|1>               the host's health is enabled (a child's `health enabled` of stream.conf, or
//                                      a host a detach turned it off for)
//   disconnect                         a streaming child's detach as stream-receiver.c makes it, off the HEALTH
//                                      thread: the host may no longer run health (the gate), then
//                                      rrdcalc_child_disconnected(), then its health is not enabled. A reconnect
//                                      is `health-enabled 1`, `gate 1` and, for a postponement, `delay-up-to`
//   reload                             health_plugin_reload()'s two calls, off the HEALTH thread:
//                                      health_reload_prototypes() over the two trees, with the registration and
//                                      what the model of the core sends back, then
//                                      health_apply_prototypes_to_all_hosts() over the hosts' index (the one host)
//   pending host-init|host-recheck|chart-init <chart>|chart-recheck <chart>
//   clock <second> [microseconds]
//   pass <now> [hibernate]             one call of the pass
//   unlink <chart>                     rrdcalc_unlink_and_delete_all_rrdset_alerts()
//   apply                              health_apply_prototypes_to_host()
// `unlink`, `apply`, `store`, `restart`, `cleanup`, `alarm-log`, `configs`, `transitions`, `configurations`, `wait`,
// `load`, `manage`, `dyncfg`, `register`, `unregister`, `disconnect` and `reload` are steps too.
//
// Field encoding: see health-oracle.h. A double is `nan` or its bits in hex.

#include "health-loop-oracle.h"
#include "health/rrdvar.h"
#include "database/sqlite/sqlite_functions.h"
#include "database/sqlite/sqlite_health.h"
#include "database/contexts/api_v2_contexts_alerts.h"
#include "web/server/web_client.h"
#include <sys/resource.h>
#include <sys/wait.h>

// the management key (api_v1_manage.c's in the daemon, the stubs' here)
extern char *api_secret;

// health_silencers.c defines it and no header declares it
void health_silencers2json(BUFFER *wb);

char *format_value_and_unit(char *value_string, size_t value_string_len, NETDATA_DOUBLE value, const char *units, int precision);

#define T0 2000000000L

static void die(const char *what, const char *detail) {
    fprintf(stdout, "%s: %s\n", what, detail ? detail : "");
    exit(1);
}

// the values of units.tsv and of badge-format.tsv
static const NETDATA_DOUBLE unit_values[] = {
    0.0, -0.0, NAN, INFINITY, -INFINITY,
    0.00001, 0.0001, 0.00012345, 0.001, 0.0099, 0.01, 0.0999, 0.1, 0.4, 0.5, 0.6, 0.99, 0.999, 0.9999, 1.0,
    1.4, 1.5, 1.23456789, 9.99, 9.994, 9.995, 9.9999, 10.0, 10.5, 59.0, 59.5, 60.0, 61.0, 99.4, 99.5, 99.94,
    99.95, 99.99, 100.0, 100.5, 119.0, 120.0, 125.0, 999.4, 999.5, 999.94, 999.95, 999.99, 1000.0, 1000.5,
    1439.0, 1440.0, 1441.0, 3599.0, 3600.0, 3601.0, 3700.0, 86399.0, 86400.0, 86401.0, 90061.0, 172800.0,
    172801.0, 1e6, 1.5e6, 1e9, 1e12, 1e15, 1e18, 1e20, 18446744073709551615.0, 1e300,
    -0.00012345, -0.5, -1.0, -1.5, -9.995, -59.0, -60.0, -99.95, -999.95, -1000.0, -3600.0, -86400.0, -90061.0,
    -1e9, -1e18,
};

static void put_double(FILE *f, NETDATA_DOUBLE v) {
    char text[32];
    oracle_double(text, sizeof(text), v);
    fputs(text, f);
}

// ------------------------------------------------------------------------------------------------
// the tables

static void sanitize_row(FILE *f, const char *argument) {
    char buf[8192];
    bool fits = sanitize_command_argument_string(buf, argument, sizeof(buf) - 1);
    oracle_esc(f, argument);
    fprintf(f, "\t%d\t", fits ? 1 : 0);
    if(fits)
        oracle_esc(f, buf);
    else
        fputc('-', f);
    fputc('\n', f);
}

// sqlite_health.c's, declared by no header
int calculate_delay(RRDCALC_STATUS old_status, RRDCALC_STATUS new_status);

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
    for(size_t u = 0; u < sizeof(units) / sizeof(units[0]); u++)
        for(size_t v = 0; v < sizeof(unit_values) / sizeof(unit_values[0]); v++) {
            char buf[100 + 1];
            char *text = format_value_and_unit(buf, 100, unit_values[v], units[u], -1);
            put_double(f, unit_values[v]);
            fputc('\t', f);
            oracle_esc(f, units[u]);
            fputc('\t', f);
            oracle_esc(f, text);
            fputc('\n', f);
        }
    if(ferror(f) || fclose(f) != 0) die("cannot write", path);

    // health_edit_command_from_source(): the command an alert log entry and a notification carry, from a rule's
    // source text, with the user configuration directory and localhost's registry hostname
    snprintf(path, sizeof(path), "%s/edit.tsv", dir);
    f = fopen(path, "w");
    if(!f) die("cannot create", path);
    fprintf(f, "# generated by tests/oracle/gen-loop-vectors.c from the C implementation; do not edit\n");
    fprintf(f, "# columns: a rule's source text, the edit command\n");
    static RRDHOST edit_host;
    edit_host.registry_hostname = string_strdupz("registry-host");
    localhost = &edit_host;
    netdata_configured_user_config_dir = "/oracle/etc";
    static const char *sources[] = {
        "line=16,file=/etc/netdata/health.d/cpu.conf", "line=16,file=tests/corpus/x.conf",
        "12@/etc/netdata/health.d/ram.conf", "12@relative.conf", "a/b@c", "line=7,file=/a/b@c.conf", "1@/a/b@c",
        "line=7file=/x/y.conf", "file=/x/y.conf,line=9", "file=/x/y.conf,line=9,more", "line=,file=/z.conf",
        "line=3,file=/", "line=5,file=/a/b/", "xline=5,file=/a.conf", "line=1,line=2,file=/a/c.conf",
        "line=1,file=/a.conf,file=/b/d.conf", "", "@", "/@", "line=4", "file=/only.conf", "plain text",
        "line=2,file=/dir with spaces/and=signs.conf", "0@/health.d/x.conf", "line=18446744073709551615,file=/big.conf",
    };
    for(size_t i = 0; i < sizeof(sources) / sizeof(sources[0]); i++) {
        char *command = health_edit_command_from_source(sources[i]);
        oracle_esc(f, sources[i]);
        fputc('\t', f);
        oracle_esc(f, command);
        fputc('\n', f);
        freez(command);
    }
    localhost = NULL;
    if(ferror(f) || fclose(f) != 0) die("cannot write", path);

    // sanitize_command_argument_string(), as prepare_command() calls it for each text argument of a notification's
    // command: into a buffer of 8192 bytes, of which it is given 8191
    snprintf(path, sizeof(path), "%s/sanitize.tsv", dir);
    f = fopen(path, "w");
    if(!f) die("cannot create", path);
    fprintf(f, "# generated by tests/oracle/gen-loop-vectors.c from the C implementation; do not edit\n");
    fprintf(f, "# columns: the argument, whether it fits (0|1), the sanitized argument (`-` when it does not fit)\n");
    // every byte between two letters
    for(int byte = 1; byte < 256; byte++) {
        char argument[4] = { 'a', (char)byte, 'z', '\0' };
        sanitize_row(f, argument);
    }
    static const char *arguments[] = {
        "", "-", "--", "---x", "-a-b", "a-", "- x", "--'--", "-$-", "'", "`", "''", "``", "it's", "a`b`c", "$HOME",
        "$(x)", "${x}", "a$", "tab\there", "line\nbreak", "\r", "\x7f", "\x01\x02\x1f", "a b", "\"quoted\"",
        "back\\slash", "semi;colon", "pipe|amp&", "/usr/libexec/netdata/plugins.d/alarm-notify.sh", "sysadmin root",
        "caf\xc3\xa9", "\xff\xfe",
    };
    for(size_t i = 0; i < sizeof(arguments) / sizeof(arguments[0]); i++)
        sanitize_row(f, arguments[i]);
    // around the buffer's end: plain bytes, then a byte that takes one or four bytes as the last one
    static const size_t lengths[] = { 8185, 8186, 8187, 8188, 8189, 8190, 8191, 8192 };
    static const char *tails[] = { "", "$", "'", "`", "''", "-" };
    for(size_t l = 0; l < sizeof(lengths) / sizeof(lengths[0]); l++)
        for(size_t t = 0; t < sizeof(tails) / sizeof(tails[0]); t++) {
            char *argument = mallocz(lengths[l] + strlen(tails[t]) + 1);
            memset(argument, 'x', lengths[l]);
            strcpy(argument + lengths[l], tails[t]);
            sanitize_row(f, argument);
            freez(argument);
        }
    // leading dashes do not count
    {
        char *argument = mallocz(9000 + 2);
        memset(argument, '-', 9000);
        strcpy(argument + 9000, "x");
        sanitize_row(f, argument);
        freez(argument);
    }
    if(ferror(f) || fclose(f) != 0) die("cannot write", path);

    // calculate_delay() of sqlite_health.c: how long the Cloud's queue waits on a status change, by the status left
    // and the one taken (REMOVED -2 to CRITICAL 4, and a number past them)
    snprintf(path, sizeof(path), "%s/aclk-delay.tsv", dir);
    f = fopen(path, "w");
    if(!f) die("cannot create", path);
    fprintf(f, "# generated by tests/oracle/gen-loop-vectors.c from the C implementation; do not edit\n");
    fprintf(f, "# columns: the status left, the status taken, the delay in seconds\n");
    static const int statuses[] = { -2, -1, 0, 1, 2, 3, 4, 9 };
    for(size_t from = 0; from < sizeof(statuses) / sizeof(statuses[0]); from++)
        for(size_t to = 0; to < sizeof(statuses) / sizeof(statuses[0]); to++)
            fprintf(f, "%d\t%d\t%d\n", statuses[from], statuses[to],
                    calculate_delay((RRDCALC_STATUS)statuses[from], (RRDCALC_STATUS)statuses[to]));
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

// the silencers' file: a path beside the records, named `{file}` in every row
static char silencers_path[4096];

static void name_file(char *line) {
    size_t len = strlen(silencers_path);
    if(len < 6)
        return;
    for(char *at = strstr(line, silencers_path); at; at = strstr(at, silencers_path)) {
        memcpy(at, "{file}", 6);
        memmove(at + 6, at + len, strlen(at + len) + 1);
        at += 6;
    }
}

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

// What contexts_v2_alert_config_to_json() (database/contexts/api_v2_contexts_alert_config.c) does for one hash,
// without its web client: C's query and C's JSON callback into a buffer. The endpoint answers 200 with the body
// when a rule was found, and a text otherwise: the row has no body then.
static void config_row(const char *hash) {
    BUFFER *wb = buffer_create(0, NULL);
    struct alert_transitions_callback_data data = { .wb = wb, .debug = false, .only_one_config = false };
    DICTIONARY *configs = dictionary_create(DICT_OPTION_SINGLE_THREADED | DICT_OPTION_DONT_OVERWRITE_VALUE);
    dictionary_set(configs, hash, NULL, 0);
    buffer_json_initialize(wb, "\"", "\"", 0, true, BUFFER_JSON_OPTIONS_DEFAULT);
    int added = sql_get_alert_configuration(
        configs, contexts_v2_alert_config_to_json_from_sql_alert_config_data, &data, false);
    buffer_json_finalize(wb);
    dictionary_destroy(configs);

    row("config");
    field(out, hash);
    fprintf(out, "\t%d", added);
    field(out, added > 0 ? buffer_tostring(wb) : "");
    fputc('\n', out);
    buffer_free(wb);
}

static void configs(const char *whole) {
    if(!db_meta) die("no real database", whole);
    sqlite3_stmt *stmt = NULL;
    if(sqlite3_prepare_v2(db_meta, "SELECT hash_id FROM alert_hash ORDER BY rowid", -1, &stmt, NULL) != SQLITE_OK)
        die("cannot read alert_hash", sqlite3_errmsg(db_meta));
    size_t used = 0, size = 0;
    char (*hashes)[UUID_STR_LEN] = NULL;
    while(sqlite3_step(stmt) == SQLITE_ROW) {
        if(sqlite3_column_bytes(stmt, 0) != (int)sizeof(nd_uuid_t)) die("a hash that is no UUID", whole);
        if(used == size) {
            size = size ? size * 2 : 64;
            hashes = reallocz(hashes, size * sizeof(*hashes));
        }
        uuid_unparse_lower(*(const nd_uuid_t *)sqlite3_column_blob(stmt, 0), hashes[used++]);
    }
    sqlite3_finalize(stmt);
    for(size_t i = 0; i < used; i++)
        config_row(hashes[i]);
    freez(hashes);
    config_row("5a1e0000-0000-4000-8000-00000000dead");
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
        name_file(line);
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
    // C frees a log entry whose command runs without taking it off the list of running notifications, and its
    // next wait then walks freed memory: a scenario must wait first
    if(oracle.commands_running) die("a restart with notifications in flight", NULL);
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

// a `pass` ends with the wait for the notifications in flight
static bool auto_wait = true;

// the status a name stands for, as rrdcalc_status2string() names them
static bool status_of(const char *name, RRDCALC_STATUS *status) {
    for(int s = RRDCALC_STATUS_REMOVED; s <= RRDCALC_STATUS_CRITICAL; s++)
        if(strcmp(rrdcalc_status2string((RRDCALC_STATUS)s), name) == 0) {
            *status = (RRDCALC_STATUS)s;
            return true;
        }
    return false;
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

// ------------------------------------------------------------------------------------------------
// the SQL half of /api/v2/alert_transitions: C's own sql_alert_transitions() and sql_get_alert_configuration()
// (database/sqlite/sqlite_health.c), each with a callback that writes what C hands it. Of the daemon they take
// only a dictionary whose item names are UUID texts; `debug` is read by neither.

static void field_uuid(FILE *f, const nd_uuid_t *id) {
    fputc('\t', f);
    for(size_t i = 0; i < sizeof(nd_uuid_t); i++)
        fprintf(f, "%02x", (unsigned)(*id)[i]);
}

static void transition_row(struct sql_alert_transition_data *t, void *data) {
    (*(size_t *)data)++;
    row("transition");
    field_uuid(out, t->host_id);
    fprintf(out, "\t%u", t->alarm_id);
    field_uuid(out, t->config_hash_id);
    field(out, t->alert_name);
    field(out, t->chart);
    field(out, t->chart_name);
    field(out, t->family);
    field(out, t->recipient);
    field(out, t->units);
    field(out, t->exec);
    field(out, t->chart_context);
    fprintf(out, "\t%lld\t%lld\t%lld\t%016llx\t%lld", (long long)t->when_key, (long long)t->duration,
            (long long)t->non_clear_duration, (unsigned long long)t->flags, (long long)t->delay_up_to_timestamp);
    field(out, t->info);
    fprintf(out, "\t%d\t%d\t%d\t%d", t->exec_code, t->new_status, t->old_status, t->delay);
    field_double(out, t->new_value);
    field_double(out, t->old_value);
    fprintf(out, "\t%lld", (long long)t->last_repeat);
    field_uuid(out, t->transition_id);
    fprintf(out, "\t%llu", (unsigned long long)t->global_id);
    field(out, t->classification);
    field(out, t->type);
    field(out, t->component);
    fprintf(out, "\t%lld", (long long)t->exec_run_timestamp);
    field(out, t->summary);
    fputc('\n', out);
}

// a window's end: an offset from T0 (0 stays 0), or `=<n>` for the number itself
static time_t window_second(const char *text) {
    return (*text == '=') ? (time_t)strtoll(text + 1, NULL, 10) : second(text);
}

// a UUID text of a scenario as C's callers hand one on: uuid_unparse_lower() of the id
static void dictionary_set_uuid(DICTIONARY *dict, const char *text, const char *whole) {
    nd_uuid_t id;
    char lower[UUID_STR_LEN];
    if(uuid_parse(text, id) != 0) die("a text that is no UUID", whole);
    uuid_unparse_lower(id, lower);
    dictionary_set(dict, lower, NULL, 0);
}

static void transitions(char *rest, const char *whole) {
    if(!db_meta) die("no real database", whole);
    // contexts_v2_alert_transitions_to_json() hands on the request's node dictionary: the function reads its
    // item names alone
    DICTIONARY *nodes = dictionary_create(DICT_OPTION_SINGLE_THREADED | DICT_OPTION_DONT_OVERWRITE_VALUE);
    size_t handed = 0;
    char *how = word(&rest, whole);
    if(strcmp(how, "id") == 0) {
        if(!rest || !*rest) die("a missing argument", whole);
        sql_alert_transitions(nodes, 0, 0, NULL, NULL, rest, transition_row, &handed, false);
    }
    else if(strcmp(how, "window") == 0) {
        time_t after = window_second(word(&rest, whole));
        time_t before = window_second(word(&rest, whole));
        char *context = word(&rest, whole);
        char *alert = word(&rest, whole);
        while(rest && *rest)
            dictionary_set_uuid(nodes, word(&rest, whole), whole);
        sql_alert_transitions(nodes, after, before, strcmp(context, "-") == 0 ? NULL : context,
                              strcmp(alert, "-") == 0 ? NULL : alert, NULL, transition_row, &handed, false);
    }
    else
        die("unknown transitions", whole);
    dictionary_destroy(nodes);

    row("transitions");
    fprintf(out, "\t%zu\n", handed);
}

static void configuration_row(struct sql_alert_config_data *c, void *data) {
    (*(size_t *)data)++;
    row("configuration");
    field_uuid(out, c->config_hash_id);
    field(out, c->name);
    field(out, c->selectors.on_template);
    field(out, c->selectors.on_key);
    field(out, c->classification);
    field(out, c->component);
    field(out, c->type);
    field(out, c->value.db.lookup);
    field(out, c->value.every);
    field(out, c->value.units);
    field(out, c->value.calc);
    field(out, c->selectors.families);
    field(out, c->status.green);
    field(out, c->status.red);
    field(out, c->status.warn);
    field(out, c->status.crit);
    field(out, c->notification.exec);
    field(out, c->notification.to_key);
    field(out, c->info);
    field(out, c->notification.delay);
    field(out, c->notification.options);
    field(out, c->notification.repeat);
    field(out, c->selectors.host_labels);
    field(out, c->value.db.dimensions);
    field(out, c->value.db.method);
    fprintf(out, "\t%u\t%d\t%d\t%d", (unsigned)c->value.db.options, (int)c->value.db.after, (int)c->value.db.before,
            (int)c->value.update_every);
    field(out, c->source);
    field(out, c->selectors.chart_labels);
    field(out, c->summary);
    fprintf(out, "\t%d", (int)c->value.db.time_group_condition);
    field_double(out, c->value.db.time_group_value);
    fprintf(out, "\t%d\t%d\n", (int)c->value.db.dims_group, (int)c->value.db.data_source);
}

static void configurations(char *rest, const char *whole) {
    if(!db_meta) die("no real database", whole);
    // the dictionary contexts_v2_alert_transitions_to_json() makes of the kept entries' hashes
    DICTIONARY *hashes = dictionary_create(DICT_OPTION_SINGLE_THREADED | DICT_OPTION_DONT_OVERWRITE_VALUE);
    while(rest && *rest)
        dictionary_set_uuid(hashes, word(&rest, whole), whole);
    size_t handed = 0;
    int added = sql_get_alert_configuration(hashes, configuration_row, &handed, false);
    dictionary_destroy(hashes);
    if(added >= 0 && (size_t)added != handed) die("a count that is not the callbacks'", whole);

    row("configurations");
    fprintf(out, "\t%d\n", added);
}

// ------------------------------------------------------------------------------------------------
// the silencers: C's own health_silencers.c

static void file_write(const void *bytes, size_t len) {
    FILE *to = fopen(silencers_path, "w");
    if(!to || fwrite(bytes, 1, len, to) != len || fclose(to) != 0) die("cannot write", silencers_path);
}

// whether the silencers' file is there, and its bytes
static void file_fields(FILE *f) {
    FILE *from = fopen(silencers_path, "r");
    if(!from) {
        fputs("\t0\t", f);
        return;
    }
    char *bytes = NULL;
    size_t len = 0, size = 0, got;
    do {
        size += 65536;
        bytes = reallocz(bytes, size);
        got = fread(bytes + len, 1, size - len, from);
        len += got;
    } while(got);
    fclose(from);
    fputs("\t1\t", f);
    oracle_esc_bytes(f, bytes, len);
    freez(bytes);
}

// the state as C prints it
static void list_field(FILE *f) {
    BUFFER *wb = buffer_create(0, NULL);
    health_silencers2json(wb);
    field(f, buffer_tostring(wb));
    buffer_free(wb);
}

// One request through C's handler: the reply's code, whether its content type is JSON, its body. The token is the
// management key (`ok`), another text (`bad`) or none (`-`); the query is what the web server hands over, decoded.
static void manage_fields(FILE *f, const char *token, const char *query) {
    static struct web_client w;
    static char another[] = "another-key";
    memset(&w, 0, sizeof(w));
    w.response.data = buffer_create(0, NULL);
    if(strcmp(token, "ok") == 0)
        w.auth_bearer_token = api_secret;
    else if(strcmp(token, "bad") == 0)
        w.auth_bearer_token = another;
    else if(strcmp(token, "-") != 0)
        die("an unknown token", token);
    char *url = strdupz(query);
    int code = web_client_api_request_v1_mgmt_health(&host, &w, url);
    fprintf(f, "\t%d\t%d", code, w.response.data->content_type == CT_APPLICATION_JSON ? 1 : 0);
    field(f, buffer_tostring(w.response.data));
    buffer_free(w.response.data);
    freez(url);
}

// ------------------------------------------------------------------------------------------------
// the badge: C's own api_v1_badge()

int api_v1_badge(RRDHOST *host, struct web_client *w, char *url);

// A request as the dispatcher hands it over (web_api.c): the decoded query, a data buffer that is text/plain and
// not to be cached, an empty header buffer.
static void badge_fields(FILE *f, const char *query) {
    static struct web_client w;
    memset(&w, 0, sizeof(w));
    w.response.data = buffer_create(0, NULL);
    w.response.header = buffer_create(0, NULL);
    w.response.data->content_type = CT_TEXT_PLAIN;
    buffer_no_cacheable(w.response.data);
    char *url = strdupz(query);
    int code = api_v1_badge(&host, &w, url);
    BUFFER *wb = w.response.data;
    fprintf(f, "\t%d\t%s\t%s\t", code,
            wb->content_type == CT_IMAGE_SVG_XML ? "svg" : wb->content_type == CT_TEXT_PLAIN ? "text" : "other",
            (wb->options & WB_CONTENT_CACHEABLE) ? "c" : (wb->options & WB_CONTENT_NO_CACHEABLE) ? "n" : "-");
    if(wb->date)
        fprintf(f, "%ld\t", (long)(wb->date - oracle.clock_s));
    else
        fputs("-\t", f);
    if(wb->expires)
        fprintf(f, "%ld", (long)(wb->expires - oracle.clock_s));
    else
        fputc('-', f);
    field(f, buffer_tostring(w.response.header));
    fputc('\t', f);
    oracle_esc_bytes(f, buffer_tostring(wb), buffer_strlen(wb));
    buffer_free(w.response.data);
    buffer_free(w.response.header);
    freez(url);
}

// ------------------------------------------------------------------------------------------------
// DynCfg: C's own health_dyncfg.c

// The hosts' index, holding the one host. A scenario's first DynCfg step makes it, so that every other scenario
// runs without one, as it always did.
static void host_index(void) {
    if(rrdhost_root_index)
        return;
    rrdhost_root_index = dictionary_create_advanced(
        DICT_OPTION_VALUE_LINK_DONT_CLONE | DICT_OPTION_DONT_OVERWRITE_VALUE, NULL, 0);
    dictionary_set(rrdhost_root_index, host.machine_guid, &host, sizeof(RRDHOST));
}

// a payload: `-` (or nothing) is none, `@<path>` a file's bytes, `=<id>` a GET's answer, anything else the text
// itself
static BUFFER *payload_of(const char *text) {
    if(!text || strcmp(text, "-") == 0)
        return NULL;
    BUFFER *wb = buffer_create(0, NULL);
    if(*text == '=') {
        usec_t stop_monotonic_ut = 0;
        bool cancelled = false;
        if(dyncfg_health_cb("oracle", text + 1, DYNCFG_CMD_GET, NULL, NULL, &stop_monotonic_ut, &cancelled, wb,
                            HTTP_ACCESS_ALL, "oracle", NULL) != 200)
            die("no GET of", text + 1);
        return wb;
    }
    if(*text != '@') {
        buffer_strcat(wb, text);
        return wb;
    }
    FILE *from = fopen(text + 1, "r");
    if(!from) die("cannot read the payload", text + 1);
    char chunk[65536];
    size_t got;
    while((got = fread(chunk, 1, sizeof(chunk), from)) > 0)
        buffer_memcat(wb, chunk, got);
    fclose(from);
    return wb;
}

// `none` is the command no word names
static bool action_of(const char *word, DYNCFG_CMDS *cmd) {
    *cmd = dyncfg_cmds2id(word);
    return *cmd != DYNCFG_CMD_NONE || strcmp(word, "none") == 0;
}

// One call of C's callback, off the HEALTH thread: the code, the content type, whether the body may be cached (c),
// may not (n) or neither was said (-), the expiry as seconds after the clock (`-`: none), the body.
static void dyncfg_fields(FILE *f, const char *id, DYNCFG_CMDS cmd, const char *name, BUFFER *payload) {
    BUFFER *result = buffer_create(0, NULL);
    usec_t stop_monotonic_ut = 0;
    bool cancelled = false;
    bool health_thread = is_health_thread;
    is_health_thread = false;
    int code = dyncfg_health_cb("oracle", id, cmd, name, payload, &stop_monotonic_ut, &cancelled, result,
                                HTTP_ACCESS_ALL, "oracle", NULL);
    is_health_thread = health_thread;
    fprintf(f, "\t%d", code);
    field(f, content_type_id2string(result->content_type));
    fprintf(f, "\t%s\t", (result->options & WB_CONTENT_CACHEABLE) ? "c"
                         : (result->options & WB_CONTENT_NO_CACHEABLE) ? "n" : "-");
    if(result->expires)
        fprintf(f, "%ld", (long)(result->expires - oracle.clock_s));
    else
        fputc('-', f);
    fputc('\t', f);
    oracle_esc_bytes(f, buffer_tostring(result), buffer_strlen(result));
    buffer_free(result);
}

// the last `store` row written of each name: a name gets a row again only when it changed
static struct store_row {
    char *name;
    char *text;
} *store_rows_seen;
static size_t store_rows_used, store_rows_size;

static bool store_row_changed(const char *name, char *text) {
    for(size_t i = 0; i < store_rows_used; i++)
        if(strcmp(store_rows_seen[i].name, name) == 0) {
            if(strcmp(store_rows_seen[i].text, text) == 0) {
                free(text);
                return false;
            }
            free(store_rows_seen[i].text);
            store_rows_seen[i].text = text;
            return true;
        }
    if(store_rows_used == store_rows_size) {
        store_rows_size = store_rows_size ? store_rows_size * 2 : 16;
        store_rows_seen = reallocz(store_rows_seen, store_rows_size * sizeof(*store_rows_seen));
    }
    store_rows_seen[store_rows_used++] = (struct store_row){ .name = strdupz(name), .text = text };
    return true;
}

// the rules' store: how many names and which, then each name that changed with its enabled flag, its chain's
// length and the chain as a GET prints it
static void store_rows(void) {
    RRD_ALERT_PROTOTYPE *ap;
    size_t names = 0;
    dfe_start_read(health_globals.prototypes.dict, ap) {
        names++;
    }
    dfe_done(ap);
    row("stored");
    fprintf(out, "\t%zu\t%s", names, names ? "" : "-");
    dfe_start_read(health_globals.prototypes.dict, ap) {
        fprintf(out, "%s%s", ap_dfe.counter ? " " : "", ap_dfe.name);
    }
    dfe_done(ap);
    fputc('\n', out);

    BUFFER *wb = buffer_create(0, NULL);
    dfe_start_read(health_globals.prototypes.dict, ap) {
        size_t rules = 0;
        for(RRD_ALERT_PROTOTYPE *t = ap; t; t = t->_internal.next)
            rules++;
        health_prototype_to_json(wb, ap, false);
        char *text = NULL;
        size_t size = 0;
        FILE *f = open_memstream(&text, &size);
        fprintf(f, "\t%d\t%zu", ap->_internal.enabled ? 1 : 0, rules);
        field(f, buffer_tostring(wb));
        fclose(f);
        if(store_row_changed(ap_dfe.name, text)) {
            row("store");
            field(out, ap_dfe.name);
            for(size_t i = 0; i < store_rows_used; i++)
                if(strcmp(store_rows_seen[i].name, ap_dfe.name) == 0)
                    fprintf(out, "%s\n", store_rows_seen[i].text);
        }
    }
    dfe_done(ap);
    buffer_free(wb);
}

// What every scenario starts from: C's records captured, the health globals, and one host without a chart.
static void world_init(const char *records_path) {
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
    // the names of the lookup options, as the daemon's start fills their hashes (web_client_api_v1_init())
    rrdr_options_init();
    rrdlabels_aral_init(false);
    health_init_prototypes();
    health_alarm_entry_aral_init();
    health_globals.config.default_exec = string_strdupz("/oracle/plugins.d/alarm-notify.sh");
    health_globals.config.default_recipient = string_strdupz("root");
    health_globals.config.enabled_alerts = simple_pattern_create("*", NULL, SIMPLE_PATTERN_EXACT, true);
    netdata_configured_user_config_dir = "/oracle/etc";
    is_health_thread = true;

    // C's silencers, empty, and no file of theirs
    snprintf(silencers_path, sizeof(silencers_path), "%s.silencers.json", records_path);
    unlink(silencers_path);
    health_globals.config.silencers_filename = string_strdupz(silencers_path);
    health_initialize_global_silencers();

    host.hostname = string_strdupz("oracle-host");
    // not the hostname: a notification's third word is this one
    host.registry_hostname = string_strdupz("oracle-registry");
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

    world_init(records_path);

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
            script->lookup_absolute = rest && strcmp(rest, "absolute") == 0;
            if(rest && *rest && !script->lookup_absolute) die("an unknown word after a lookup", whole);
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
        else if(strcmp(directive, "exec") == 0) {
            if(oracle.exec_rules_used == ORACLE_EXEC_RULES_MAX) die("too many exec rules", whole);
            struct oracle_exec_rule *rule = &oracle.exec_rules[oracle.exec_rules_used++];
            memset(rule, 0, sizeof(*rule));
            snprintf(rule->alert, sizeof(rule->alert), "%s", word(&rest, whole));
            snprintf(rule->status, sizeof(rule->status), "%s", word(&rest, whole));
            const char *kind = word(&rest, whole);
            if(strcmp(kind, "exit") == 0) {
                rule->kind = ORACLE_EXEC_EXIT;
                rule->slices = strtoul(word(&rest, whole), NULL, 10);
                rule->code = (int)strtol(word(&rest, whole), NULL, 10);
            }
            else if(strcmp(kind, "fail") == 0)
                rule->kind = ORACLE_EXEC_FAIL;
            else if(strcmp(kind, "error") == 0) {
                rule->kind = ORACLE_EXEC_ERROR;
                rule->slices = strtoul(word(&rest, whole), NULL, 10);
            }
            else if(strcmp(kind, "hang") == 0)
                rule->kind = ORACLE_EXEC_HANG;
            else
                die("unknown exec outcome", whole);
        }
        else if(strcmp(directive, "timeout") == 0) {
            long seconds = strtol(word(&rest, whole), NULL, 10);
            health_globals.config.notification_execution_timeout_seconds = (int32_t)seconds;
        }
        else if(strcmp(directive, "last-executed") == 0) {
            const char *answer = word(&rest, whole);
            if(strcmp(answer, "fail") == 0)
                oracle.last_executed_ret = -1;
            else if(strcmp(answer, "none") == 0)
                oracle.last_executed_ret = 0;
            else {
                oracle.last_executed_ret = 1;
                if(!status_of(answer, &oracle.last_executed_status)) die("unknown status", whole);
            }
        }
        else if(strcmp(directive, "use-summary") == 0)
            health_globals.config.use_summary_for_notifications = strcmp(word(&rest, whole), "1") == 0;
        else if(strcmp(directive, "default-exec") == 0) {
            string_freez(health_globals.config.default_exec);
            health_globals.config.default_exec = (rest && strcmp(rest, "-") != 0) ? string_strdupz(rest) : NULL;
        }
        else if(strcmp(directive, "auto-wait") == 0)
            auto_wait = strcmp(word(&rest, whole), "1") == 0;
        else if(strcmp(directive, "wait") == 0) {
            wait_for_all_notifications_to_finish_before_allowing_health_to_be_cleaned_up();
            dump(whole);
        }
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
            // health_event_loop(): after its hosts, unless the service stops
            if(auto_wait && oracle_running_peek())
                wait_for_all_notifications_to_finish_before_allowing_health_to_be_cleaned_up();
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
            // the daemon's one call (sqlite_metadata.c), which ends with the memory's cleanup unless its statement
            // fails to prepare; without a database that statement never prepares, and the memory's cleanup is
            // called alone
            sql_health_alarm_log_cleanup(&host);
            if(!db_meta)
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
        else if(strcmp(directive, "configs") == 0) {
            configs(whole);
            dump(whole);
        }
        else if(strcmp(directive, "transitions") == 0) {
            transitions(rest, whole);
            dump(whole);
        }
        else if(strcmp(directive, "configurations") == 0) {
            configurations(rest, whole);
            dump(whole);
        }
        else if(strcmp(directive, "silencers-file") == 0) {
            const char *text = rest ? rest : "";
            if(strcmp(text, "-") == 0)
                unlink(silencers_path);
            else
                file_write(text, strlen(text));
        }
        else if(strcmp(directive, "load") == 0) {
            health_silencers_init();
            row("list");
            list_field(out);
            fputc('\n', out);
            dump(whole);
        }
        else if(strcmp(directive, "manage") == 0) {
            char *token = word(&rest, whole);
            row("reply");
            manage_fields(out, token, rest ? rest : "");
            fputc('\n', out);
            row("file");
            file_fields(out);
            fputc('\n', out);
            dump(whole);
        }
        else if(strcmp(directive, "badge") == 0) {
            row("badge");
            badge_fields(out, rest ? rest : "");
            fputc('\n', out);
            dump(whole);
        }
        else if(strcmp(directive, "dyncfg") == 0) {
            char *id = word(&rest, whole);
            DYNCFG_CMDS cmd;
            if(!action_of(word(&rest, whole), &cmd)) die("an unknown action", whole);
            char *name = word(&rest, whole);
            BUFFER *payload = payload_of(rest);
            host_index();
            row("answer");
            dyncfg_fields(out, id, cmd, strcmp(name, "-") == 0 ? NULL : name, payload);
            fputc('\n', out);
            buffer_free(payload);
            store_rows();
            dump(whole);
        }
        else if(strcmp(directive, "register") == 0) {
            host_index();
            bool health_thread = is_health_thread;
            is_health_thread = false;
            __atomic_store_n(&health_globals.prototypes.registering, true, __ATOMIC_RELAXED);
            health_dyncfg_register_all_prototypes();
            __atomic_store_n(&health_globals.prototypes.registering, false, __ATOMIC_RELAXED);
            is_health_thread = health_thread;
            store_rows();
            dump(whole);
        }
        else if(strcmp(directive, "unregister") == 0) {
            health_dyncfg_unregister_all_prototypes();
            dump(whole);
        }
        else if(strcmp(directive, "dyncfg-saved") == 0) {
            if(oracle.dyncfg_saved_used == ORACLE_DYNCFG_MAX) die("too many saved jobs", whole);
            struct oracle_dyncfg_saved *saved = &oracle.dyncfg_saved[oracle.dyncfg_saved_used++];
            snprintf(saved->name, sizeof(saved->name), "%s", word(&rest, whole));
            BUFFER *payload = payload_of(rest);
            if(!payload) die("a saved job without a payload", whole);
            saved->len = buffer_strlen(payload);
            saved->payload = mallocz(saved->len + 1);
            memcpy(saved->payload, buffer_tostring(payload), saved->len + 1);
            buffer_free(payload);
        }
        else if(strcmp(directive, "dyncfg-user-disabled") == 0) {
            if(oracle.dyncfg_user_disabled_used == ORACLE_DYNCFG_MAX) die("too many disabled nodes", whole);
            snprintf(oracle.dyncfg_user_disabled[oracle.dyncfg_user_disabled_used++],
                     sizeof(oracle.dyncfg_user_disabled[0]), "%s", word(&rest, whole));
        }
        else if(strcmp(directive, "cloud-has") == 0)
            oracle.hash_not_sent = atoi(word(&rest, whole)) == 0;
        else if(strcmp(directive, "dyncfg-forget") == 0) {
            const char *what = word(&rest, whole);
            size_t kept = 0;
            for(size_t i = 0; i < oracle.dyncfg_saved_used; i++) {
                if(strcmp(oracle.dyncfg_saved[i].name, what) == 0) {
                    freez(oracle.dyncfg_saved[i].payload);
                    continue;
                }
                oracle.dyncfg_saved[kept++] = oracle.dyncfg_saved[i];
            }
            bool forgot = kept != oracle.dyncfg_saved_used;
            oracle.dyncfg_saved_used = kept;
            kept = 0;
            for(size_t i = 0; i < oracle.dyncfg_user_disabled_used; i++) {
                if(strcmp(oracle.dyncfg_user_disabled[i], what) == 0)
                    continue;
                if(kept != i)
                    memcpy(oracle.dyncfg_user_disabled[kept], oracle.dyncfg_user_disabled[i],
                           sizeof(oracle.dyncfg_user_disabled[0]));
                kept++;
            }
            forgot = forgot || kept != oracle.dyncfg_user_disabled_used;
            oracle.dyncfg_user_disabled_used = kept;
            if(!forgot) die("nothing to forget", whole);
        }
        else if(strcmp(directive, "health-dirs") == 0) {
            oracle_health_user_dir = strdupz(word(&rest, whole));
            const char *stock = word(&rest, whole);
            health_globals.config.stock_enabled = strcmp(stock, "-") != 0;
            if(health_globals.config.stock_enabled)
                oracle_health_stock_dir = strdupz(stock);
        }
        else if(strcmp(directive, "health-enabled") == 0)
            host.health.enabled = atoi(word(&rest, whole)) != 0;
        else if(strcmp(directive, "disconnect") == 0) {
            bool health_thread = is_health_thread;
            is_health_thread = false;
            oracle.gate = false;
            rrdcalc_child_disconnected(&host);
            host.health.enabled = false;
            is_health_thread = health_thread;
            dump(whole);
        }
        else if(strcmp(directive, "reload") == 0) {
            host_index();
            bool health_thread = is_health_thread;
            is_health_thread = false;
            health_reload_prototypes();
            health_apply_prototypes_to_all_hosts();
            is_health_thread = health_thread;
            store_rows();
            dump(whole);
        }
        else if(strcmp(directive, "enabled-alarms") == 0) {
            if(!rest || !*rest) die("a missing argument", whole);
            simple_pattern_free(health_globals.config.enabled_alerts);
            health_globals.config.enabled_alerts = simple_pattern_create(rest, NULL, SIMPLE_PATTERN_EXACT, true);
        }
        else
            die("unknown directive", whole);

        freez(whole);
    }
    free(line);
    fclose(input);
    if(ferror(out) || fclose(out) != 0) die("cannot write", out_path);
}

// ------------------------------------------------------------------------------------------------
// the decision table

// the messages of the records written since the last call, joined by ` | `; `-` for none
static void messages(FILE *f) {
    fflush(stderr);
    off_t end = lseek(records_fd, 0, SEEK_END);
    if(end <= records_read) {
        fputc('-', f);
        return;
    }
    size_t len = (size_t)(end - records_read);
    char *text = mallocz(len + 1);
    if(pread(records_fd, text, len, records_read) != (ssize_t)len) die("cannot read the records", NULL);
    text[len] = '\0';
    records_read = end;

    size_t written = 0;
    char *save = NULL;
    for(char *line = strtok_r(text, "\n", &save); line; line = strtok_r(NULL, "\n", &save)) {
        char *msg = strstr(line, " msg=\"");
        if(!msg)
            continue;
        msg += 6;
        char *close = strrchr(msg, '"');
        if(close)
            *close = '\0';
        if(written++)
            fputs(" | ", f);
        name_file(msg);
        oracle_esc(f, msg);
    }
    if(!written)
        fputc('-', f);
    freez(text);
}

static void decide(const char *dir, const char *records_path) {
    char path[4096];
    snprintf(path, sizeof(path), "%s/decide.tsv", dir);
    FILE *f = fopen(path, "w");
    if(!f) die("cannot create", path);
    fprintf(f, "# generated by tests/oracle/gen-loop-vectors.c from the C implementation; do not edit\n");
    fprintf(f, "# columns: new status, old status, flags before, the table's answer about the last executed event "
               "(fail, none or a status), the table was asked, a command was spawned, flags after, "
               "exec_run_timestamp was set, saves, the records' messages\n");

    world_init(records_path);
    // what a host's first pass takes from the configuration
    host.health.default_exec = string_dup(health_globals.config.default_exec);
    host.health.default_recipient = string_dup(health_globals.config.default_recipient);
    host.health.use_summary_for_notifications = health_globals.config.use_summary_for_notifications;
    struct health_raised_summary *hrm = alerts_raised_summary_create(&host);

    ALARM_ENTRY *ae = health_alarm_entry_create();
    ae->name = string_strdupz("d_alert");
    ae->chart = string_strdupz("d.chart");
    ae->chart_name = string_strdupz("d.chart_name");
    ae->chart_context = string_strdupz("d.context");
    ae->units = string_strdupz("things");
    ae->info = string_strdupz("an alert of the decision table");
    ae->summary = string_strdupz("a summary");
    ae->old_value_string = string_strdupz("1 things");
    ae->new_value_string = string_strdupz("2 things");
    ae->unique_id = 7;
    ae->alarm_id = 3;
    ae->alarm_event_id = 5;
    ae->when = oracle.clock_s;
    ae->old_value = 1;
    ae->new_value = 2;

    static const HEALTH_ENTRY_FLAGS bits[] = { HEALTH_ENTRY_FLAG_NO_CLEAR_NOTIFICATION, HEALTH_ENTRY_FLAG_SILENCED,
                                               HEALTH_ENTRY_RUN_ONCE, HEALTH_ENTRY_FLAG_IS_REPEATING };
    for(int new_status = RRDCALC_STATUS_REMOVED; new_status <= RRDCALC_STATUS_CRITICAL; new_status++)
        for(int old_status = RRDCALC_STATUS_REMOVED; old_status <= RRDCALC_STATUS_CRITICAL; old_status++)
            for(unsigned subset = 0; subset < 16; subset++)
                // the table's answer: the question fails, no executed event, or one with each status
                for(int answer = -1; answer <= 7; answer++) {
                    HEALTH_ENTRY_FLAGS flags = 0;
                    for(size_t b = 0; b < 4; b++)
                        if(subset & (1u << b))
                            flags |= bits[b];
                    ae->new_status = (RRDCALC_STATUS)new_status;
                    ae->old_status = (RRDCALC_STATUS)old_status;
                    ae->flags = flags;
                    ae->exec_run_timestamp = 0;
                    ae->exec_code = 0;
                    oracle.last_executed_ret = (answer < 1) ? answer : 1;
                    oracle.last_executed_status = (RRDCALC_STATUS)(answer - 1 + RRDCALC_STATUS_REMOVED);
                    oracle.asked = oracle.spawned = oracle.saved = 0;
                    errno = 0;

                    health_send_notification(&host, ae, hrm);
                    HEALTH_ENTRY_FLAGS after = ae->flags;
                    bool timestamp = ae->exec_run_timestamp != 0;
                    // a started command is waited for, so that the entry leaves the list of running notifications
                    if(ae->popen_instance)
                        health_alarm_wait_for_execution(ae);

                    fprintf(f, "%s\t%s\t%08x\t%s\t%zu\t%zu\t%08x\t%d\t%zu\t", rrdcalc_status2string(ae->new_status),
                            rrdcalc_status2string(ae->old_status), (unsigned)flags,
                            answer == -1 ? "fail" : answer == 0 ? "none"
                                         : rrdcalc_status2string(oracle.last_executed_status),
                            oracle.asked, oracle.spawned, (unsigned)after, timestamp ? 1 : 0, oracle.saved);
                    messages(f);
                    fputc('\n', f);
                }
    if(ferror(f) || fclose(f) != 0) die("cannot write", path);
}

// ------------------------------------------------------------------------------------------------
// the silencers' tables

// A case runs in a child process, so that a crash of C is a row too: `label`, already escaped, then `signal <n>`.
// The child appends its rows itself, each one whole; what it logged is its own.
static void in_child(FILE *f, const char *label, void (*rows)(FILE *f, const void *arg), const void *arg) {
    fflush(NULL);
    pid_t pid = fork();
    if(pid < 0) die("cannot fork", NULL);
    if(pid == 0) {
        struct rlimit none = { 0, 0 };
        setrlimit(RLIMIT_CORE, &none);
        rows(f, arg);
        fflush(f);
        _exit(ferror(f) ? 1 : 0);
    }
    int status = 0;
    if(waitpid(pid, &status, 0) != pid) die("cannot wait for the process of", label);
    if(WIFSIGNALED(status))
        fprintf(f, "%s\tsignal %d\n", label, WTERMSIG(status));
    else if(!WIFEXITED(status) || WEXITSTATUS(status) != 0)
        die("a row's process failed", label);
    records_read = lseek(records_fd, 0, SEEK_END);
}

// a table: created empty with its two comment lines, then appended to by this process and its children
static FILE *table(const char *dir, const char *name, const char *columns) {
    char path[4096];
    snprintf(path, sizeof(path), "%s/%s", dir, name);
    FILE *f = fopen(path, "w");
    if(!f || fclose(f) != 0) die("cannot create", path);
    f = fopen(path, "a");
    if(!f) die("cannot append to", path);
    fprintf(f, "# generated by tests/oracle/gen-loop-vectors.c from the C implementation; do not edit\n");
    fprintf(f, "# columns: %s\n", columns);
    return f;
}

static char *escaped(const void *bytes, size_t len) {
    char *text = NULL;
    size_t size = 0;
    FILE *to = open_memstream(&text, &size);
    oracle_esc_bytes(to, bytes, len);
    fclose(to);
    return text;
}

struct file_case {
    const char *bytes;
    size_t len;
};

static void file_row(FILE *f, const void *arg) {
    const struct file_case *c = arg;
    file_write(c->bytes, c->len);
    errno = 0;
    health_silencers_init();
    oracle_esc_bytes(f, c->bytes, c->len);
    list_field(f);
    fputc('\t', f);
    messages(f);
    fputc('\n', f);
}

#define MANAGE_REQUESTS_MAX 48
struct manage_case {
    const char *name;
    const char *requests[MANAGE_REQUESTS_MAX];      // each `<token> <query>`
};

static void manage_rows(FILE *f, const void *arg) {
    const struct manage_case *c = arg;
    bool unwritable = strcmp(c->name, "unwritable") == 0;
    if(unwritable) {
        // the file's path leads through a regular file
        char path[4200];
        file_write("", 0);
        snprintf(path, sizeof(path), "%s/x", silencers_path);
        health_globals.config.silencers_filename = string_strdupz(path);
    }
    for(size_t n = 0; n < MANAGE_REQUESTS_MAX && c->requests[n]; n++) {
        char *request = strdupz(c->requests[n]);
        char *query = request;
        char *token = strsep(&query, " ");
        if(!query) query = "";
        if(!unwritable)
            unlink(silencers_path);
        errno = 0;
        fprintf(f, "%s\t%zu\t%s", c->name, n, token);
        field(f, query);
        manage_fields(f, token, query);
        file_fields(f);
        fputc('\t', f);
        messages(f);
        fputc('\n', f);
        fflush(f);
        freez(request);
    }
}

#define MATCH_REQUESTS_MAX 3
struct match_case {
    const char *requests[MATCH_REQUESTS_MAX + 1];   // each a query, sent with the management key
};

// a state's requests, joined by ` ; `
static char *match_label(const struct match_case *c) {
    char *text = NULL;
    size_t size = 0;
    FILE *label = open_memstream(&text, &size);
    for(size_t n = 0; c->requests[n]; n++) {
        fprintf(label, "%s", n ? " ; " : "");
        oracle_esc(label, c->requests[n]);
    }
    fclose(label);
    return text;
}

static const char *type_name(SILENCE_TYPE type) {
    return type == STYPE_NONE ? "None" : type == STYPE_DISABLE_ALARMS ? "DISABLE" : "SILENCE";
}

static void match_rows(FILE *f, const void *arg) {
    const struct match_case *c = arg;
    static const struct {
        const char *name, *chart, *context, *hostname;
    } alerts[] = {
        { "m_alert", "m.chart", "m.context", "m-host" },
        { "m_alert", "m.chart", NULL, "m-host" },           // no chart: an alert between an unlink and its free
        { "x_alert", "x.chart", "x.context", "x-host" },
    };
    static const uint32_t before[] = { 0, RRDCALC_FLAG_RUNNABLE, RRDCALC_FLAG_RUNNABLE | RRDCALC_FLAG_DISABLED,
                                       RRDCALC_FLAG_RUNNABLE | RRDCALC_FLAG_SILENCED };

    // the state, through the handler; its replies and records are of the other table
    char *state = match_label(c);
    FILE *sink = fopen("/dev/null", "w");
    if(!sink) die("cannot open", "/dev/null");
    for(size_t n = 0; c->requests[n]; n++)
        manage_fields(sink, "ok", c->requests[n]);
    fclose(sink);
    unlink(silencers_path);
    fflush(stderr);
    records_read = lseek(records_fd, 0, SEEK_END);

    for(size_t a = 0; a < sizeof(alerts) / sizeof(alerts[0]); a++)
        for(size_t b = 0; b < sizeof(before) / sizeof(before[0]); b++) {
            static RRDHOST h;
            static RRDSET st;
            static RRDCALC rc;
            memset(&h, 0, sizeof(h));
            memset(&st, 0, sizeof(st));
            memset(&rc, 0, sizeof(rc));
            h.hostname = string_strdupz(alerts[a].hostname);
            // the registry's hostname is not asked
            h.registry_hostname = string_strdupz("m-registry");
            // the chart's name is not asked either
            st.id = string_strdupz(alerts[a].chart);
            st.name = string_strdupz("m.chart_name");
            if(alerts[a].context) {
                st.context = string_strdupz(alerts[a].context);
                rc.rrdset = &st;
            }
            rc.config.name = string_strdupz(alerts[a].name);
            rc.chart = string_strdupz(alerts[a].chart);
            rc.run_flags = before[b];

            errno = 0;
            SILENCE_TYPE type = health_silencers_check_silenced(&rc, alerts[a].hostname);
            int ret = health_silencers_update_disabled_silenced(&h, &rc);
            fprintf(f, "%s", state);
            field(f, alerts[a].name);
            field(f, alerts[a].chart);
            field(f, alerts[a].context);
            field(f, alerts[a].hostname);
            fprintf(f, "\t%08x\t%s\t%d\t%08x\t", (unsigned)before[b], type_name(type), ret, (unsigned)rc.run_flags);
            messages(f);
            fputc('\n', f);
            fflush(f);
        }
}

static void silencers_tables(const char *dir, const char *records_path) {
    world_init(records_path);

    // ---- the file
    FILE *f = table(dir, "silencers-file.tsv", "the file's bytes, the state as health_silencers2json() prints it, "
                                               "the records' messages; or the bytes and `signal <n>`");
    static const char *const files[] = {
        // what C itself writes
        "{\n\t\"all\": false,\n\t\"type\": \"None\",\n\t\"silencers\": []\n}\n",
        "{\n\t\"all\": true,\n\t\"type\": \"DISABLE\",\n\t\"silencers\": []\n}\n",
        "{\n\t\"all\": false,\n\t\"type\": \"SILENCE\",\n\t\"silencers\": [\n\t\t{\n\t\t\t\"alarm\": \"a\""
        "\n\t\t}\n\t]\n}\n",
        "{\n\t\"all\": false,\n\t\"type\": \"DISABLE\",\n\t\"silencers\": [\n\t\t{\n\t\t\t\"alarm\": \"a b\",\n\t\t\t"
        "\"chart\": \"c\"\n\t\t},\n\t\t{\n\t\t\t\"context\": \"x\",\n\t\t\t\"hosts\": \"h\"\n\t\t}\n\t]\n}\n",
        "{\n\t\"all\": false,\n\t\"type\": \"SILENCE\",\n\t\"silencers\": [\n\t\t{\n\t\t\t\"alarm\": \"1\"\n\t\t},"
        "\n\t\t{\n\t\t\t\"alarm\": \"2\"\n\t\t},\n\t\t{\n\t\t\t\"alarm\": \"3\"\n\t\t}\n\t]\n}\n",
        // the type
        "{\"type\":\"SILENCE\"}",
        "{\"type\":\"DISABLE\"}",
        "{\"type\":\"None\"}",
        "{\"type\":\"silence\"}",
        "{\"type\":\"\"}",
        "{\"all\":true}",
        "{\"type\":\"SILENCE\",\"type\":\"DISABLE\"}",
        "{\"type\":\"SILENCE\",\"silencers\":[{\"type\":\"None\"}]}",
        "{\"type\":\"SILENCE\",\"silencers\":[{\"type\":\"DISABLE\"}]}",
        "{\"silencers\":[{\"type\":\"DISABLE\",\"alarm\":\"a\"}]}",
        "{\"TYPE\":\"SILENCE\"}",
        "{\"type\":true}",
        "{\"type\":1}",
        // the booleans
        "{\"all\":false}",
        "{\"foo\":true}",
        "{\"all\":true,\"foo\":false}",
        "{\"foo\":false,\"all\":true}",
        "{\"zz\":true,\"aa\":false}",
        "{\"aa\":true,\"zz\":false}",
        "{\"aa\":true,\"zz\":false,\"aa\":true}",
        "{\"zz\":false,\"aa\":true,\"zz\":true}",
        "{\"silencers\":[{\"alarm\":\"a\",\"x\":true}]}",
        "{\"all\":true,\"silencers\":[{\"alarm\":\"a\",\"x\":false}]}",
        "{\"silencers\":[{\"alarm\":\"a\",\"x\":false}],\"all\":true}",
        "{\"all\":1}",
        "{\"all\":\"true\"}",
        "{\"all\":null}",
        // the arrays
        "{\"silencers\":[]}",
        "{\"silencers\":null}",
        "{\"silencers\":{\"alarm\":\"a\"}}",
        "{\"other\":[{\"alarm\":\"a\"}]}",
        "{\"one\":[{\"alarm\":\"a\"}],\"two\":[{\"alarm\":\"b\"}]}",
        "{\"silencers\":[{\"alarm\":\"a\",\"more\":[{\"chart\":\"c\"}]}]}",
        "{\"silencers\":[{\"more\":[{\"chart\":\"c\"}],\"alarm\":\"a\"}]}",
        "{\"silencers\":[{\"o\":{\"alarm\":\"x\",\"type\":\"DISABLE\",\"b\":true},\"chart\":\"c\"}]}",
        "{\"o\":{\"alarm\":\"x\",\"type\":\"DISABLE\",\"b\":true}}",
        "[{\"alarm\":\"a\"}]",
        "{\"silencers\":[{}]}",
        "{\"silencers\":[{},{}]}",
        "{\"silencers\":[null]}",
        "{\"silencers\":[null,{\"alarm\":\"a\"},null]}",
        // the names
        "{\"silencers\":[{\"ALARM\":\"a\"}]}",
        "{\"silencers\":[{\"Alarm\":\"a\",\"CHART\":\"c\",\"Context\":\"x\",\"HOSTS\":\"h\"}]}",
        "{\"silencers\":[{\"alarm\":\"a\",\"ALARM\":\"b\"}]}",
        "{\"silencers\":[{\"ALARM\":\"b\",\"alarm\":\"a\"}]}",
        "{\"silencers\":[{\"alarm\":\"a\",\"alarm\":\"b\"}]}",
        "{\"silencers\":[{\"template\":\"t\"}]}",
        "{\"silencers\":[{\"foo\":\"x\"}]}",
        "{\"silencers\":[{\"host\":\"h\"}]}",
        "{\"silencers\":[{\"families\":\"f\"}]}",
        "{\"silencers\":[{\"hosts\":\"h\",\"context\":\"x\",\"chart\":\"c\",\"alarm\":\"a\"}]}",
        "{\"alarm\":\"a\"}",
        "{\"alarm\":\"a\",\"silencers\":[{\"chart\":\"c\"}]}",
        // the values
        "{\"silencers\":[{\"alarm\":5}]}",
        "{\"silencers\":[{\"alarm\":1.5}]}",
        "{\"silencers\":[{\"alarm\":null}]}",
        "{\"silencers\":[{\"alarm\":\"\"}]}",
        "{\"silencers\":[{\"alarm\":\" \"}]}",
        "{\"silencers\":[{\"alarm\":\"!\"}]}",
        "{\"silencers\":[{\"alarm\":\"a\\\\b\\\"c\"}]}",
        "{\"silencers\":[{\"alarm\":\"a\\tb\\nc\"}]}",
        "{\"silencers\":[{\"alarm\":\"a\\u0000b\"}]}",
        "{\"silencers\":[{\"alarm\\u0000x\":\"a\"}]}",
        "{\"silencers\":[{\"alarm\":\"\\u00e9\\u20ac\"}]}",
        "{\"silencers\":[{\"alarm\":\"\xc3\xa9\"}]}",
        "{\"silencers\":[{\"alarm\":\"\xff\"}]}",
        // an element that is no object: C dies
        "{\"silencers\":[\"x\"]}",
        "{\"silencers\":[5]}",
        "{\"silencers\":[true]}",
        "{\"silencers\":[[{\"alarm\":\"a\"}]]}",
        "{\"silencers\":[{\"alarm\":\"a\"},\"x\"]}",
        // what is no JSON, or more than one value
        "",
        " ",
        "{",
        "not json",
        "null",
        "5",
        "\"text\"",
        "{\"all\":true} trailing",
        "{\"all\":true}{\"all\":false}",
        "{\"all\":true,}",
        "{'all':true}",
        "/* a comment */ {\"all\":true}",
        "{\"all\":TRUE}",
        "\xef\xbb\xbf{\"all\":true}",
        // deeper shapes: an array in an object in an element; 32 levels, and 33
        "{\"silencers\":[{\"o\":{\"in\":[{\"alarm\":\"deep\"}]},\"chart\":\"c\"}]}",
        "{\"a\":[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[true]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]}",
        "{\"a\":[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[true]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]}",
        // a control byte as it is in a string; half of a surrogate pair
        "{\"silencers\":[{\"alarm\":\"a\x01" "b\"}]}",
        "{\"silencers\":[{\"alarm\":\"a\\ud800b\"}]}",
    };
    for(size_t i = 0; i < sizeof(files) / sizeof(files[0]); i++) {
        struct file_case c = { files[i], strlen(files[i]) };
        char *label = escaped(c.bytes, c.len);
        in_child(f, label, file_row, &c);
        free(label);
    }
    // a name of 300 bytes; a file of 9,999 bytes, and one of 10,000
    {
        char text[10001];
        int n = snprintf(text, sizeof(text), "{\"silencers\":[{\"");
        memset(text + n, 'n', 300);
        snprintf(text + n + 300, sizeof(text) - n - 300, "\":\"v\",\"alarm\":\"a\"}]}");
        struct file_case c = { text, strlen(text) };
        char *label = escaped(c.bytes, c.len);
        in_child(f, label, file_row, &c);
        free(label);

        static const size_t sizes[] = { 9999, 10000 };
        for(size_t i = 0; i < 2; i++) {
            n = snprintf(text, sizeof(text), "{\"all\":true,\"type\":\"DISABLE\",\"silencers\":[{\"alarm\":\"a\"}]}");
            memset(text + n, ' ', sizes[i] - (size_t)n);
            c = (struct file_case){ text, sizes[i] };
            label = escaped(c.bytes, c.len);
            in_child(f, label, file_row, &c);
            free(label);
        }
    }
    if(ferror(f) || fclose(f) != 0) die("cannot write", "silencers-file.tsv");

    // ---- the requests
    f = table(dir, "manage.tsv", "sequence, n, the token (ok, bad, -), the query, the reply's code, its content "
                                 "type is JSON, its body, the file is there, the file's bytes, the records' "
                                 "messages; or the sequence and `signal <n>`");
    static const struct manage_case sequences[] = {
        // C's own test (tests/health_mgmtapi/health-cmdapi-test.sh.in), each command and the LIST that follows it
        { "script", {
            "ok cmd=RESET", "ok cmd=LIST",
            "bad cmd=DISABLE ALL", "bad cmd=LIST",
            "ok cmd=DISABLE ALL", "ok cmd=LIST",
            "ok cmd=RESET", "ok cmd=LIST",
            "ok cmd=SILENCE ALL", "ok cmd=LIST",
            "ok cmd=RESET", "ok cmd=LIST",
            "ok cmd=SILENCE&alarm=*10min_cpu_usage *load_trigger", "ok cmd=LIST",
            "ok cmd=DISABLE", "ok cmd=LIST",
            "ok cmd=SILENCE", "ok cmd=LIST",
            "ok alarm=*10min_cpu_iowait", "ok cmd=LIST",
            "ok cmd=RESET", "ok cmd=LIST",
            "ok cmd=DISABLE&chart=system.load", "ok cmd=LIST",
            "ok context=system.cpu", "ok cmd=LIST",
            "ok cmd=RESET", "ok cmd=LIST",
            "ok cmd=SILENCE&alarm=*10min_cpu_usage *load_trigger&chart=system.load", "ok cmd=LIST",
            "ok alarm=*10min_cpu_usage *load_trigger&context=system.cpu", "ok cmd=LIST",
            "ok cmd=RESET", "ok cmd=LIST",
            "ok cmd=SILENCE", "ok cmd=LIST",
            "ok cmd=RESET", "ok cmd=LIST",
            "ok cmd=SILENCE", "ok cmd=LIST",
            "ok hosts=*", "ok cmd=LIST",
            "ok cmd=RESET", "ok cmd=LIST",
        } },
        // the token
        { "no-token", { "- cmd=SILENCE ALL", "- cmd=LIST", "- ", "ok cmd=LIST" } },
        { "bad-token", { "bad cmd=SILENCE ALL", "bad alarm=a", "bad ", "ok cmd=LIST" } },
        // what is no command and no selector
        { "empty", { "ok ", "ok cmd=LIST" } },
        { "separators", { "ok &&", "ok &=&", "ok =", "ok ==x", "ok cmd", "ok cmd=", "ok cmd=LIST" } },
        { "unknown-cmd", { "ok cmd=FOO", "ok cmd=list", "ok cmd=Silence", "ok cmd=SILENCE ", "ok cmd= SILENCE",
                           "ok cmd=SILENCE  ALL", "ok cmd=LIST" } },
        { "cmd-case", { "ok CMD=SILENCE ALL", "ok Cmd=LIST", "ok cmd=LIST" } },
        { "unknown-key", { "ok foo=bar", "ok cmd=LIST", "ok foo=bar&alarm=a", "ok cmd=LIST", "ok alarm=b&foo=bar",
                           "ok cmd=LIST" } },
        // LIST with other things
        { "list-first", { "ok cmd=LIST&cmd=SILENCE ALL", "ok cmd=LIST" } },
        { "list-last", { "ok cmd=SILENCE ALL&cmd=LIST", "ok cmd=LIST" } },
        { "list-selector", { "ok alarm=a&cmd=LIST", "ok cmd=LIST&alarm=b", "ok cmd=LIST" } },
        { "list-twice", { "ok cmd=SILENCE&alarm=a", "ok cmd=LIST&cmd=LIST" } },
        { "list-warning", { "ok cmd=SILENCE", "ok cmd=LIST", "ok cmd=DISABLE&cmd=LIST" } },
        // several commands in one request
        { "commands", { "ok cmd=DISABLE ALL&cmd=SILENCE", "ok cmd=LIST", "ok cmd=SILENCE ALL&cmd=RESET", "ok cmd=LIST",
                        "ok cmd=RESET&cmd=SILENCE ALL", "ok cmd=LIST", "ok cmd=SILENCE&cmd=DISABLE", "ok cmd=LIST" } },
        { "all-stays", { "ok cmd=SILENCE ALL", "ok cmd=DISABLE", "ok cmd=LIST", "ok cmd=SILENCE", "ok cmd=LIST" } },
        // a selector and a reset in one request
        { "selector-reset", { "ok cmd=SILENCE&alarm=old", "ok alarm=x&cmd=RESET", "ok cmd=LIST",
                              "ok cmd=RESET&alarm=y", "ok cmd=LIST" } },
        // the selector's keys
        { "keys", { "ok cmd=SILENCE&hosts=h&context=x&chart=c&alarm=a", "ok cmd=LIST" } },
        { "key-case", { "ok cmd=SILENCE&ALARM=a&Chart=c&CONTEXT=x&hOsTs=h", "ok cmd=LIST" } },
        { "key-repeat", { "ok cmd=SILENCE&alarm=a&alarm=b", "ok cmd=LIST" } },
        { "key-empty", { "ok cmd=SILENCE&ALARM=", "ok cmd=LIST", "ok alarm=&chart=c", "ok cmd=LIST" } },
        { "template", { "ok cmd=SILENCE&template=x", "ok cmd=LIST" } },
        { "host", { "ok cmd=SILENCE&host=h", "ok cmd=LIST" } },
        { "families", { "ok cmd=SILENCE&families=load", "ok cmd=LIST" } },
        // the selector's values
        { "value-equals", { "ok alarm=a=b", "ok chart==c", "ok cmd=LIST" } },
        { "value-quote", { "ok cmd=SILENCE&alarm=a\"b", "ok cmd=LIST" } },
        { "value-backslash", { "ok cmd=SILENCE&alarm=a\\b", "ok cmd=LIST" } },
        { "value-control", { "ok cmd=SILENCE&alarm=a\tb\nc", "ok cmd=LIST" } },
        { "value-space", { "ok cmd=SILENCE&alarm= ", "ok cmd=LIST" } },
        { "value-negative", { "ok cmd=SILENCE&alarm=!", "ok cmd=LIST", "ok alarm=!a *", "ok cmd=LIST" } },
        { "value-utf8", { "ok cmd=SILENCE&alarm=\xc3\xa9\xff", "ok cmd=LIST" } },
        // the order of selectors, and the warnings
        { "order", { "ok alarm=1", "ok alarm=2", "ok alarm=3", "ok cmd=LIST" } },
        { "warnings", { "ok alarm=a", "ok cmd=SILENCE", "ok cmd=RESET", "ok cmd=DISABLE", "ok alarm=a",
                        "ok cmd=SILENCE ALL&cmd=RESET&cmd=SILENCE", "ok cmd=LIST" } },
        // the file cannot be written
        { "unwritable", { "ok cmd=SILENCE ALL", "ok cmd=LIST", "ok ", "bad cmd=RESET" } },
    };
    for(size_t i = 0; i < sizeof(sequences) / sizeof(sequences[0]); i++)
        in_child(f, sequences[i].name, manage_rows, &sequences[i]);
    if(ferror(f) || fclose(f) != 0) die("cannot write", "manage.tsv");

    // ---- the match
    f = table(dir, "silencers-match.tsv", "the requests that made the state, the alert's name, its chart, its "
                                          "chart's context, the hostname, run flags before, the type that "
                                          "matches, the update's result, run flags after, the records' messages; "
                                          "or the requests and `signal <n>`");
    static const struct match_case states[] = {
        { { NULL } },
        { { "cmd=SILENCE" } },
        { { "cmd=SILENCE ALL" } },
        { { "cmd=DISABLE ALL" } },
        { { "alarm=m_alert" } },
        { { "cmd=SILENCE&alarm=m_alert" } },
        { { "cmd=DISABLE&alarm=m_alert" } },
        { { "cmd=SILENCE&alarm=m_*" } },
        { { "cmd=SILENCE&alarm=*" } },
        { { "cmd=SILENCE&alarm=other m_alert" } },
        { { "cmd=SILENCE&alarm=!m_alert *" } },
        { { "cmd=SILENCE&alarm=!other *" } },
        { { "cmd=SILENCE&alarm=other" } },
        { { "cmd=SILENCE&alarm= " } },
        { { "cmd=SILENCE&alarm=!" } },
        { { "cmd=SILENCE&alarm=M_ALERT" } },
        { { "cmd=SILENCE&chart=m.chart" } },
        { { "cmd=SILENCE&chart=m.chart_name" } },
        { { "cmd=SILENCE&chart=other" } },
        { { "cmd=SILENCE&context=m.context" } },
        { { "cmd=SILENCE&context=other" } },
        { { "cmd=SILENCE&hosts=m-host" } },
        { { "cmd=SILENCE&hosts=m-registry" } },
        { { "cmd=SILENCE&hosts=other" } },
        { { "cmd=SILENCE&hosts=*" } },
        { { "cmd=SILENCE&template=x" } },
        { { "cmd=SILENCE&alarm=m_alert&chart=other" } },
        { { "cmd=SILENCE&alarm=other&chart=m.chart" } },
        { { "cmd=DISABLE&alarm=m_alert&chart=m.chart&context=m.context&hosts=m-host" } },
        { { "cmd=SILENCE&alarm=other", "alarm=m_alert" } },
        { { "cmd=SILENCE&alarm=m_alert", "alarm=other" } },
        { { "cmd=DISABLE&alarm=x_alert", "chart=m.chart" } },
        { { "cmd=SILENCE ALL", "alarm=other" } },
        { { "cmd=DISABLE&alarm=other", "cmd=SILENCE ALL" } },
        { { "cmd=SILENCE ALL", "cmd=DISABLE" } },
        { { "cmd=SILENCE ALL", "cmd=RESET" } },
    };
    for(size_t i = 0; i < sizeof(states) / sizeof(states[0]); i++) {
        char *label = match_label(&states[i]);
        in_child(f, label, match_rows, &states[i]);
        free(label);
    }
    if(ferror(f) || fclose(f) != 0) die("cannot write", "silencers-match.tsv");
}

// ------------------------------------------------------------------------------------------------
// the DynCfg tables

// one case of a list: its place and its bytes
struct list_case {
    size_t n;
    char *bytes;
    size_t len;
    // an action's other fields
    char *id, *name;
    DYNCFG_CMDS cmd;
    bool has_payload;
};

// a field of the vectors' encoding, back into its bytes
static char *unescaped(const char *text, size_t *len) {
    char *bytes = mallocz(strlen(text) + 1);
    size_t n = 0;
    for(const char *s = text; *s;) {
        if(s[0] == '\\' && s[1] == '\\') {
            bytes[n++] = '\\';
            s += 2;
        }
        else if(s[0] == '\\' && s[1] == 'x' && isxdigit((unsigned char)s[2]) && isxdigit((unsigned char)s[3])) {
            char hex[3] = { s[2], s[3], '\0' };
            bytes[n++] = (char)strtoul(hex, NULL, 16);
            s += 4;
        }
        else
            bytes[n++] = *s++;
    }
    bytes[n] = '\0';
    if(len) *len = n;
    return bytes;
}

// Every case of a list, each in its own process: a line that is no comment, counted from 0.
static void each_case(FILE *f, const char *path, bool actions, void (*rows)(FILE *f, const void *arg)) {
    FILE *input = fopen(path, "r");
    if(!input) die("cannot read", path);
    char *line = NULL;
    size_t size = 0, n = 0;
    ssize_t len;
    while((len = getline(&line, &size, input)) > 0) {
        if(line[len - 1] == '\n')
            line[len - 1] = '\0';
        if(*line == '#')
            continue;
        struct list_case c = { .n = n++ };
        if(actions) {
            char *rest = line;
            char *id = strsep(&rest, "\t");
            char *action = rest ? strsep(&rest, "\t") : NULL;
            char *name = rest ? strsep(&rest, "\t") : NULL;
            if(!action || !name || !rest) die("an action without its four fields in", path);
            c.id = unescaped(id, NULL);
            if(!action_of(action, &c.cmd)) die("an unknown action in", path);
            c.name = strcmp(name, "-") == 0 ? NULL : unescaped(name, NULL);
            c.has_payload = strcmp(rest, "-") != 0;
            c.bytes = unescaped(c.has_payload ? rest : "", &c.len);
        }
        else
            c.bytes = unescaped(line, &c.len);
        char label[32];
        snprintf(label, sizeof(label), "%zu", c.n);
        in_child(f, label, rows, &c);
        freez(c.bytes);
        freez(c.id);
        freez(c.name);
    }
    free(line);
    fclose(input);
}

static void payload_rows(FILE *f, const void *arg) {
    const struct list_case *c = arg;
    static const struct {
        const char *name;
        unsigned flags;
    } modes[] = { { "required", JSONC_REQUIRED }, { "optional", JSONC_OPTIONAL } };
    for(size_t m = 0; m < 2; m++) {
        BUFFER *error = buffer_create(0, NULL);
        errno = 0;
        RRD_ALERT_PROTOTYPE *ap = health_prototype_payload_parse(c->bytes, c->len, error, "p_name", modes[m].flags);
        size_t rules = 0;
        for(RRD_ALERT_PROTOTYPE *t = ap; t; t = t->_internal.next)
            rules++;
        fprintf(f, "%zu\t%s\tparse\t%zu", c->n, modes[m].name, rules);
        field(f, buffer_tostring(error));
        if(ap) {
            BUFFER *wb = buffer_create(0, NULL);
            health_prototype_to_json(wb, ap, true);
            field(f, buffer_tostring(wb));
            buffer_free(wb);
        }
        else
            fputs("\t-", f);
        fputc('\t', f);
        messages(f);
        fputc('\n', f);
        size_t place = 0;
        for(RRD_ALERT_PROTOTYPE *t = ap; t; t = t->_internal.next) {
            fprintf(f, "%zu\t%s\trule\t%zu", c->n, modes[m].name, place++);
            oracle_rule_fields(f, t);
            fputc('\n', f);
        }
        buffer_free(error);
    }
}

static void userconfig_rows(FILE *f, const void *arg) {
    const struct list_case *c = arg;
    for(int job = 0; job < 2; job++)
        for(int exec = 0; exec < 2; exec++) {
            // what a host's first pass takes from the configuration, or nothing yet
            host.health.default_exec = exec ? string_dup(health_globals.config.default_exec) : NULL;
            BUFFER *payload = buffer_create(0, NULL);
            buffer_memcat(payload, c->bytes, c->len);
            errno = 0;
            fprintf(f, "%zu\t%s\t%d", c->n, job ? "job" : "template", exec);
            dyncfg_fields(f, job ? "health:alert:prototype:d_tpl" : "health:alert:prototype", DYNCFG_CMD_USERCONFIG,
                          "u_name", payload);
            fputc('\t', f);
            messages(f);
            fputc('\n', f);
            buffer_free(payload);
        }
}

static void action_rows(FILE *f, const void *arg) {
    const struct list_case *c = arg;
    BUFFER *payload = NULL;
    if(c->has_payload) {
        payload = buffer_create(0, NULL);
        buffer_memcat(payload, c->bytes, c->len);
    }
    fflush(oracle_calls);
    size_t calls_before = calls_size;
    errno = 0;
    fprintf(f, "%zu", c->n);
    dyncfg_fields(f, c->id, c->cmd, c->name, payload);
    fputc('\t', f);
    messages(f);

    // the calls: a call's fields separated by tabs, the calls by ` | `
    fflush(oracle_calls);
    fputc('\t', f);
    if(calls_size == calls_before)
        fputc('-', f);
    for(size_t i = calls_before; i < calls_size; i++) {
        if(calls_text[i] != '\n')
            fputc(calls_text[i], f);
        else if(i + 1 < calls_size)
            fputs(" | ", f);
    }

    fputc('\t', f);
    RRD_ALERT_PROTOTYPE *ap;
    dfe_start_read(health_globals.prototypes.dict, ap) {
        size_t rules = 0;
        for(RRD_ALERT_PROTOTYPE *t = ap; t; t = t->_internal.next)
            rules++;
        fprintf(f, "%s%s:%d:%zu", ap_dfe.counter ? " " : "", ap_dfe.name, ap->_internal.enabled ? 1 : 0, rules);
    }
    dfe_done(ap);
    fputc('\n', f);
}

static void dyncfg_tables(const char *dir, const char *records_path) {
    world_init(records_path);
    host_index();
    if(health_readfile("tests/corpus/dyncfg/base.conf", NULL, false) != 1) die("cannot read", "base.conf");

    FILE *f = table(dir, "payload.tsv", "the case, the mode, then `parse`, the rules (0: refused), the error text, the "
                                        "chain's hashed JSON, the records' messages; or `rule`, its place and a "
                                        "rule's fields as rules.tsv has them up to the hash; or the case and "
                                        "`signal <n>`");
    each_case(f, "tests/corpus/dyncfg/payloads.txt", false, payload_rows);
    if(ferror(f) || fclose(f) != 0) die("cannot write", "payload.tsv");

    f = table(dir, "userconfig.tsv", "the case, the node asked (template or job), localhost's default command is "
                                     "set, the code, the content type, the cache word, the expiry, the body, the "
                                     "records' messages; or the case and `signal <n>`");
    each_case(f, "tests/corpus/dyncfg/userconfig.txt", false, userconfig_rows);
    if(ferror(f) || fclose(f) != 0) die("cannot write", "userconfig.tsv");

    // the actions' store: two jobs added to the file's three names
    static const struct {
        const char *name, *path;
    } added[] = { { "d_dyn", "@tests/corpus/dyncfg/one.json" }, { "d_off", "@tests/corpus/dyncfg/off.json" } };
    for(size_t i = 0; i < 2; i++) {
        BUFFER *payload = payload_of(added[i].path);
        BUFFER *result = buffer_create(0, NULL);
        usec_t stop_monotonic_ut = 0;
        bool cancelled = false;
        int code = dyncfg_health_cb("oracle", "health:alert:prototype", DYNCFG_CMD_ADD, added[i].name, payload,
                                    &stop_monotonic_ut, &cancelled, result, HTTP_ACCESS_ALL, "oracle", NULL);
        if(code != 202 && code != 298) die("cannot add", added[i].name);
        buffer_free(payload);
        buffer_free(result);
    }
    records_read = lseek(records_fd, 0, SEEK_END);

    f = table(dir, "actions.tsv", "the case, the code, the content type, the cache word, the expiry, the body, the "
                                  "records' messages, the calls, the store after it; or the case and `signal <n>`");
    each_case(f, "tests/corpus/dyncfg/actions.txt", true, action_rows);
    if(ferror(f) || fclose(f) != 0) die("cannot write", "actions.tsv");
}

// ---------------------------------------------------------------------------------------------------------------------
// the badge: C's own web_buffer_svg.c, its static functions through tests/oracle/badge-splice.inc

double oracle_verdana11_width(const char *s);
size_t oracle_escape_xmlz(char *dst, const char *src, size_t len);
void oracle_calc_colorz(const char *color, char *final, size_t len, NETDATA_DOUBLE value);
const char *oracle_parse_color_argument(const char *arg, const char *def);
void oracle_buffer_svg(BUFFER *wb, const char *label, NETDATA_DOUBLE value, const char *units, const char *label_color,
                       const char *value_color, int precision, int scale, uint32_t options, int fixed_width_lbl,
                       int fixed_width_val, const char *text_color_lbl, const char *text_color_val);

static void badge_width_row(FILE *f, const char *text) {
    oracle_esc(f, text);
    fputc('\t', f);
    put_double(f, oracle_verdana11_width(text));
    fputc('\n', f);
}

static void badge_escape_row(FILE *f, const char *text, size_t limit) {
    char out[1024 + 1];
    size_t used = oracle_escape_xmlz(out, text, limit);
    oracle_esc(f, text);
    fprintf(f, "\t%zu\t", limit);
    oracle_esc(f, out);
    fprintf(f, "\t%zu\n", used);
}

// `n` bytes of `c`, then `tail`
static char *badge_run(char c, size_t n, const char *tail) {
    static char buf[8192];
    if(n + strlen(tail) >= sizeof(buf)) die("a text too long for", "badge_run");
    memset(buf, c, n);
    strcpy(buf + n, tail);
    return buf;
}

static void badge_svg_row(FILE *f, const char *label, NETDATA_DOUBLE value, const char *units, const char *label_color,
                          const char *value_color, int precision, int scale, uint32_t options, int width_lbl,
                          int width_val, const char *text_lbl, const char *text_val) {
    BUFFER *wb = buffer_create(0, NULL);
    oracle_buffer_svg(wb, label, value, units, label_color, value_color, precision, scale, options, width_lbl,
                      width_val, text_lbl, text_val);
    oracle_esc(f, label);
    fputc('\t', f);
    put_double(f, value);
    fputc('\t', f);
    oracle_esc(f, units);
    fputc('\t', f);
    oracle_esc(f, label_color);
    fputc('\t', f);
    oracle_esc(f, value_color);
    fprintf(f, "\t%d\t%d\t%u\t%d\t%d\t", precision, scale, (unsigned)options, width_lbl, width_val);
    oracle_esc(f, text_lbl);
    fputc('\t', f);
    oracle_esc(f, text_val);
    fprintf(f, "\t%d\t", (int)wb->content_type);
    oracle_esc(f, buffer_tostring(wb));
    fputc('\n', f);
    buffer_free(wb);
}

static void badge_tables(const char *dir) {
    // format_value_and_unit() with a precision: -1 is units.tsv's
    FILE *f = table(dir, "badge-format.tsv", "value (the double's bits, or nan), units, precision, text");
    static const int precisions[] = { -2, 0, 1, 2, 3, 7, 15, 17, 49, 50, 51, INT_MAX };
    // "5xx": a unit that starts with a digit takes the separator too (isalnum(), not isalpha())
    static const char *format_units[] = { "", "things", "%", "seconds", "minutes ago", "hours", "up/down", "null",
                                          "5xx" };
    for(size_t u = 0; u < sizeof(format_units) / sizeof(format_units[0]); u++)
        for(size_t p = 0; p < sizeof(precisions) / sizeof(precisions[0]); p++)
            for(size_t v = 0; v < sizeof(unit_values) / sizeof(unit_values[0]); v++) {
                char buf[100 + 1];
                char *text = format_value_and_unit(buf, 100, unit_values[v], format_units[u], precisions[p]);
                put_double(f, unit_values[v]);
                fputc('\t', f);
                oracle_esc(f, format_units[u]);
                fprintf(f, "\t%d\t", precisions[p]);
                oracle_esc(f, text);
                fputc('\n', f);
            }
    if(ferror(f) || fclose(f) != 0) die("cannot write", "badge-format.tsv");

    // verdana11_width() at the badge's font size
    f = table(dir, "badge-width.tsv", "text, the width (the double's bits)");
    for(int c = 1; c < 256; c++) {
        char one[2] = { (char)c, 0 };
        badge_width_row(f, one);
    }
    static const char *width_texts[] = {
        "", "-", "chart not found", "alarm not found", "cpu", "system.cpu", "used ram", "12.3 %", "1,234.5 MiB",
        "a b", "WWWW", "iiii", "il1|", "\xc3\xa9", "caf\xc3\xa9", "\xe2\x82\xac", "\xf0\x9f\x98\x80",
        "a\xc3\xa9\xe2\x82\xac\xf0\x9f\x98\x80z", "a\xc3", "\xc3", "\xe2\x82", "\x80", "\xbf\xbf", "a\x80z",
        "\xc3\xa9\xc3\xa9", "\xff\xfe", "\t", "a\tb", "\x7f", "0123456789", "%%", "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "abcdefghijklmnopqrstuvwxyz", "~!@#$%^&*()_+{}|:\"<>?`-=[]\\;',./",
    };
    for(size_t i = 0; i < sizeof(width_texts) / sizeof(width_texts[0]); i++)
        badge_width_row(f, width_texts[i]);
    badge_width_row(f, badge_run('m', 300, ""));
    badge_width_row(f, badge_run(' ', 50, "x"));
    if(ferror(f) || fclose(f) != 0) die("cannot write", "badge-width.tsv");

    // escape_xmlz() at the label's and the value's sizes
    f = table(dir, "badge-escape.tsv", "text, limit, the escaped text, the function's result");
    static const char *specials[] = { "&", "<", ">", "\"", "'", "\\", "a", "\xc3\xa9", "&&", "<>", "&x" };
    static const size_t limits[] = { 200, 100 };
    for(size_t l = 0; l < sizeof(limits) / sizeof(limits[0]); l++) {
        for(size_t s = 0; s < sizeof(specials) / sizeof(specials[0]); s++) {
            badge_escape_row(f, specials[s], limits[l]);
            for(size_t before = limits[l] - 7; before <= limits[l] + 1; before++)
                badge_escape_row(f, badge_run('a', before, specials[s]), limits[l]);
        }
        badge_escape_row(f, "", limits[l]);
        badge_escape_row(f, "a < b && c > \"d\" 'e' \\ f", limits[l]);
        badge_escape_row(f, badge_run('&', 60, ""), limits[l]);
        badge_escape_row(f, badge_run('\\', 250, ""), limits[l]);
    }
    if(ferror(f) || fclose(f) != 0) die("cannot write", "badge-escape.tsv");

    // calc_colorz(), then parse_color_argument() of its text as buffer_svg() asks, with the default 555
    f = table(dir, "badge-color.tsv", "expression, value (the double's bits, or nan), the chosen text, the color");
    static const char *expressions[] = {
        "red", "4c1", "#fff", "", "|", "red|", "|red", "red|green",
        "red>10", "red>=10", "red<10", "red<=10", "red=10", "red:10", "red!=10", "red!10", "red<>10", "red<)10",
        "red<}10", "red)10", "red}10", "red(10", "red{10", "red)=10", "red(=10", "red}=10", "red{=10", "red==10",
        "red>100", "red<-5", "red>-5", "red=-5", "red=0", "red>0", "red<0", "red!=0",
        "red>10|green", "red>10|yellow>5|green", "green<5|yellow<10|red", "red>10|yellow>5", "red<0|yellow<5|green<10",
        "red:null", "red=null", "red!=null", "red>null", "red<null", "red:", "red=", "red>", "red<", "red!",
        "red:null|green", "green|red:null", "red:null|yellow>5|green", "grey:null|green<10|red",
        "red>10.9", "red>abc", "red> 7", "red>7 ", "red>1e2", "red>0x10", "red>+5", "red>--5", "red>10abc",
        ">10", ">10|green", "=5", ":null", "|>10", "red>10|>5|green", "red>10||green",
        "red>5>10", "red>5<10", "red>10=5", "red>=<5", "red=!5", "red<=>5", "red>10!", "red!>5",
        "RED>10", "Red", "brightgreen>5|yellowgreen>0|orange", "lightgrey:null|blue", "gray|grey",
        "#ff0000>5|#00ff00", "ff0000>5|00ff00", "f00>5|0f0", "ff00>5", "abcdefg>5",
        "red>5|", "red>5||", "||red", "red|>5", "a>5|b>6|c>7|d>8|e>9|f>10|g>11|h>12",
        "red>9223372036854775807", "red>-9223372036854775808", "red>99999999999999999999",
        "re d>5", "red>5 |green", " red>5", "red >5", "red\t>5",
        "red>5|green:null", "green:null|red>5", "red>=5|yellow>=0|blue", "red<=5|yellow<=10|blue",
        "red!=5|green", "red<>5|green", "red:5|green", "red=5|yellow=10|green=0.5",
        // an entry followed by another, where an early stop and a fall-through differ: a null value against a null
        // threshold under any operator, a threshold read as an integer, and `null` only in lower case
        "red!=null|green", "red>null|green", "red<|green", "red>10.9|green", "red>1e2|green", "red>10abc|green",
        "red:NULL|green", "red=Null|green",
    };
    static const NETDATA_DOUBLE color_values[] = { NAN, INFINITY, -INFINITY, -5.0, -0.0, 0.0, 0.5, 5.0, 10.0, 10.5,
                                                   11.0, 100.0, 1e30 };
    char longs[3][600];
    for(int i = 0; i < 3; i++) {
        // a color of 255, 256 and 257 bytes before the operator, and a threshold of as many zeros before a 9
        memset(longs[i], 'c', 255 + i);
        strcpy(longs[i] + 255 + i, ">5|green");
    }
    char thresholds[3][600];
    for(int i = 0; i < 3; i++) {
        strcpy(thresholds[i], "red>");
        memset(thresholds[i] + 4, '0', 254 + i);
        strcpy(thresholds[i] + 4 + 254 + i, "9|green");
    }
    size_t fixed = sizeof(expressions) / sizeof(expressions[0]);
    for(size_t e = 0; e < fixed + 6; e++) {
        const char *expression = e < fixed ? expressions[e] : e < fixed + 3 ? longs[e - fixed] : thresholds[e - fixed - 3];
        for(size_t v = 0; v < sizeof(color_values) / sizeof(color_values[0]); v++) {
            char chosen[100 + 1];
            oracle_calc_colorz(expression, chosen, 100, color_values[v]);
            oracle_esc(f, expression);
            fputc('\t', f);
            put_double(f, color_values[v]);
            fputc('\t', f);
            oracle_esc(f, chosen);
            fputc('\t', f);
            oracle_esc(f, oracle_parse_color_argument(chosen, "555"));
            fputc('\n', f);
        }
    }
    if(ferror(f) || fclose(f) != 0) die("cannot write", "badge-color.tsv");

    // parse_color_argument()
    f = table(dir, "badge-color-arg.tsv", "argument (\\x00: none), default, result");
    static const char *arguments[] = {
        NULL, "", "brightgreen", "green", "yellow", "yellowgreen", "orange", "red", "blue", "grey", "gray", "lightgrey",
        "lightgray", "Red", "RED", "greenish", "gree", " red", "red ", "black", "white",
        "f", "ff", "fff", "ffff", "fffff", "ffffff", "fffffff", "ffffffff", "F", "FF", "FFF", "FFFF", "FFFFFF",
        "FFFFFFFF", "aBc", "aBcDeF", "aBcDeF12", "#fff", "#ffffff", "ggg", "fgf", "12g", "000", "000000", "123",
        "1234567", "12345678", "123456789", "abcdef1", "0x123", "a", "ab", "4c1", "97CA00", "e05d44", "007ec6",
        "1234567890123456789", "12345678901234567890", "123456789012345678901", "aaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaa", "re", "r", "\xc3\xa9\xc3\xa9", "fff ", " fff", "ff f", "ffg", "FfF",
    };
    static const char *defaults[] = { "555", "999", NULL };
    for(size_t d = 0; d < sizeof(defaults) / sizeof(defaults[0]); d++)
        for(size_t a = 0; a < sizeof(arguments) / sizeof(arguments[0]); a++) {
            oracle_esc(f, arguments[a]);
            fputc('\t', f);
            oracle_esc(f, defaults[d]);
            fputc('\t', f);
            oracle_esc(f, oracle_parse_color_argument(arguments[a], defaults[d]));
            fputc('\n', f);
        }
    if(ferror(f) || fclose(f) != 0) die("cannot write", "badge-color-arg.tsv");

    // buffer_svg()
    f = table(dir, "badge-svg.tsv", "label, value (the double's bits, or nan), units, label color, value color, "
                                    "precision, scale, options, the label's fixed width, the value's, the label's "
                                    "text color, the value's (\\x00: none), the content type's number, the body");
    static const int scales[] = { 100, 50, 0, -1, 99, 101, 125, 200, 1000, INT_MAX };
    for(size_t i = 0; i < sizeof(scales) / sizeof(scales[0]); i++) {
        badge_svg_row(f, "cpu", 12.3456, "%", NULL, NULL, -1, scales[i], 0, -1, -1, NULL, NULL);
        badge_svg_row(f, "cpu", 12.3456, "%", NULL, NULL, -1, scales[i], 0, 80, 120, NULL, NULL);
    }
    static const int widths[][2] = { { -1, -1 }, { 0, 5 }, { 5, 0 }, { 80, 120 }, { 1, 1 }, { -1, 50 }, { 50, -1 },
                                     { 0, 0 }, { 1000, 1 }, { INT_MAX, INT_MAX } };
    for(size_t i = 0; i < sizeof(widths) / sizeof(widths[0]); i++) {
        badge_svg_row(f, "used ram", 1234.5, "MiB", "blue", "orange", 1, 100, 0, widths[i][0], widths[i][1], NULL, NULL);
        badge_svg_row(f, "used ram", 1234.5, "MiB", "blue", "orange", 1, 150, 0, widths[i][0], widths[i][1], "fff", "000");
    }
    static const NETDATA_DOUBLE svg_values[] = { NAN, INFINITY, -INFINITY, 0.0, -0.0, 0.5, -0.5, 1.0, -12.5, 99.95,
                                                 1000.0, 1e9, 1e30 };
    static const char *svg_units[] = { "", "%", "things", "seconds", "up/down", "null", "percentage" };
    for(size_t v = 0; v < sizeof(svg_values) / sizeof(svg_values[0]); v++)
        for(size_t u = 0; u < sizeof(svg_units) / sizeof(svg_units[0]); u++) {
            badge_svg_row(f, "label", svg_values[v], svg_units[u], NULL, NULL, -1, 100, 0, -1, -1, NULL, NULL);
            // RRDR_OPTION_DISPLAY_ABS
            badge_svg_row(f, "label", svg_values[v], svg_units[u], NULL, "red<0|green", 2, 100,
                          RRDR_OPTION_DISPLAY_ABS, -1, -1, NULL, NULL);
        }
    static const char *svg_colors[] = { NULL, "", "red", "#fff", "4c1", "nocolor", "f", "ffffffff", "red>5|green",
                                        "grey:null|blue" };
    for(size_t a = 0; a < sizeof(svg_colors) / sizeof(svg_colors[0]); a++)
        for(size_t b = 0; b < sizeof(svg_colors) / sizeof(svg_colors[0]); b++)
            badge_svg_row(f, "colors", 7.0, "x", svg_colors[a], svg_colors[b], 0, 100, 0, -1, -1, svg_colors[b],
                          svg_colors[a]);
    static const char *svg_labels[] = { "", " ", "a < b & c > d", "\"quoted\" 'label'", "back\\slash",
                                        "caf\xc3\xa9 \xe2\x82\xac \xf0\x9f\x98\x80", "chart not found",
                                        "alarm not found", "\x80\xff" };
    for(size_t l = 0; l < sizeof(svg_labels) / sizeof(svg_labels[0]); l++) {
        badge_svg_row(f, svg_labels[l], 1.0, svg_labels[l], NULL, NULL, -1, 100, 0, -1, -1, NULL, NULL);
        badge_svg_row(f, svg_labels[l], NAN, "", NULL, NULL, -1, 100, 0, -1, -1, NULL, NULL);
    }
    badge_svg_row(f, badge_run('L', 199, "&"), 1.0, "u", NULL, NULL, -1, 100, 0, -1, -1, NULL, NULL);
    badge_svg_row(f, badge_run('L', 5000, ""), 1.0, "u", NULL, NULL, -1, 100, 0, -1, -1, NULL, NULL);
    char long_units[400];
    strcpy(long_units, badge_run('u', 300, ""));
    badge_svg_row(f, "long units", 1.0, long_units, NULL, NULL, -1, 100, 0, -1, -1, NULL, NULL);
    static const int svg_precisions[] = { -2, -1, 0, 1, 5, 50, 51, INT_MAX, INT_MIN };
    for(size_t p = 0; p < sizeof(svg_precisions) / sizeof(svg_precisions[0]); p++)
        badge_svg_row(f, "precision", 1234.56789, "things", NULL, NULL, svg_precisions[p], 100, 0, -1, -1, NULL, NULL);
    if(ferror(f) || fclose(f) != 0) die("cannot write", "badge-svg.tsv");
}


int main(int argc, char **argv) {
    if(argc == 3 && strcmp(argv[1], "tables") == 0) {
        tables(argv[2]);
        return 0;
    }
    if(argc == 4 && strcmp(argv[1], "decide") == 0) {
        decide(argv[2], argv[3]);
        return 0;
    }
    if(argc == 4 && strcmp(argv[1], "silencers") == 0) {
        silencers_tables(argv[2], argv[3]);
        return 0;
    }
    if(argc == 4 && strcmp(argv[1], "dyncfg") == 0) {
        dyncfg_tables(argv[2], argv[3]);
        return 0;
    }
    if(argc == 3 && strcmp(argv[1], "badge") == 0) {
        badge_tables(argv[2]);
        return 0;
    }
    if(argc == 5 && strcmp(argv[1], "scenario") == 0) {
        run_scenario(argv[2], argv[3], argv[4]);
        return 0;
    }
    fprintf(stdout, "usage: %s tables <directory> | decide <directory> <records file> | "
                    "silencers <directory> <records file> | dyncfg <directory> <records file> | badge <directory> | "
                    "scenario <loop.tsv> <scenario file> <records file>\n",
            argv[0]);
    return 1;
}
