// SPDX-License-Identifier: GPL-3.0-or-later

#include "health.h"
#include "health_internals.h"

/*
 * Which program dispatches alert notifications.
 *
 * A build with a Rust toolchain installs the native dispatcher `alarm-notify`; a
 * build without one installs the shell implementation `alarm-notify.sh`. Exactly one
 * of the two is present, so the program is chosen by looking. An explicitly
 * configured path always wins, with one exception: a configuration left over from an
 * older installation may still name `alarm-notify.sh` after an upgrade replaced it
 * with the native dispatcher, and silently sending no notifications would be the
 * worst possible outcome - so the sibling program is used instead, loudly.
 */
/*
 * Can this path be run as the notification program?
 *
 * POSIX answers the question directly. Windows cannot: the C runtime's _access()
 * understands only existence and the read-only attribute, and rejects the execute
 * mode outright - asking for X_OK there fails for every file, executable or not.
 * Existence is the best answer available, and it is enough, because the installer
 * places exactly one notifier.
 */
bool health_notification_program_is_usable(const char *path) {
    if(!path || !*path)
        return false;

#if defined(OS_WINDOWS)
    return access(path, R_OK) == 0;
#else
    return access(path, X_OK) == 0;
#endif
}

void health_notification_program_default(char *dst, size_t dst_size) {
    char native[FILENAME_MAX + 1];
#if defined(OS_WINDOWS)
    snprintfz(native, FILENAME_MAX, "%s/alarm-notify.exe", netdata_configured_primary_plugins_dir);
#else
    snprintfz(native, FILENAME_MAX, "%s/alarm-notify", netdata_configured_primary_plugins_dir);
#endif

    if(health_notification_program_is_usable(native)) {
        snprintfz(dst, dst_size, "%s", native);
        return;
    }

    snprintfz(dst, dst_size, "%s/alarm-notify.sh", netdata_configured_primary_plugins_dir);
}

static const char *health_notification_program_configured(void) {
    char filename[FILENAME_MAX + 1];
    health_notification_program_default(filename, sizeof(filename));

    const char *configured =
        inicfg_get_filename(&netdata_config, CONFIG_SECTION_HEALTH, "script to execute on alarm", filename);

    if(!configured || !*configured || health_notification_program_is_usable(configured))
        return configured;

    // The configured program is unusable. If it is the shell notifier this build no
    // longer ships, use the native one next to it rather than notify nobody.
    const char *base = strrchr(configured, '/');
#if defined(OS_WINDOWS)
    const char *base_win = strrchr(configured, '\\');
    if(base_win && (!base || base_win > base))
        base = base_win;
#endif
    if(base && strcmp(base + 1, "alarm-notify.sh") == 0 &&
        health_notification_program_is_usable(filename)) {
        nd_log(NDLS_DAEMON, NDLP_WARNING,
               "HEALTH: [health].'script to execute on alarm' is '%s', which cannot be run. "
               "Using '%s' instead. Please update netdata.conf.",
               configured, filename);
        return inicfg_set(&netdata_config, CONFIG_SECTION_HEALTH, "script to execute on alarm", filename);
    }

    nd_log(NDLS_DAEMON, NDLP_ERR,
           "HEALTH: [health].'script to execute on alarm' is '%s', which cannot be executed. "
           "Alert notifications will not be sent.",
           configured);
    return configured;
}

struct health_plugin_globals health_globals = {
    .initialization = {
        .spinlock = SPINLOCK_INITIALIZER,
        .done = false,
    },
    .config = {
        .enabled = true,
        .stock_enabled = true,
        .use_summary_for_notifications = true,

        .health_log_entries_max = HEALTH_LOG_ENTRIES_DEFAULT,
        .health_log_retention_s = HEALTH_LOG_RETENTION_DEFAULT,

        .default_warn_repeat_every = 0,
        .default_crit_repeat_every = 0,

        .run_at_least_every_seconds = 10,
        .postpone_alarms_during_hibernation_for_seconds = 60,
        .notification_execution_timeout_seconds = 120,
    },
    .prototypes = {
        .dict = NULL,
        .registering = false,
    }
};

bool health_plugin_enabled(void) {
    return health_globals.config.enabled;
}

void health_plugin_disable(void) {
    health_globals.config.enabled = false;
}


void health_load_config_defaults(void) {
    static bool done = false;
    if(done) return;
    done = true;

    health_globals.config.enabled =
        inicfg_get_boolean(&netdata_config, CONFIG_SECTION_HEALTH,
                           "enabled",
                           health_globals.config.enabled);

    health_globals.config.stock_enabled =
        inicfg_get_boolean(&netdata_config, CONFIG_SECTION_HEALTH,
                           "enable stock health configuration",
                           health_globals.config.stock_enabled);

    health_globals.config.use_summary_for_notifications =
        inicfg_get_boolean(&netdata_config, CONFIG_SECTION_HEALTH,
                           "use summary for notifications",
                           health_globals.config.use_summary_for_notifications);

    health_globals.config.default_warn_repeat_every =
        inicfg_get_duration_seconds(&netdata_config, CONFIG_SECTION_HEALTH, "default repeat warning", 0);

    health_globals.config.default_crit_repeat_every =
        inicfg_get_duration_seconds(&netdata_config, CONFIG_SECTION_HEALTH, "default repeat critical", 0);

    health_globals.config.health_log_entries_max =
        inicfg_get_number(&netdata_config, CONFIG_SECTION_HEALTH, "in memory max health log entries",
                          health_globals.config.health_log_entries_max);

    health_globals.config.health_log_retention_s =
        inicfg_get_duration_seconds(&netdata_config, CONFIG_SECTION_HEALTH, "health log retention", HEALTH_LOG_RETENTION_DEFAULT);

    health_globals.config.default_exec = string_strdupz(health_notification_program_configured());

    health_globals.config.enabled_alerts =
        simple_pattern_create(inicfg_get(&netdata_config, CONFIG_SECTION_HEALTH, "enabled alarms", "*"),
                              NULL, SIMPLE_PATTERN_EXACT, true);

    health_globals.config.run_at_least_every_seconds =
        (int)inicfg_get_duration_seconds(&netdata_config, CONFIG_SECTION_HEALTH, "run at least every",
                                         health_globals.config.run_at_least_every_seconds);

    health_globals.config.postpone_alarms_during_hibernation_for_seconds =
        inicfg_get_duration_seconds(&netdata_config, CONFIG_SECTION_HEALTH,
                                    "postpone alarms during hibernation for",
                                    health_globals.config.postpone_alarms_during_hibernation_for_seconds);

    time_t notification_execution_timeout =
        inicfg_get_duration_seconds(&netdata_config, CONFIG_SECTION_HEALTH,
                                    "notification execution timeout",
                                    health_globals.config.notification_execution_timeout_seconds);
    // clamp to [0, INT32_MAX] before narrowing to int32: the upper clamp prevents a huge
    // value from overflowing into a negative, the lower clamp normalizes negatives to 0.
    // 0 means "wait forever".
    if(notification_execution_timeout < 0) notification_execution_timeout = 0;
    if(notification_execution_timeout > INT32_MAX) notification_execution_timeout = INT32_MAX;
    health_globals.config.notification_execution_timeout_seconds = (int32_t)notification_execution_timeout;

    health_globals.config.default_recipient =
        string_strdupz("root");

    // ------------------------------------------------------------------------
    // verify after loading

    if(health_globals.config.run_at_least_every_seconds < 1)
        health_globals.config.run_at_least_every_seconds = 1;

    if(health_globals.config.health_log_entries_max < HEALTH_LOG_ENTRIES_MIN) {
        nd_log(NDLS_DAEMON, NDLP_WARNING,
               "Health configuration has invalid max log entries %u, using minimum of %u",
               health_globals.config.health_log_entries_max,
               HEALTH_LOG_ENTRIES_MIN);

        health_globals.config.health_log_entries_max = HEALTH_LOG_ENTRIES_MIN;
        inicfg_set_number(&netdata_config, CONFIG_SECTION_HEALTH, "in memory max health log entries",
                          (long)health_globals.config.health_log_entries_max);
    }
    else if(health_globals.config.health_log_entries_max > HEALTH_LOG_ENTRIES_MAX) {
        nd_log(NDLS_DAEMON, NDLP_WARNING,
               "Health configuration has invalid max log entries %u, using maximum of %u",
               health_globals.config.health_log_entries_max,
               HEALTH_LOG_ENTRIES_MAX);

        health_globals.config.health_log_entries_max = HEALTH_LOG_ENTRIES_MAX;
        inicfg_set_number(&netdata_config, CONFIG_SECTION_HEALTH, "in memory max health log entries",
                          (long)health_globals.config.health_log_entries_max);
    }

    if (health_globals.config.health_log_retention_s < HEALTH_LOG_MINIMUM_HISTORY) {
        nd_log(NDLS_DAEMON, NDLP_WARNING,
               "Health configuration has invalid health log retention %u. Using minimum %d",
               health_globals.config.health_log_retention_s, HEALTH_LOG_MINIMUM_HISTORY);

        health_globals.config.health_log_retention_s = HEALTH_LOG_MINIMUM_HISTORY;
        inicfg_set_duration_seconds(&netdata_config, CONFIG_SECTION_HEALTH, "health log retention", health_globals.config.health_log_retention_s);
    }

    nd_log(NDLS_DAEMON, NDLP_DEBUG,
           "Health log history is set to %u seconds (%u days)",
           health_globals.config.health_log_retention_s, health_globals.config.health_log_retention_s / 86400);
}

inline const char *health_user_config_dir(void) {
    char buffer[FILENAME_MAX + 1];
    snprintfz(buffer, FILENAME_MAX, "%s/health.d", netdata_configured_user_config_dir);
    return inicfg_get_path(&netdata_config, CONFIG_SECTION_DIRECTORIES, "health config", buffer);
}

inline const char *health_stock_config_dir(void) {
    char buffer[FILENAME_MAX + 1];
    snprintfz(buffer, FILENAME_MAX, "%s/health.d", netdata_configured_stock_config_dir);
    return inicfg_get_path(&netdata_config, CONFIG_SECTION_DIRECTORIES, "stock health config", buffer);
}

void health_plugin_init(void) {
    spinlock_lock(&health_globals.initialization.spinlock);

    if(health_globals.initialization.done)
        goto cleanup;

    health_globals.initialization.done = true;

    health_alarm_entry_aral_init();
    health_init_prototypes();
    health_load_config_defaults();

    if(!health_plugin_enabled())
        goto cleanup;

    health_reload_prototypes();
    health_silencers_init();

cleanup:
    spinlock_unlock(&health_globals.initialization.spinlock);
}

void health_plugin_destroy(void) {
    if(!health_globals.initialization.done)
        return;

    spinlock_lock(&health_globals.initialization.spinlock);

    // Clean up health prototypes dictionary
    if(health_globals.prototypes.dict) {
        dictionary_destroy(health_globals.prototypes.dict);
        health_globals.prototypes.dict = NULL;
    }

    // Free allocated strings
    string_freez(health_globals.config.default_exec);
    string_freez(health_globals.config.default_recipient);
    string_freez(health_globals.config.silencers_filename);
    
    // Free the enabled_alerts pattern
    simple_pattern_free(health_globals.config.enabled_alerts);

    // Reset pointers to NULL
    health_globals.config.default_exec = NULL;
    health_globals.config.default_recipient = NULL;
    health_globals.config.silencers_filename = NULL;
    health_globals.config.enabled_alerts = NULL;

    alert_variable_lookup_cleanup();

    health_globals.initialization.done = false;

    spinlock_unlock(&health_globals.initialization.spinlock);
}

void health_plugin_reload(void) {
    health_reload_prototypes();
    health_apply_prototypes_to_all_hosts();
}
