// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml/kmeans/kmeans-c.h"
#include "daemon/common.h"

#define TRAIN_EVERY 50
#define NUM_SAMPLES 120

#define DIFF_N      1
#define SMOOTH_N    3
#define LAG_N       5

struct kmeans_config {
    // Host and chart and to run k-means on.
    RRDHOST *host;
    RRDSET *set;
    const char *chart_name;

    // Info required to run features extraction and the core k-means
    // clustering algorithm for this chart.
    size_t num_samples;
    size_t num_dims_per_sample;
    size_t size_per_sample;

    size_t diff_n;
    size_t smooth_n;
    size_t lag_n;

    // Time right before we run k-means clustering.
    time_t curr_training_time;
};

/*
 * Track the info we need a new kmeans configuration.
 */
static void
kmeans_init_config(struct kmeans_config *km_cfg,
                   RRDHOST *host, RRDSET *set, size_t num_samples,
                   size_t diff_n, size_t smooth_n, size_t lag_n) {
    km_cfg->host = host;
    km_cfg->set = set;
    km_cfg->chart_name = set->name ? set->name : "unnamed";

    km_cfg->num_samples = num_samples;

    km_cfg->num_dims_per_sample = 0;
    for (RRDDIM *dim = set->dimensions; dim; dim = dim->next)
        km_cfg->num_dims_per_sample++;

    km_cfg->size_per_sample =
        sizeof(calculated_number) * km_cfg->num_dims_per_sample * (lag_n + 1);

    km_cfg->diff_n = diff_n;
    km_cfg->smooth_n = smooth_n;
    km_cfg->lag_n = lag_n;

    km_cfg->curr_training_time = now_realtime_sec();
}

/*
 * Check if we can or should run this kmeans configuration.
 */
static bool
kmeans_ok_to_run(struct kmeans_config *km_cfg) {
    // Make sure our charts have at least 1 dimension.
    if (km_cfg->num_dims_per_sample == 0) {
        info("Skipping %s (zero dims)", km_cfg->chart_name);
        return false;
    }

    // For the time being, We only consider charts with dims that we update
    // every second.
    RRDSET *set = km_cfg->set;

    if (set->update_every != 1) {
        info("Skipping %s (update_every > 1)", km_cfg->chart_name);
        return false;
    }

    for (RRDDIM *dim = set->dimensions; dim; dim = dim->next) {
        if (dim->update_every != 1) {
            info("Skipping %s (dim update_every > 1)", km_cfg->chart_name);
            return false;
        }
    }

    // Train only if at least TRAIN_EVERY seconds have passed since the
    // previous training of this chart.
    return now_realtime_sec() >= (set->last_trained_at + TRAIN_EVERY);
}

static bool
kmeans_update_dim(struct kmeans_config *km_cfg, RRDDIM *dim, size_t dim_idx) {
    struct rrddim_query_ops *query_ops = &dim->state->query_ops;
    struct rrddim_query_handle query_handle;

    time_t latest_time = km_cfg->curr_training_time - 1;
    time_t oldest_time = km_cfg->curr_training_time - km_cfg->num_samples;

    size_t num_collected_samples = 0;

    query_ops->init(dim, &query_handle, oldest_time, latest_time);
    do {
        storage_number st_num;
        time_t curr_time;

        st_num = query_ops->next_metric(&query_handle, &curr_time);
        if (!does_storage_number_exist(st_num))
            break;

        size_t offset = (num_collected_samples * km_cfg->num_dims_per_sample) + dim_idx;
        km_cfg->set->train_data[offset] = unpack_storage_number(st_num);;

        num_collected_samples++;
    } while (!query_ops->is_finished(&query_handle));
    query_ops->finalize(&query_handle);

    return (num_collected_samples == km_cfg->num_samples);
}

static bool
kmeans_update_samples(struct kmeans_config *km_cfg) {
    size_t dim_idx = 0;

    for (RRDDIM *dim = km_cfg->set->dimensions; dim; dim = dim->next) {
        if (!kmeans_update_dim(km_cfg, dim, dim_idx))
            return false;

        dim_idx++;
    }

    return true;
}

static void
process_host_chart(RRDHOST *host, RRDSET *set) {
    struct timeval samples_update_begin_time, samples_update_end_time;
    now_monotonic_high_precision_timeval(&samples_update_begin_time);

        struct kmeans_config km_cfg = { 0 };

        kmeans_init_config(&km_cfg, host, set,
                           NUM_SAMPLES, DIFF_N, SMOOTH_N, LAG_N);

        if (!kmeans_ok_to_run(&km_cfg))
            return;

        km_cfg.set->train_data = callocz(km_cfg.num_samples, km_cfg.size_per_sample);

        if (!kmeans_update_samples(&km_cfg)) {
            info("could not update samples for %s", km_cfg.chart_name);
            goto RETURN_LBL;
        }

    now_monotonic_high_precision_timeval(&samples_update_end_time);
    usec_t dt_samples_update = dt_usec(&samples_update_end_time, &samples_update_begin_time);

    struct timeval kmeans_begin_time, kmeans_end_time;
    now_monotonic_high_precision_timeval(&kmeans_begin_time);

        kmeans_ref km_ref = kmeans_new(2);
        kmeans_train(km_ref, km_cfg.set->train_data,
                     km_cfg.num_samples, km_cfg.num_dims_per_sample,
                     km_cfg.diff_n, km_cfg.smooth_n, km_cfg.lag_n);
        kmeans_delete(km_ref);

        km_cfg.set->last_trained_at = now_realtime_sec();

    now_monotonic_high_precision_timeval(&kmeans_end_time);
    usec_t dt_kmeans = dt_usec(&kmeans_end_time, &kmeans_begin_time);

    info("updating samples for \"%s\" took %Lu usec", km_cfg.chart_name, dt_samples_update);
    info("computing k-means for \"%s\" took %Lu usec", km_cfg.chart_name, dt_kmeans);

RETURN_LBL:
    freez(km_cfg.set->train_data);
}

/*
 * logic to make the ml thread run every loop_every seconds
*/
static int
yield_at_most(time_t loop_every) {
    static time_t current_loop_time = 0;

    // this will be the first loop
    if (current_loop_time == 0) {
        current_loop_time = now_realtime_sec();
        goto YIELD_NOW;
    }

    // processing took more than loop_every
    time_t time_now = now_realtime_sec();
    if (time_now >= (current_loop_time + loop_every)) {
        current_loop_time = time_now;
        goto YIELD_NOW;
    }

    // processing took less than loop_every -> sleep
    time_t time_to_sleep = (current_loop_time + loop_every) - time_now;
    sleep_usec(USEC_PER_SEC * time_to_sleep);
    current_loop_time += time_to_sleep;

YIELD_NOW:
    return !netdata_exit;
}

/*
 * ml thread cleanup
*/
static void
ml_thread_cleanup(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up thread...\n");
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

void *
ml_main(void *ptr) {
    netdata_thread_cleanup_push(ml_thread_cleanup, ptr);

    while (yield_at_most(TRAIN_EVERY)) {
        struct timeval loop_begin_time, loop_end_time;
        now_monotonic_high_precision_timeval(&loop_begin_time);

            rrdhost_rdlock(localhost);

            RRDSET *set;
            rrdset_foreach_read(set, localhost) {
                struct timeval set_begin_time, set_end_time;

                now_monotonic_high_precision_timeval(&set_begin_time);

                    rrdset_rdlock(set);
                    process_host_chart(localhost, set);
                    rrdset_unlock(set);

                now_monotonic_high_precision_timeval(&set_end_time);
                usec_t dt = dt_usec(&set_end_time, &set_begin_time);
                info("%s loop run in %Lu usec", set->name ? set->name : "unnamed", dt);
            }

            rrdhost_unlock(localhost);

        now_monotonic_high_precision_timeval(&loop_end_time);
        usec_t dt = dt_usec(&loop_end_time, &loop_begin_time);
        info("ml loop run in %Lu usec", dt);
    }

    netdata_thread_cleanup_pop(1);
    return NULL;
}
