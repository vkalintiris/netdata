// SPDX-License-Identifier: GPL-3.0-or-later

#include "health_internals.h"
#include "health-alert-entry.h"

// the queue of executed alarm notifications that haven't been waited for yet
static ALARM_ENTRY *alarm_notifications_in_progress = NULL;

// how often the notification wait loop wakes up to re-check shutdown and the deadline
#define HEALTH_NOTIFICATION_WAIT_SLICE_MS 1000

struct health_raised_summary {
    RRDHOST *host;
    DICTIONARY *rrdcalc_dict;

    struct {
        size_t size;
        size_t used;
        const DICTIONARY_ITEM **array;
    } active_alerts;
};

void health_alarm_wait_for_execution(ALARM_ENTRY *ae) {
    // this has to ALWAYS remove the given alarm entry from the queue

    int code = 0;

    bool in_process = ae->flags & HEALTH_ENTRY_FLAG_EXEC_IN_PROGRESS;
    if (!in_process) {
        nd_log(NDLS_DAEMON, NDLP_ERR, "attempted to wait for the execution of alert that has not an execution in progress");
        code = 128;
        goto cleanup;
    }

    if(!ae->popen_instance) {
        nd_log(NDLS_DAEMON, NDLP_ERR, "attempted to wait for the execution of alert that has not spawn a notification");
        code = 128;
        goto cleanup;
    }

    // bound the wait so a hung notification process (seen on Windows, where msys children can
    // wedge during startup) cannot block the single health thread - and with it all health
    // evaluation. Each slice is always bounded; the overall wait is bounded only when a non-zero
    // timeout is configured. timeout == 0 means "wait forever" - the loop then breaks only on
    // child exit or shutdown. The deadline is monotonic, so a wall-clock jump cannot extend it.
    int32_t timeout = health_globals.config.notification_execution_timeout_seconds;
    usec_t deadline_ut = now_monotonic_usec() + (usec_t)timeout * USEC_PER_SEC;

    while(true) {
        SPAWN_TIMEDWAIT_RESULT r = spawn_popen_timedwait(ae->popen_instance, HEALTH_NOTIFICATION_WAIT_SLICE_MS, &code);
        if(r == SPAWN_TIMEDWAIT_EXITED)
            break;

        // RUNNING: keep waiting unless we should stop. ERROR: the wait broke and must never be
        // looped on (it would spin forever at timeout == 0), so always fall through to the kill.
        // re-check shutdown every slice, so a slow notification cannot block agent exit.
        bool deadline_reached = (timeout > 0 && now_monotonic_usec() >= deadline_ut);
        if(r == SPAWN_TIMEDWAIT_ERROR || unlikely(!service_running(SERVICE_HEALTH)) || deadline_reached) {
            nd_log(NDLS_DAEMON, NDLP_ERR,
                   "HEALTH: alert notification '%s' (pid %d) %s - killing it",
                   ae_name(ae), (int)spawn_popen_pid(ae->popen_instance),
                   (r == SPAWN_TIMEDWAIT_ERROR) ? "could not be waited for (status channel error)"
                                                : "is still running past its execution timeout");

            spawn_popen_kill(ae->popen_instance, 0);
            code = 128;
            break;
        }
    }
    ae->popen_instance = NULL;
    netdata_log_debug(D_HEALTH, "done executing command - returned with code %d", code);

cleanup:
    ae->exec_code = code;
    ae->flags &= ~HEALTH_ENTRY_FLAG_EXEC_IN_PROGRESS;

    if(ae->exec_code != 0)
        ae->flags |= HEALTH_ENTRY_FLAG_EXEC_FAILED;

    if (in_process)
        unlink_alarm_notify_in_progress(ae);
}

void wait_for_all_notifications_to_finish_before_allowing_health_to_be_cleaned_up(void) {
    ALARM_ENTRY *ae;
    while (NULL != (ae = alarm_notifications_in_progress)) {
        if(unlikely(!service_running(SERVICE_HEALTH)))
            break;

        health_alarm_wait_for_execution(ae);
    }
}

void unlink_alarm_notify_in_progress(ALARM_ENTRY *ae)
{
    fatal_assert(ae->prev_in_progress || ae->next_in_progress);
    DOUBLE_LINKED_LIST_REMOVE_ITEM_UNSAFE(alarm_notifications_in_progress, ae, prev_in_progress, next_in_progress);
}

static inline void enqueue_alarm_notify_in_progress(ALARM_ENTRY *ae)
{
    fatal_assert(!ae->prev_in_progress && !ae->next_in_progress);
    DOUBLE_LINKED_LIST_APPEND_ITEM_UNSAFE(alarm_notifications_in_progress, ae, prev_in_progress, next_in_progress);
}

/*
 * The notification program is executed directly, not through a shell.
 *
 * Windows has no shell once the bundled MSYS2 root is gone, and going through
 * `sh -c` also meant every argument had to survive shell quoting: a single quote or
 * a `$` in an alert's text used to be escaped or replaced before the notifier saw
 * it. Passing an argv array removes both problems, so the notifier receives each
 * value exactly as the alert carries it.
 */

#define HEALTH_NOTIFY_MAX_ARGS 40

// The program plus the 33 positional arguments.
#define HEALTH_NOTIFY_ARGC 34

typedef struct health_notify_command {
    const char *argv[HEALTH_NOTIFY_MAX_ARGS];
    size_t argc;

    // Backing storage for the arguments that have to be formatted.
    char unique_id[UINT64_MAX_LENGTH];
    char alarm_id[UINT64_MAX_LENGTH];
    char alarm_event_id[UINT64_MAX_LENGTH];
    char when[UINT64_MAX_LENGTH];
    // NETDATA_DOUBLE_FORMAT_ZERO is "%0.0f", so DBL_MAX renders as ~309 digits. A
    // short buffer would not overflow - snprintfz is bounded - but it would hand the
    // notifier a silently truncated, numerically wrong value.
    char new_value[512];
    char old_value[512];
    char duration[UINT64_MAX_LENGTH];
    char non_clear_duration[UINT64_MAX_LENGTH];
    char n_warn[UINT64_MAX_LENGTH];
    char n_crit[UINT64_MAX_LENGTH];
    char transition_id[UUID_STR_LEN];
} HEALTH_NOTIFY_COMMAND;

// An empty string rather than NULL: argv must keep its fixed positions.
static inline void health_notify_add(HEALTH_NOTIFY_COMMAND *cmd, const char *value) {
    if(cmd->argc >= HEALTH_NOTIFY_MAX_ARGS - 1)
        return;

    cmd->argv[cmd->argc++] = value ? value : "";
}

// The argv entries point into `cmd` and at strings the caller owns. That is safe
// because spawn_popen_run_argv() copies everything before it returns - it blocks on
// the spawn server's status report. A fire-and-forget spawn would break this.
static bool prepare_command(HEALTH_NOTIFY_COMMAND *cmd,
                            const char *exec,
                            const char *recipient,
                            const char *registry_hostname,
                            uint32_t unique_id,
                            uint32_t alarm_id,
                            uint32_t alarm_event_id,
                            uint32_t when,
                            const char *alert_name,
                            const char *alert_chart_name,
                            const char *new_status,
                            const char *old_status,
                            NETDATA_DOUBLE new_value,
                            NETDATA_DOUBLE old_value,
                            const char *alert_source,
                            uint32_t duration,
                            uint32_t non_clear_duration,
                            const char *alert_units,
                            const char *alert_info,
                            const char *new_value_string,
                            const char *old_value_string,
                            const char *source,
                            const char *error_msg,
                            int n_warn,
                            int n_crit,
                            const char *warn_alarms,
                            const char *crit_alarms,
                            const char *classification,
                            const char *edit_command,
                            const char *machine_guid,
                            nd_uuid_t *transition_id,
                            const char *summary,
                            const char *context,
                            const char *component,
                            const char *type
) {
    if(!exec || !*exec)
        return false;

    memset(cmd, 0, sizeof(*cmd));

    snprintfz(cmd->unique_id, sizeof(cmd->unique_id), "%u", unique_id);
    snprintfz(cmd->alarm_id, sizeof(cmd->alarm_id), "%u", alarm_id);
    snprintfz(cmd->alarm_event_id, sizeof(cmd->alarm_event_id), "%u", alarm_event_id);
    snprintfz(cmd->when, sizeof(cmd->when), "%u", when);
    snprintfz(cmd->new_value, sizeof(cmd->new_value), NETDATA_DOUBLE_FORMAT_ZERO, new_value);
    snprintfz(cmd->old_value, sizeof(cmd->old_value), NETDATA_DOUBLE_FORMAT_ZERO, old_value);
    snprintfz(cmd->duration, sizeof(cmd->duration), "%u", duration);
    snprintfz(cmd->non_clear_duration, sizeof(cmd->non_clear_duration), "%u", non_clear_duration);
    snprintfz(cmd->n_warn, sizeof(cmd->n_warn), "%d", n_warn);
    snprintfz(cmd->n_crit, sizeof(cmd->n_crit), "%d", n_crit);
    uuid_unparse_lower(*transition_id, cmd->transition_id);

    // argv[0] is the program; the 33 arguments follow in their documented order.
    health_notify_add(cmd, exec);
    health_notify_add(cmd, recipient);
    health_notify_add(cmd, registry_hostname);
    health_notify_add(cmd, cmd->unique_id);
    health_notify_add(cmd, cmd->alarm_id);
    health_notify_add(cmd, cmd->alarm_event_id);
    health_notify_add(cmd, cmd->when);
    health_notify_add(cmd, alert_name);
    health_notify_add(cmd, alert_chart_name);
    health_notify_add(cmd, new_status);
    health_notify_add(cmd, old_status);
    health_notify_add(cmd, cmd->new_value);
    health_notify_add(cmd, cmd->old_value);
    health_notify_add(cmd, alert_source);
    health_notify_add(cmd, cmd->duration);
    health_notify_add(cmd, cmd->non_clear_duration);
    health_notify_add(cmd, alert_units);
    health_notify_add(cmd, alert_info);
    health_notify_add(cmd, new_value_string);
    health_notify_add(cmd, old_value_string);
    health_notify_add(cmd, source);
    health_notify_add(cmd, error_msg);
    health_notify_add(cmd, cmd->n_warn);
    health_notify_add(cmd, cmd->n_crit);
    health_notify_add(cmd, warn_alarms);
    health_notify_add(cmd, crit_alarms);
    health_notify_add(cmd, classification);
    health_notify_add(cmd, edit_command);
    health_notify_add(cmd, machine_guid);
    health_notify_add(cmd, cmd->transition_id);
    health_notify_add(cmd, summary);
    health_notify_add(cmd, context);
    health_notify_add(cmd, component);
    health_notify_add(cmd, type);

    cmd->argv[cmd->argc] = NULL;

    // The count is a contract with every notification program (33 arguments plus the
    // program itself). Adding a field without extending HEALTH_NOTIFY_MAX_ARGS would
    // otherwise truncate argv silently.
    if(cmd->argc != HEALTH_NOTIFY_ARGC) {
        netdata_log_error("Notification program arguments: expected %d, prepared %zu.",
                          HEALTH_NOTIFY_ARGC, cmd->argc);
        return false;
    }

    return true;
}

static inline int compare_raised_alerts(const void *a, const void *b) {
    const DICTIONARY_ITEM *item1 = *(const DICTIONARY_ITEM **)a;
    const DICTIONARY_ITEM *item2 = *(const DICTIONARY_ITEM **)b;

    RRDCALC *rc1 = dictionary_acquired_item_value(item1);
    RRDCALC *rc2 = dictionary_acquired_item_value(item2);

    return (rc1->last_status_change < rc2->last_status_change) -
           (rc1->last_status_change > rc2->last_status_change);
}

static void health_raised_summary_add_alert(struct health_raised_summary *hrm, const DICTIONARY_ITEM  *item) {
    if(hrm->active_alerts.used >= hrm->active_alerts.size) {
        if(hrm->active_alerts.size == 0)
            hrm->active_alerts.size = 2;

        hrm->active_alerts.size *= 2;
        hrm->active_alerts.array = reallocz(hrm->active_alerts.array, sizeof(const DICTIONARY_ITEM *) * hrm->active_alerts.size);
    }

    hrm->active_alerts.array[hrm->active_alerts.used++] = dictionary_acquired_item_dup(hrm->rrdcalc_dict, item);
}

void alerts_raised_summary_free(struct health_raised_summary *hrm) {
    for(size_t i = 0; i < hrm->active_alerts.used ;i++)
        dictionary_acquired_item_release(hrm->rrdcalc_dict, hrm->active_alerts.array[i]);

    freez(hrm->active_alerts.array);
    freez(hrm);
}

struct health_raised_summary *alerts_raised_summary_create(RRDHOST *host) {
    struct health_raised_summary *hrm = callocz(1, sizeof(*hrm));
    hrm->rrdcalc_dict = host->rrdcalc_root_index;
    hrm->host = host;
    return hrm;
}

void alerts_raised_summary_populate(struct health_raised_summary *hrm) {
    RRDCALC *rc;
    foreach_rrdcalc_in_rrdhost_read(hrm->host, rc) {
        RRDSET *st = rrdcalc_rrdset_read_lock(rc);
        if(unlikely(!st)) continue;
        bool collected = st->last_collected_time.tv_sec;
        rrdcalc_rrdset_read_unlock(st);
        if(unlikely(!collected)) continue;
        health_raised_summary_add_alert(hrm, rc_dfe.item);
    }
    foreach_rrdcalc_in_rrdhost_done(rc);

    if (hrm->active_alerts.used > 1)
        qsort(hrm->active_alerts.array, hrm->active_alerts.used, sizeof(const DICTIONARY_ITEM *), compare_raised_alerts);
}

static size_t
health_raised_summary_entries(struct health_raised_summary *hrm, BUFFER *dst, ALARM_ENTRY *ae, RRDCALC_STATUS status) {
    buffer_flush(dst);

    size_t count = 0;
    for(size_t i = 0; i < hrm->active_alerts.used ;i++) {
        RRDCALC *rc = dictionary_acquired_item_value(hrm->active_alerts.array[i]);
        if(rc->status != status) continue;
        if(rc->id == ae->alarm_id) continue;

        count++;
        if(buffer_strlen(dst)) buffer_putc(dst, ',');
        buffer_sprintf(dst, "%s=%" PRId64, string2str(rc->config.name), (int64_t)rc->last_status_change);
    }

    return count;
}

static const char *health_raised_summary_my_expression_source(struct health_raised_summary *hrm, ALARM_ENTRY *ae) {
    for(size_t i = 0; i < hrm->active_alerts.used ;i++) {
        RRDCALC *rc = dictionary_acquired_item_value(hrm->active_alerts.array[i]);
        if(rc->id != ae->alarm_id) continue;

        if(rc->status == RRDCALC_STATUS_CRITICAL)
            return expression_source(rc->config.critical);
        else
            return expression_source(rc->config.warning);
    }

    return "";
}

static const char *health_raised_summary_my_expression_error(struct health_raised_summary *hrm, ALARM_ENTRY *ae) {
    for(size_t i = 0; i < hrm->active_alerts.used ;i++) {
        RRDCALC *rc = dictionary_acquired_item_value(hrm->active_alerts.array[i]);
        if(rc->id != ae->alarm_id) continue;

        if(rc->status == RRDCALC_STATUS_CRITICAL)
            return expression_error_msg(rc->config.critical);
        else
            return expression_error_msg(rc->config.warning);
    }

    return "";
}

void health_send_notification(RRDHOST *host, ALARM_ENTRY *ae, struct health_raised_summary *hrm) {
    netdata_log_debug(D_HEALTH, "Health alarm '%s.%s' = " NETDATA_DOUBLE_FORMAT_AUTO " - changed status from %s to %s",
                      ae->chart?ae_chart_id(ae):"NOCHART", ae_name(ae),
                      ae->new_value,
                      rrdcalc_status2string(ae->old_status),
                      rrdcalc_status2string(ae->new_status)
    );

    ae->flags |= HEALTH_ENTRY_FLAG_PROCESSED;

    if(unlikely(ae->new_status < RRDCALC_STATUS_CLEAR)) {
        // do not send notifications for internal statuses
        netdata_log_debug(D_HEALTH, "Health not sending notification for alarm '%s.%s' status %s (internal statuses)", ae_chart_id(ae), ae_name(ae), rrdcalc_status2string(ae->new_status));
        goto done;
    }

    if(unlikely(ae->new_status <= RRDCALC_STATUS_CLEAR && (ae->flags & HEALTH_ENTRY_FLAG_NO_CLEAR_NOTIFICATION))) {
        // do not send notifications for disabled statuses

        nd_log(NDLS_DAEMON, NDLP_DEBUG,
               "[%s]: Health not sending notification for alarm '%s.%s' status %s (it has no-clear-notification enabled)",
               rrdhost_hostname(host), ae_chart_id(ae), ae_name(ae), rrdcalc_status2string(ae->new_status));

        // mark it as run, so that we will send the same alarm if it happens again
        goto done;
    }

    // find the previous notification for the same alarm
    // which we have run the exec script
    // exception: alarms with HEALTH_ENTRY_FLAG_NO_CLEAR_NOTIFICATION set
    RRDCALC_STATUS last_executed_status = -3;
    if(likely(!(ae->flags & HEALTH_ENTRY_FLAG_NO_CLEAR_NOTIFICATION))) {
        int ret = sql_health_get_last_executed_event(host, ae, &last_executed_status);

        if (likely(ret == 1)) {
            // we have executed this alarm notification in the past
            if(last_executed_status == ae->new_status && !(ae->flags & HEALTH_ENTRY_FLAG_IS_REPEATING)) {
                // don't send the notification for the same status again
                nd_log(NDLS_DAEMON, NDLP_DEBUG,
                       "[%s]: Health not sending again notification for alarm '%s.%s' status %s",
                       rrdhost_hostname(host), ae_chart_id(ae), ae_name(ae),
                       rrdcalc_status2string(ae->new_status));
                goto done;
            }
        }
        else {
            // we have not executed this alarm notification in the past
            // so, don't send CLEAR notifications
            if(unlikely(ae->new_status == RRDCALC_STATUS_CLEAR)) {
                if((!(ae->flags & HEALTH_ENTRY_RUN_ONCE)) || (ae->flags & HEALTH_ENTRY_RUN_ONCE && ae->old_status < RRDCALC_STATUS_RAISED) ) {
                    netdata_log_debug(D_HEALTH, "Health not sending notification for first initialization of alarm '%s.%s' status %s"
                                      , ae_chart_id(ae), ae_name(ae), rrdcalc_status2string(ae->new_status));
                    goto done;
                }
            }
        }
    }

    // Check if alarm notifications are silenced
    if (ae->flags & HEALTH_ENTRY_FLAG_SILENCED) {
        nd_log(NDLS_DAEMON, NDLP_DEBUG,
               "[%s]: Health not sending notification for alarm '%s.%s' status %s "
               "(command API has disabled notifications)",
               rrdhost_hostname(host), ae_chart_id(ae), ae_name(ae), rrdcalc_status2string(ae->new_status));
        goto done;
    }

    nd_log(NDLS_DAEMON, NDLP_DEBUG,
           "[%s]: Sending notification for alarm '%s.%s' status %s.",
           rrdhost_hostname(host), ae_chart_id(ae), ae_name(ae), rrdcalc_status2string(ae->new_status));

    const char *exec      = (ae->exec)      ? ae_exec(ae)      : string2str(host->health.default_exec);
    const char *recipient = (ae->recipient) ? ae_recipient(ae) : string2str(host->health.default_recipient);

    char *edit_command = ae->source ? health_edit_command_from_source(ae_source(ae)) : strdupz("UNKNOWN=0=UNKNOWN");

    BUFFER *warn_alarms = buffer_create(1024, &netdata_buffers_statistics.buffers_health);
    BUFFER *crit_alarms = buffer_create(1024, &netdata_buffers_statistics.buffers_health);

    size_t n_warn = health_raised_summary_entries(hrm, warn_alarms, ae, RRDCALC_STATUS_WARNING);
    size_t n_crit = health_raised_summary_entries(hrm, crit_alarms, ae, RRDCALC_STATUS_CRITICAL);

    HEALTH_NOTIFY_COMMAND cmd;
    bool ok = prepare_command(&cmd,
                              exec,
                              recipient,
                              rrdhost_registry_hostname(host),
                              ae->unique_id,
                              ae->alarm_id,
                              ae->alarm_event_id,
                              (unsigned long)ae->when,
                              ae_name(ae),
                              ae->chart?ae_chart_id(ae):"NOCHART",
                              rrdcalc_status2string(ae->new_status),
                              rrdcalc_status2string(ae->old_status),
                              ae->new_value,
                              ae->old_value,
                              ae->source?ae_source(ae):"UNKNOWN",
                              nd_duration_to_uint32_saturating(ae->duration),
                              nd_duration_to_uint32_saturating(
                                  (ae->flags & HEALTH_ENTRY_FLAG_IS_REPEATING && ae->new_status >= RRDCALC_STATUS_WARNING) ?
                                      ae->duration : ae->non_clear_duration),
                              ae_units(ae),
                              ae_info(ae),
                              ae_new_value_string(ae),
                              ae_old_value_string(ae),
                              health_raised_summary_my_expression_source(hrm, ae),
                              health_raised_summary_my_expression_error(hrm, ae),
                              n_warn,
                              n_crit,
                              buffer_tostring(warn_alarms),
                              buffer_tostring(crit_alarms),
                              ae->classification?ae_classification(ae):"Unknown",
                              edit_command,
                              host->machine_guid,
                              &ae->transition_id,
                              host->health.use_summary_for_notifications && ae->summary?ae_summary(ae):ae_name(ae),
                              string2str(ae->chart_context),
                              string2str(ae->component),
                              string2str(ae->type)
    );

    if (ok) {
        ae->flags |= HEALTH_ENTRY_FLAG_EXEC_RUN;
        ae->exec_run_timestamp = now_realtime_sec(); /* will be updated by real time after spawning */

        netdata_log_debug(D_HEALTH, "executing notification program '%s'", cmd.argv[0]);
        ae->popen_instance = spawn_popen_run_argv(cmd.argv);
        if(ae->popen_instance) {
            ae->flags |= HEALTH_ENTRY_FLAG_EXEC_IN_PROGRESS;
            enqueue_alarm_notify_in_progress(ae);
        }
        else
            netdata_log_error(
                "Failed to execute alarm notification program '%s'. It is executed directly, "
                "not through a shell, so a script must start with an interpreter line (#!).",
                cmd.argv[0]);

        health_alarm_log_save(host, ae, false);
    }
    else
        netdata_log_error("Failed to prepare the notification program arguments");

    buffer_free(warn_alarms);
    buffer_free(crit_alarms);
    freez(edit_command);

    return; //health_alarm_wait_for_execution
done:
    health_alarm_log_save(host, ae, false);
}

void health_alarm_log_process_to_send_notifications(RRDHOST *host, struct health_raised_summary *hrm) {
    time_t now = now_realtime_sec();

    rw_spinlock_read_lock(&host->health_log.spinlock);

    uint32_t first_waiting = (host->health_log.alarms)?host->health_log.alarms->unique_id:0;

    for(ALARM_ENTRY *ae = host->health_log.alarms; ae && ae->unique_id >= host->health_last_processed_id; ae = ae->next) {
        if(unlikely(
                !(ae->flags & HEALTH_ENTRY_FLAG_PROCESSED) &&
                !(ae->flags & HEALTH_ENTRY_FLAG_UPDATED)
                    )) {
            if(unlikely(ae->unique_id < first_waiting))
                first_waiting = ae->unique_id;

            if(likely(now >= ae->delay_up_to_timestamp))
                health_send_notification(host, ae, hrm);
        }
    }

    rw_spinlock_read_unlock(&host->health_log.spinlock);

    // remember this for the next iteration
    host->health_last_processed_id = first_waiting;

    //delete those that are updated, no in progress execution, and is not repeating
    rw_spinlock_write_lock(&host->health_log.spinlock);

    ALARM_ENTRY *ae = host->health_log.alarms;
    while(ae) {
        ALARM_ENTRY *next = ae->next; // set it here, for the next iteration

        if((likely(!(ae->flags & HEALTH_ENTRY_FLAG_IS_REPEATING)) &&
             (ae->flags & HEALTH_ENTRY_FLAG_UPDATED) &&
             (ae->flags & HEALTH_ENTRY_FLAG_SAVED) &&
             !(ae->flags & HEALTH_ENTRY_FLAG_EXEC_IN_PROGRESS))
            ||
            ((ae->new_status == RRDCALC_STATUS_REMOVED) &&
             (ae->flags & HEALTH_ENTRY_FLAG_SAVED) &&
             (nd_time_t_add_compare(ae->when, 86400, now_realtime_sec()) < 0)))
        {
            DOUBLE_LINKED_LIST_REMOVE_ITEM_UNSAFE(host->health_log.alarms, ae, prev, next);
            health_alarm_log_free_one_nochecks_nounlink(ae);
        }

        ae = next;
    }

    rw_spinlock_write_unlock(&host->health_log.spinlock);
}
