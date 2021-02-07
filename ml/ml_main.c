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

static void
report_ml_kmeans_status() {
    fprintf(mti.log_fp, "loop %zu: ", mti.loop_counter);

    switch (mti.status) {
        case ML_ERR_SET_UPDATE:
            fprintf(mti.log_fp, "\"%s\" has update_every != 1\n", mti.chart_name);
            break;
        case ML_ERR_DIM_UPDATE:
            fprintf(mti.log_fp, "\"%s.%s\" has update_every != 1\n",
                    mti.chart_name, mti.dim_name);
            break;
        case ML_ERR_ZERO_DIMS:
            fprintf(mti.log_fp, "\"%s\" has 0 dims\n", mti.chart_name);
            break;
        case ML_ERR_NO_STORAGE_NUMBER:
            fprintf(mti.log_fp, "\"%s.%s\" missing storage number in [%ld, %ld]\n",
                    mti.chart_name, mti.dim_name,
                    mti.dim_oldest_time, mti.dim_latest_time);
            break;
        case ML_ERR_NOT_ENOUGH_SAMPLES:
            fprintf(mti.log_fp, "\"%s.%s\" has only %zu values in [%ld, %ld]\n",
                    mti.chart_name, mti.dim_name,
                    mti.num_collected_samples,
                    mti.dim_oldest_time, mti.dim_latest_time);
            break;
        case ML_OK: {
            mti.num_trained_charts++;

            usec_t update_dt = dt_usec(&mti.update_end, &mti.update_begin);
            if (update_dt > mti.max_update_duration)
                mti.max_update_duration = update_dt;

            usec_t train_dt = dt_usec(&mti.train_end, &mti.train_begin);
            if (train_dt > mti.max_train_duration)
                mti.max_train_duration = train_dt;

            fprintf(mti.log_fp, "update_dt = %Lu usec, train_dt = %Lu\n usec\n",
                    update_dt, train_dt);

            return;
        }
        default:
            fatal("bad ml kmeans status %zu\n", mti.status);
    }

    mti.num_skipped_charts++;
}

void *
ml_main(void *ptr) {
    netdata_thread_cleanup_push(ml_thread_cleanup, ptr);

    memset(&mti, 0, sizeof(mti));

    mti.train_every = 20;
    mti.num_samples = 60;
    mti.diff_n = 1;
    mti.smooth_n = 3;
    mti.lag_n = 5;

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

                report_ml_kmeans_status();
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
        fprintf(mti.log_fp, "max train duration so far: %Lu\n",
                mti.max_train_duration);

        fflush(mti.log_fp);
    }

    netdata_thread_cleanup_pop(1);
    return NULL;
}
