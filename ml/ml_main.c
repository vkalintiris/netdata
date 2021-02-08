// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"
#include "daemon/common.h"

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
    mti.num_samples = 3600;
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

void *
ml_main(void *ptr) {
    netdata_thread_cleanup_push(ml_thread_cleanup, ptr);

    init_ml_thread();

    heartbeat_t hb;
    heartbeat_init(&hb);
    usec_t hb_step = 1 * USEC_PER_SEC;

    while (!netdata_exit) {
        netdata_thread_testcancel();
        heartbeat_next(&hb, hb_step);
        if (!netdata_exit)
            break;

        mti.num_total_charts = 0;
        mti.num_trained_charts = 0;
        mti.max_feature_size = 0;
        mti.host = localhost;
        mti.loop_counter++;

        now_monotonic_high_precision_timeval(&mti.curr_loop_begin);
        run_ml_kmeans();
        now_monotonic_high_precision_timeval(&mti.curr_loop_end);

        usec_t loop_duration = dt_usec(&mti.curr_loop_end, &mti.curr_loop_begin);
        fprintf(mti.log_fp, "loop %zu took %Lu usec (skipped = %zu, trained = %zu)\n",
                mti.loop_counter, loop_duration,
                mti.num_trained_charts);

        fprintf(mti.log_fp, "max update duration so far: %Lu\n",
                mti.max_update_duration);
        fprintf(mti.log_fp, "max train duration so far: %Lu\n",
                mti.max_train_duration);
        fprintf(mti.log_fp, "max feature size: %zu\n\n",
                mti.max_feature_size);

        fflush(mti.log_fp);
    }

    netdata_thread_cleanup_pop(1);
    return NULL;
}
