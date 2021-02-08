// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"
#include "daemon/common.h"

#define HB_STEP 120

struct ml_thread_info mti;

static void
ml_thread_cleanup(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up thread...\n");
    fflush(mti.log_fp);
    fclose(mti.log_fp);
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

static void init_ml_thread() {
    memset(&mti, 0, sizeof(mti));

    mti.train_every = 60;
    mti.num_samples = 3600 * 4;
    mti.diff_n = 1;
    mti.smooth_n = 3;
    mti.lag_n = 5;

    mti.log_fp = fopen("/tmp/ml.log", "w");
    if (!mti.log_fp)
        fatal("Could not open ML log file");

    mti.max_loop_duration = 0;
    mti.loop_counter = 0;
}

static void run_ml_kmeans(void) {
    rrdhost_rdlock(mti.host);

    rrdset_foreach_read(mti.set, mti.host) {
        mti.num_total_charts++;

        rrdset_rdlock(mti.set);
        ml_kmeans();
        rrdset_unlock(mti.set);
    }

    rrdhost_unlock(mti.host);
}


static void
stats_collect(void) {
    static RRDSET *st_num_charts_trained = NULL;
    static RRDDIM *rd_num_charts_trained = NULL;

    if (!st_num_charts_trained) {
        st_num_charts_trained = rrdset_create_localhost(
            "netdata", "trained_charts", NULL, "ml", NULL, "Number of charts trained",
            "num charts trained", "netdata", "stats", 606060, HB_STEP, RRDSET_TYPE_AREA);

        rd_num_charts_trained = rrddim_add(st_num_charts_trained, "trained charts", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
    } else {
        rrdset_next(st_num_charts_trained);
    }

    rrddim_set_by_pointer(st_num_charts_trained, rd_num_charts_trained, mti.num_trained_charts);
    rrdset_done(st_num_charts_trained);

    static RRDSET *st_total_time = NULL;
    static RRDDIM *rd_total_time = NULL;

    if (!st_total_time) {
        st_total_time = rrdset_create_localhost(
            "netdata", "ml_loop_time", NULL, "ml", NULL, "Total time spent in ML loop",
            "time running ML loop", "netdata", "stats", 606061, HB_STEP, RRDSET_TYPE_AREA);

        rd_total_time = rrddim_add(st_total_time, "ML loop time", NULL, 1, 1000, RRD_ALGORITHM_ABSOLUTE);
    } else {
        rrdset_next(st_total_time);
    }

    rrddim_set_by_pointer(st_total_time, rd_total_time, mti.loop_duration);
    rrdset_done(st_total_time);
}

void *
ml_main(void *ptr) {
    netdata_thread_cleanup_push(ml_thread_cleanup, ptr);

    init_ml_thread();

    heartbeat_t hb;
    heartbeat_init(&hb);
    usec_t hb_step = HB_STEP * USEC_PER_SEC;

    while (!netdata_exit) {
        netdata_thread_testcancel();

        heartbeat_next(&hb, hb_step);
        if (netdata_exit)
            break;

        mti.num_total_charts = 0;
        mti.num_trained_charts = 0;
        mti.max_feature_size = 0;
        mti.host = localhost;
        mti.loop_counter++;

        now_monotonic_high_precision_timeval(&mti.curr_loop_begin);
        run_ml_kmeans();
        now_monotonic_high_precision_timeval(&mti.curr_loop_end);

        mti.loop_duration = dt_usec(&mti.curr_loop_end, &mti.curr_loop_begin);
        fprintf(mti.log_fp, "loop %zu took %Lu usec (trained = %zu/%zu)\n",
                mti.loop_counter, mti.loop_duration,
                mti.num_trained_charts, mti.num_total_charts);

        fprintf(mti.log_fp, "max update duration so far: %Lu\n",
                mti.max_update_duration);
        fprintf(mti.log_fp, "max train duration so far: %Lu\n",
                mti.max_train_duration);
        fprintf(mti.log_fp, "max feature size: %zu\n\n",
                mti.max_feature_size);

        fflush(mti.log_fp);

        stats_collect();
    }

    netdata_thread_cleanup_pop(1);
    return NULL;
}
