// SPDX-License-Identifier: GPL-3.0-or-later

#include "plugins_d.h"
#include "pluginsd_parser.h"

char *plugin_directories[PLUGINSD_MAX_DIRECTORIES] = { [0] = PLUGINS_DIR, };
struct plugind *pluginsd_root = NULL;

static inline void pluginsd_sleep(const int seconds) {
    int timeout_ms = seconds * 1000;
    int waited_ms = 0;
    while(waited_ms < timeout_ms) {
        if(!service_running(SERVICE_COLLECTORS)) break;
        sleep_usec(ND_CHECK_CANCELLABILITY_WHILE_WAITING_EVERY_MS * USEC_PER_MS);
        waited_ms += ND_CHECK_CANCELLABILITY_WHILE_WAITING_EVERY_MS;
    }
}

inline size_t pluginsd_initialize_plugin_directories()
{
    char plugins_dirs[(FILENAME_MAX * 2) + 1];
    static char *plugins_dir_list = NULL;

    // Get the configuration entry
    if (likely(!plugins_dir_list)) {
        snprintfz(plugins_dirs, FILENAME_MAX * 2, "\"%s\" \"%s/custom-plugins.d\"", PLUGINS_DIR, CONFIG_DIR);
        plugins_dir_list = strdupz(config_get(CONFIG_SECTION_DIRECTORIES, "plugins", plugins_dirs));
    }

    // Parse it and store it to plugin directories
    return quoted_strings_splitter_config(plugins_dir_list, plugin_directories, PLUGINSD_MAX_DIRECTORIES);
}

static inline void plugin_set_disabled(struct plugind *cd) {
    spinlock_lock(&cd->unsafe.spinlock);
    cd->unsafe.enabled = false;
    spinlock_unlock(&cd->unsafe.spinlock);
}

bool plugin_is_enabled(struct plugind *cd) {
    spinlock_lock(&cd->unsafe.spinlock);
    bool ret = cd->unsafe.enabled;
    spinlock_unlock(&cd->unsafe.spinlock);
    return ret;
}

static inline void plugin_set_running(struct plugind *cd) {
    spinlock_lock(&cd->unsafe.spinlock);
    cd->unsafe.running = true;
    spinlock_unlock(&cd->unsafe.spinlock);
}

static inline bool plugin_is_running(struct plugind *cd) {
    spinlock_lock(&cd->unsafe.spinlock);
    bool ret = cd->unsafe.running;
    spinlock_unlock(&cd->unsafe.spinlock);
    return ret;
}

static void pluginsd_worker_thread_cleanup(void *pptr) {
    pluginsd_log("WTC [1]");
    struct plugind *cd = CLEANUP_FUNCTION_GET_PTR(pptr);
    pluginsd_log("WTC [2]");
    if(!cd) return;

    pluginsd_log("WTC [3]");
    worker_unregister();

    pluginsd_log("WTC [4]");
    spinlock_lock(&cd->unsafe.spinlock);

    pluginsd_log("WTC [5]");

    cd->unsafe.running = false;
    cd->unsafe.thread = 0;

    pid_t pid = cd->unsafe.pid;
    cd->unsafe.pid = 0;

    pluginsd_log("WTC [6]");

    spinlock_unlock(&cd->unsafe.spinlock);

    pluginsd_log("WTC [7]");

    if (pid) {
        siginfo_t info;

        pluginsd_log("WTC [8]");
        netdata_log_info("PLUGINSD: 'host:%s', killing data collection child process with pid %d",
             rrdhost_hostname(cd->host), pid);

        pluginsd_log("WTC [9]");

        if (killpid(pid) != -1) {
            pluginsd_log("WTC [10]");

            netdata_log_info("PLUGINSD: 'host:%s', waiting for data collection child process pid %d to exit...",
                 rrdhost_hostname(cd->host), pid);

            pluginsd_log("WTC [11]");

            netdata_waitid(P_PID, (id_t)pid, &info, WEXITED);

            pluginsd_log("WTC [12]");
        }

        pluginsd_log("WTC [13]");
    }

    pluginsd_log("WTC [14]");
}

#define SERIAL_FAILURES_THRESHOLD 10
static void pluginsd_worker_thread_handle_success(struct plugind *cd) {
    if (likely(cd->successful_collections)) {
        pluginsd_sleep(cd->update_every);
        return;
    }

    if (likely(cd->serial_failures <= SERIAL_FAILURES_THRESHOLD)) {
        netdata_log_info("PLUGINSD: 'host:%s', '%s' (pid %d) does not generate useful output but it reports success (exits with 0). %s.",
             rrdhost_hostname(cd->host), cd->fullfilename, cd->unsafe.pid,
             plugin_is_enabled(cd) ? "Waiting a bit before starting it again." : "Will not start it again - it is now disabled.");

        pluginsd_sleep(cd->update_every * 10);
        return;
    }

    if (cd->serial_failures > SERIAL_FAILURES_THRESHOLD) {
        netdata_log_error("PLUGINSD: 'host:'%s', '%s' (pid %d) does not generate useful output, "
              "although it reports success (exits with 0)."
              "We have tried to collect something %zu times - unsuccessfully. Disabling it.",
              rrdhost_hostname(cd->host), cd->fullfilename, cd->unsafe.pid, cd->serial_failures);
        plugin_set_disabled(cd);
        return;
    }
}

static void pluginsd_worker_thread_handle_error(struct plugind *cd, int worker_ret_code) {
    if (worker_ret_code == -1) {
        netdata_log_info("PLUGINSD: 'host:%s', '%s' (pid %d) was killed with SIGTERM. Disabling it.",
             rrdhost_hostname(cd->host), cd->fullfilename, cd->unsafe.pid);
        plugin_set_disabled(cd);
        return;
    }

    if (!cd->successful_collections) {
        netdata_log_error("PLUGINSD: 'host:%s', '%s' (pid %d) exited with error code %d and haven't collected any data. Disabling it.",
              rrdhost_hostname(cd->host), cd->fullfilename, cd->unsafe.pid, worker_ret_code);
        plugin_set_disabled(cd);
        return;
    }

    if (cd->serial_failures <= SERIAL_FAILURES_THRESHOLD) {
        netdata_log_error("PLUGINSD: 'host:%s', '%s' (pid %d) exited with error code %d, but has given useful output in the past (%zu times). %s",
              rrdhost_hostname(cd->host), cd->fullfilename, cd->unsafe.pid, worker_ret_code, cd->successful_collections,
              plugin_is_enabled(cd) ? "Waiting a bit before starting it again." : "Will not start it again - it is disabled.");

        pluginsd_sleep(cd->update_every * 10);
        return;
    }

    if (cd->serial_failures > SERIAL_FAILURES_THRESHOLD) {
        netdata_log_error("PLUGINSD: 'host:%s', '%s' (pid %d) exited with error code %d, but has given useful output in the past (%zu times)."
              "We tried to restart it %zu times, but it failed to generate data. Disabling it.",
              rrdhost_hostname(cd->host), cd->fullfilename, cd->unsafe.pid, worker_ret_code,
              cd->successful_collections, cd->serial_failures);
        plugin_set_disabled(cd);
        return;
    }
}

#undef SERIAL_FAILURES_THRESHOLD

static void *pluginsd_worker_thread(void *arg) {
    struct plugind *cd = (struct plugind *) arg;
    CLEANUP_FUNCTION_REGISTER(pluginsd_worker_thread_cleanup) cleanup_ptr = cd;

    pluginsd_log("WT [0]");
    worker_register("PLUGINSD");

    pluginsd_log("WT [1]");
    plugin_set_running(cd);

    size_t count = 0;

    pluginsd_log("WT [2]");
    while(service_running(SERVICE_COLLECTORS)) {
        FILE *fp_child_input = NULL;
        pluginsd_log("WT [3]");
        FILE *fp_child_output = netdata_popen(cd->cmd, &cd->unsafe.pid, &fp_child_input);
        pluginsd_log("WT [4]");

        if(unlikely(!fp_child_input || !fp_child_output)) {
            pluginsd_log("WT [5]");
            netdata_log_error("PLUGINSD: 'host:%s', cannot popen(\"%s\", \"r\").",
                              rrdhost_hostname(cd->host), cd->cmd);
            pluginsd_log("WT [6]");
            break;
        }

        pluginsd_log("WT [7]");
        nd_log(NDLS_DAEMON, NDLP_DEBUG,
               "PLUGINSD: 'host:%s' connected to '%s' running on pid %d",
               rrdhost_hostname(cd->host),
               cd->fullfilename, cd->unsafe.pid);

        pluginsd_log("WT [8]");
        const char *plugin = strrchr(cd->fullfilename, '/');
        if(plugin)
            plugin++;
        else
            plugin = cd->fullfilename;

        pluginsd_log("WT [9]");

        char module[100];
        snprintfz(module, sizeof(module), "plugins.d[%s]", plugin);
        ND_LOG_STACK lgs[] = {
                ND_LOG_FIELD_TXT(NDF_MODULE, module),
                ND_LOG_FIELD_TXT(NDF_NIDL_NODE, rrdhost_hostname(cd->host)),
                ND_LOG_FIELD_TXT(NDF_SRC_TRANSPORT, "pluginsd"),
                ND_LOG_FIELD_END(),
        };
        ND_LOG_STACK_PUSH(lgs);

        pluginsd_log("WT [10]");

        count = pluginsd_process(cd->host, cd, fp_child_input, fp_child_output, 0);

        pluginsd_log("WT [11]");

        nd_log(NDLS_DAEMON, NDLP_DEBUG,
               "PLUGINSD: 'host:%s', '%s' (pid %d) disconnected after %zu successful data collections (ENDs).",
               rrdhost_hostname(cd->host), cd->fullfilename, cd->unsafe.pid, count);

        pluginsd_log("WT [12]");

        killpid(cd->unsafe.pid);

        pluginsd_log("WT [13]");

        int worker_ret_code = netdata_pclose(fp_child_input, fp_child_output, cd->unsafe.pid);

        pluginsd_log("WT [14]");

        if(likely(worker_ret_code == 0)) {
            pluginsd_log("WT [15]");
            pluginsd_worker_thread_handle_success(cd);
            pluginsd_log("WT [16]");
        }
        else {
            pluginsd_log("WT [17]");
            pluginsd_worker_thread_handle_error(cd, worker_ret_code);
            pluginsd_log("WT [18]");
        }

        pluginsd_log("WT [19]");
        cd->unsafe.pid = 0;

        pluginsd_log("WT [20]");
        if(unlikely(!plugin_is_enabled(cd))) {
            pluginsd_log("WT [21]");
            break;
        }

        pluginsd_log("WT [22]");
    }

    pluginsd_log("WT [23]");
    return NULL;
}

static void pluginsd_main_cleanup(void *pptr) {
    pluginsd_log("C [1]");

    struct netdata_static_thread *static_thread = CLEANUP_FUNCTION_GET_PTR(pptr);

    pluginsd_log("C [2]");
    if(!static_thread) return;

    pluginsd_log("C [3]");

    static_thread->enabled = NETDATA_MAIN_THREAD_EXITING;
    netdata_log_info("PLUGINSD: cleaning up...");

    pluginsd_log("C [4]");

    struct plugind *cd;
    for (cd = pluginsd_root; cd; cd = cd->next) {
        pluginsd_log("C [5]");
        spinlock_lock(&cd->unsafe.spinlock);

        pluginsd_log("C [6]");
        if (cd->unsafe.enabled && cd->unsafe.running && cd->unsafe.thread != 0) {
            pluginsd_log("C [7]");
            netdata_log_info("PLUGINSD: 'host:%s', stopping plugin thread: %s",
                 rrdhost_hostname(cd->host), cd->id);

            pluginsd_log("C [8]");
            nd_thread_signal_cancel(cd->unsafe.thread);
            pluginsd_log("C [9]");
        }

        pluginsd_log("C [10]");
        spinlock_unlock(&cd->unsafe.spinlock);
        pluginsd_log("C [11]");
    }

    pluginsd_log("C [12]");
    netdata_log_info("PLUGINSD: cleanup completed.");
    pluginsd_log("C [13]");
    static_thread->enabled = NETDATA_MAIN_THREAD_EXITED;

    pluginsd_log("C [14]");
    worker_unregister();
    pluginsd_log("C [15]");
}

void *pluginsd_main(void *ptr) {
    CLEANUP_FUNCTION_REGISTER(pluginsd_main_cleanup) cleanup_ptr = ptr;

    pluginsd_log("[0]");
    int automatic_run = config_get_boolean(CONFIG_SECTION_PLUGINS, "enable running new plugins", 1);

    pluginsd_log("[1]");
    int scan_frequency = (int)config_get_number(CONFIG_SECTION_PLUGINS, "check for new plugins every", 60);
    if (scan_frequency < 1)
        scan_frequency = 1;

    // disable some plugins by default
    pluginsd_log("[2]");
    config_get_boolean(CONFIG_SECTION_PLUGINS, "slabinfo", CONFIG_BOOLEAN_NO);
    pluginsd_log("[3]");
    config_get_boolean(CONFIG_SECTION_PLUGINS, "logs-management", 
#if defined(LOGS_MANAGEMENT_DEV_MODE)
        CONFIG_BOOLEAN_YES
#else 
        CONFIG_BOOLEAN_NO
#endif
    );
    // it crashes (both threads) on Alpine after we made it multi-threaded
    // works with "--device /dev/ipmi0", but this is not default
    // see https://github.com/netdata/netdata/pull/15564 for details
    pluginsd_log("[4]");
    if (getenv("NETDATA_LISTENER_PORT"))
        config_get_boolean(CONFIG_SECTION_PLUGINS, "freeipmi", CONFIG_BOOLEAN_NO);

    // store the errno for each plugins directory
    // so that we don't log broken directories on each loop
    int directory_errors[PLUGINSD_MAX_DIRECTORIES] = { 0 };

    pluginsd_log("[5]");

    while (service_running(SERVICE_COLLECTORS)) {
        pluginsd_log("[6]");

        int idx;
        const char *directory_name;

        for (idx = 0; idx < PLUGINSD_MAX_DIRECTORIES && (directory_name = plugin_directories[idx]); idx++) {
            pluginsd_log("[7]");
            if (unlikely(!service_running(SERVICE_COLLECTORS)))
                break;

            pluginsd_log("[8]");
            errno = 0;
            DIR *dir = opendir(directory_name);
            if (unlikely(!dir)) {
                pluginsd_log("[9]");
                if (directory_errors[idx] != errno) {
                    directory_errors[idx] = errno;
                    pluginsd_log("[10]");
                    netdata_log_error("cannot open plugins directory '%s'", directory_name);
                }
                continue;
            }

            pluginsd_log("[11]");

            struct dirent *file = NULL;
            while (likely((file = readdir(dir)))) {
                pluginsd_log("[12]");
                if (unlikely(!service_running(SERVICE_COLLECTORS))) {
                    pluginsd_log("[13]");
                    break;
                }

                pluginsd_log("[14]");
                netdata_log_debug(D_PLUGINSD, "examining file '%s'", file->d_name);

                pluginsd_log("[15]");
                if (unlikely(strcmp(file->d_name, ".") == 0 || strcmp(file->d_name, "..") == 0)) {
                    pluginsd_log("[16]");
                    continue;
                }

                pluginsd_log("[17]");
                int len = (int)strlen(file->d_name);
                if (unlikely(len <= (int)PLUGINSD_FILE_SUFFIX_LEN)) {
                    pluginsd_log("[18]");
                    continue;
                }

                pluginsd_log("[19]");
                if (unlikely(strcmp(PLUGINSD_FILE_SUFFIX, &file->d_name[len - (int)PLUGINSD_FILE_SUFFIX_LEN]) != 0)) {
                    pluginsd_log("[20]");
                    netdata_log_debug(D_PLUGINSD, "file '%s' does not end in '%s'", file->d_name, PLUGINSD_FILE_SUFFIX);
                    pluginsd_log("[21]");
                    continue;
                }

                pluginsd_log("[22]");
                char pluginname[CONFIG_MAX_NAME + 1];
                snprintfz(pluginname, CONFIG_MAX_NAME, "%.*s", (int)(len - PLUGINSD_FILE_SUFFIX_LEN), file->d_name);
                int enabled = config_get_boolean(CONFIG_SECTION_PLUGINS, pluginname, automatic_run);
                pluginsd_log("[23]");

                if (unlikely(!enabled)) {
                    pluginsd_log("[24]");
                    netdata_log_debug(D_PLUGINSD, "plugin '%s' is not enabled", file->d_name);
                    pluginsd_log("[25]");
                    continue;
                }

                // check if it runs already
                pluginsd_log("[26]");
                struct plugind *cd;
                for (cd = pluginsd_root; cd; cd = cd->next) {
                    pluginsd_log("[26]");
                    if (unlikely(strcmp(cd->filename, file->d_name) == 0)) {
                        pluginsd_log("[27]");
                        break;
                    }
                }

                pluginsd_log("[29]");
                if (likely(cd && plugin_is_running(cd))) {
                    pluginsd_log("[30]");
                    netdata_log_debug(D_PLUGINSD, "plugin '%s' is already running", cd->filename);
                    pluginsd_log("[31]");
                    continue;
                }

                // it is not running
                // allocate a new one, or use the obsolete one
                pluginsd_log("[32]");
                if (unlikely(!cd)) {
                    pluginsd_log("[33]");

                    cd = callocz(sizeof(struct plugind), 1);

                    snprintfz(cd->id, CONFIG_MAX_NAME, "plugin:%s", pluginname);

                    strncpyz(cd->filename, file->d_name, FILENAME_MAX);
                    snprintfz(cd->fullfilename, FILENAME_MAX, "%s/%s", directory_name, cd->filename);

                    cd->host = localhost;
                    cd->unsafe.enabled = enabled;
                    cd->unsafe.running = false;

                    pluginsd_log("[34]");

                    cd->update_every = (int)config_get_number(cd->id, "update every", localhost->rrd_update_every);
                    pluginsd_log("[35]");

                    cd->started_t = now_realtime_sec();
                    pluginsd_log("[37]");

                    char *def = "";
                    pluginsd_log("[38]");
                    snprintfz(
                        cd->cmd, PLUGINSD_CMD_MAX, "exec %s %d %s", cd->fullfilename, cd->update_every,
                        config_get(cd->id, "command options", def));
                    pluginsd_log("[39]");

                    // link it
                    pluginsd_log("[40]");
                    DOUBLE_LINKED_LIST_PREPEND_ITEM_UNSAFE(pluginsd_root, cd, prev, next);

                    pluginsd_log("[41]");

                    if (plugin_is_enabled(cd)) {
                        char tag[NETDATA_THREAD_TAG_MAX + 1];
                        snprintfz(tag, NETDATA_THREAD_TAG_MAX, "PD[%s]", pluginname);

                        // spawn a new thread for it
                        pluginsd_log("[42]");

                        cd->unsafe.thread = nd_thread_create(tag, NETDATA_THREAD_OPTION_DEFAULT,
                                                             pluginsd_worker_thread, cd);
                        pluginsd_log("[43]");
                    }

                    pluginsd_log("[44]");
                }

                pluginsd_log("[45]");
            }

            pluginsd_log("[46]");

            closedir(dir);

            pluginsd_log("[47]");
        }

        pluginsd_log("[48]");
        pluginsd_sleep(scan_frequency);
        pluginsd_log("[49]");
    }

    return NULL;
}
