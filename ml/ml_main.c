// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"
#include "daemon/common.h"

struct ml_thread_info mti;

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

static void
ml_thread_cleanup(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up thread...\n");
    fflush(mti.log_fp);
    fclose(mti.log_fp);
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}


void *
ml_main(void *ptr) {
    netdata_thread_cleanup_push(ml_thread_cleanup, ptr);

    memset(&mti, 0, sizeof(mti));

    mti.train_every = 3;
    mti.num_samples = 10;
    mti.diff_n = 1;
    mti.smooth_n = 3;
    mti.lag_n = 3;

    mti.log_fp = fopen("/tmp/ml.log", "w");
    if (!mti.log_fp)
        fatal("Could not open ML log file");

    mti.max_loop_duration = 0;
    mti.loop_counter = 0;

    while (yield_at_most(mti.train_every)) {
        mti.loop_counter++;

        mti.num_skipped_charts = 0;
        mti.num_trained_charts = 0;

        now_monotonic_high_precision_timeval(&mti.curr_loop_begin);

        {
            mti.host = localhost;

            rrdhost_rdlock(mti.host);
            rrdset_foreach_read(mti.set, mti.host) {
                rrdset_rdlock(mti.set);

                ml_kmeans();

                rrdset_unlock(mti.set);
            }
            rrdhost_unlock(mti.host);
        }

        now_monotonic_high_precision_timeval(&mti.curr_loop_end);

        usec_t loop_duration = dt_usec(&mti.curr_loop_end, &mti.curr_loop_begin);
        fprintf(mti.log_fp, "loop %zu took %Lu usec (skipped = %zu, trained = %zu)\n",
                mti.loop_counter, loop_duration,
                mti.num_skipped_charts, mti.num_trained_charts);

        fprintf(mti.log_fp, "max update duration so far: %Lu\n",
                mti.max_update_duration);
        fprintf(mti.log_fp, "max train duration so far: %Lu\n\n",
                mti.max_train_duration);

        fflush(mti.log_fp);
    }

    netdata_thread_cleanup_pop(1);
    return NULL;
}
