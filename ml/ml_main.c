// SPDX-License-Identifier: GPL-3.0-or-later

#include "kmeans/kmeans-c.h"
#include "daemon/common.h"

#define DIFF_N      1
#define SMOOTH_N    3
#define LAG_N       5

static void
ml_thread_cleanup(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up thread...\n");
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

static RRDR *
get_rrdr(RRDSET *set, time_t time_after, time_t time_before) {
    if (time_after >= time_before)
        fatal("time_after >= time_before (%ld >= %ld)", time_after, time_before);

    RRDR *res = rrd2rrdr(
        set,
        0, /* points_requested */
        time_after, /* after */
        time_before, /* before */
        RRDR_GROUPING_AVERAGE, /* grouping method */
        0, /* resampling time */
        0, /* grouping options */
        NULL, /* dimensions */
        NULL /* context params */
    );

    if (!res)
        fatal("RRDR result is empty\n");

    size_t max_possible_rows = time_before - time_after;
    if (res->rows > max_possible_rows)
        fatal("res->rows > max_possible_rows (%ld > %zu)", res->rows, max_possible_rows);

    size_t row_diff = max_possible_rows - res->rows;
    if (row_diff > 2) {
        info("Result of set %s has only %zu rows",
             set->name ? set->name : "unnamed", res->rows);
        rrdr_free(res);
        return NULL;
    }

    info("result contains %ld rows", res->rows);
#ifdef KMEANS_CHECK
    for (long i = 0; i != res->rows; i++) {
        calculated_number *cn = &res->v[res->d * i];
        RRDR_VALUE_FLAGS *vf = &res->o[res->d * i];

        for (long j = 0; j != res->d; j++)
            if (vf[j] && RRDR_VALUE_EMPTY)
                fatal("Found empty value!");
    }
#endif

    return res;
}

static void
run_kmeans(calculated_number *cns,
           size_t num_samples, size_t num_dims_per_sample,
           size_t diff_n, size_t smooth_n, size_t lag_n) {
    info("Running kmeans with ns: %zu, ndps: %zu, dn: %zu, sn: %zu, ln: %zu",
         num_samples, num_dims_per_sample, diff_n, smooth_n, lag_n);

    kmeans_ref km_ref = kmeans_new(2);
    kmeans_train(km_ref, cns, num_samples, num_dims_per_sample,
                 diff_n, smooth_n, lag_n);
    kmeans_delete(km_ref);
};

static void
ml_kmeans(time_t time_after, time_t time_before) {
    struct timeval tv_begin, tv_end;

    now_monotonic_high_precision_timeval(&tv_begin);

    RRDSET *set;
    rrdset_foreach_read(set, localhost) {
        struct timeval tv_st_begin, tv_st_end;
        struct timeval tv_sb_begin, tv_sb_end;
        struct timeval tv_km_begin, tv_km_end;

        now_monotonic_high_precision_timeval(&tv_st_begin);
        now_monotonic_high_precision_timeval(&tv_sb_begin);

        size_t num_dims = 0;
        for (RRDDIM *dim = set->dimensions; dim; dim = dim->next)
            num_dims++;

        if (num_dims == 0)
            fatal("Set %s has zero dims", set->name ? set->name : "unnamed");

        if (set->update_every != 1) {
            info("Set %s has update every %d secs",
                 set->name ? set->name : "unnamed", set->update_every);
            continue;
        }

        RRDR *res = get_rrdr(set, time_after, time_before);
        if (!res)
            continue;

        size_t num_samples = res->rows;
        size_t num_dims_per_sample = res->d;
        size_t bytes_per_feature =
            sizeof(calculated_number) * num_dims_per_sample * (LAG_N + 1);

        calculated_number *cns = callocz(num_samples, bytes_per_feature);
        memcpy(cns, res->v, sizeof(calculated_number) * num_dims_per_sample * num_samples);

        now_monotonic_high_precision_timeval(&tv_sb_end);

        now_monotonic_high_precision_timeval(&tv_km_begin);
        run_kmeans(cns, num_samples, num_dims_per_sample, DIFF_N, SMOOTH_N, LAG_N);
        now_monotonic_high_precision_timeval(&tv_km_end);

        freez(cns);
        rrdr_free(res);

        now_monotonic_high_precision_timeval(&tv_st_end);

        usec_t sb_dt = dt_usec(&tv_sb_end, &tv_sb_begin);
        info("sb: %Lu usec", sb_dt);

        usec_t km_dt = dt_usec(&tv_km_end, &tv_km_begin);
        info("km: %Lu usec", km_dt);

        usec_t st_dt = dt_usec(&tv_st_end, &tv_st_begin);
        info("st: %Lu usec", st_dt);
    }

    now_monotonic_high_precision_timeval(&tv_end);
    usec_t dt = dt_usec(&tv_end, &tv_begin);
    info("total thread time: %Lu usec", dt);
}

void *
ml_main(void *ptr) {
    netdata_thread_cleanup_push(ml_thread_cleanup, ptr);

    for (size_t i = 0; i != 120; i++)
        sleep_usec(USEC_PER_SEC);

    time_t time_before = now_realtime_sec();
    time_t time_after;

    time_after = time_before - 20;
    ml_kmeans(time_after, time_before);

#if 0
    time_after = time_before - (3600 * 2);
    ml_kmeans(time_after, time_before);

    time_after = time_before - (3600 * 4);
    ml_kmeans(time_after, time_before);

    time_after = time_before - (3600 * 6);
    ml_kmeans(time_after, time_before);

    time_after = time_before - (3600 * 12);
    ml_kmeans(time_after, time_before);

    time_after = time_before - (3600 * 24);
    ml_kmeans(time_after, time_before);
#endif

    netdata_thread_cleanup_pop(1);
    return NULL;
}
