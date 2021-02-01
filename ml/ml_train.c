// SPDX-License-Identifier: GPL-3.0-or-later

#include "kmeans/kmeans-c.h"
#include "daemon/common.h"
#include <stdbool.h>

#define KMEANS_CHECK

static void
ml_train_cleanup(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up thread...\n");
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

static bool
should_train_set(RRDSET *st, size_t train_every) {
    const char *name = st->name ? st->name : "unnamed";

    size_t num_dims = 0;
    for (RRDDIM *dim = st->dimensions; dim; dim = dim->next)
        num_dims++;

    if (num_dims == 0) {
        info("Set %s not trained because it has zero dims", name);
        return false;
    }

    if (st->update_every != 1) {
        info("Set %s not trained because it updates every %d secs",
             name, st->update_every);
        return false;
    }

    time_t now = now_realtime_sec();

    if (st->last_trained_at == 0)
        st->last_trained_at = now;

    return (st->last_trained_at + train_every) < now;
}

static bool
train_heartbeat(void) {
    static bool initialized = false;
    static heartbeat_t hb;

    if (!initialized) {
        heartbeat_init(&hb);
        initialized = true;
    }

    usec_t dt = heartbeat_next(&hb, USEC_PER_SEC);
    if (netdata_exit)
        return false;

    return true;
}

static RRDR *
get_rrdr(RRDSET *set, size_t num_samples) {
    time_t time_before = now_realtime_sec();
    time_t time_after = time_before - num_samples;

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

    if (!res) {
        error("RRDR result is empty\n");
        return NULL;
    }

    size_t max_possible_rows = time_before - time_after;
    if (res->rows > max_possible_rows)
        fatal("res->rows > max_possible_rows (%ld > %zu)", res->rows, max_possible_rows);

    size_t row_diff = max_possible_rows - res->rows;
    if (row_diff > 2) {
        info("result of set %s has only %zu rows",
             set->name ? set->name : "unnamed", res->rows);
        rrdr_free(res);
        return NULL;
    }

    info("result contains %ld rows", res->rows);

#ifdef KMEANS_CHECK
    size_t num_empty_samples = 0;

    for (long i = 0; i != res->rows; i++) {
        calculated_number *cn = &res->v[res->d * i];
        RRDR_VALUE_FLAGS *vf = &res->o[res->d * i];

        for (long j = 0; j != res->d; j++)
            if (vf[j] && RRDR_VALUE_EMPTY) {
                num_empty_samples++;
                break;
            }
    }

    if (num_empty_samples) {
        error("found %zu empty value(s) in rrd result of \"%s\"",
              num_empty_samples, set->name ? set->name : "unnamed");
        rrdr_free(res);
        return NULL;
    }
#endif

    return res;
}

static bool
train_chart(RRDSET *st, size_t num_samples, size_t train_every,
         size_t diff_n, size_t smooth_n, size_t lag_n) {
    if (!should_train_set(st, train_every))
        return false;

    RRDR *res = get_rrdr(st, num_samples);
    if (!res)
        return false;

    num_samples = res->rows;
    size_t num_dims_per_sample = res->d;
    size_t bytes_per_feature =
        sizeof(calculated_number) * num_dims_per_sample * (lag_n + 1);

    calculated_number *cns = callocz(num_samples, bytes_per_feature);
    memcpy(cns, res->v, sizeof(calculated_number) * num_dims_per_sample * num_samples);
    rrdr_free(res);

    if (!st->km_ref)
        st->km_ref = kmeans_new(2);
    kmeans_train(st->km_ref, cns, num_samples, num_dims_per_sample,
                 diff_n, smooth_n, lag_n);

    freez(cns);

    st->last_trained_at = now_realtime_sec();
    return true;
}

static void
train_charts(size_t num_samples, size_t train_every,
             size_t diff_n, size_t smooth_n, size_t lag_n) {
    size_t num_trained_charts = 0;

    rrdhost_rdlock(localhost);

    RRDSET *st;
    rrdset_foreach_read(st, localhost) {
        if (train_chart(st, num_samples, train_every, diff_n, smooth_n, lag_n))
            num_trained_charts++;
    }

    rrdhost_unlock(localhost);

    info("Trained %zu charts", num_trained_charts);
}

void *
ml_train(void *ptr) {
    netdata_thread_cleanup_push(ml_train_cleanup, ptr);

    int enabled = config_get_boolean(CONFIG_SECTION_ML, "enabled", 1);
    if (!enabled)
        info("machine learning is disabled");

    size_t num_samples, train_every;
    num_samples = config_get_number(CONFIG_SECTION_ML, "num samples to train", 100);
    train_every = config_get_number(CONFIG_SECTION_ML, "train every secs", 15);

    size_t diff_n, smooth_n, lag_n;
    diff_n = config_get_number(CONFIG_SECTION_ML, "num samples to diff", 1);
    smooth_n = config_get_number(CONFIG_SECTION_ML, "num samples to smooth", 3);
    lag_n = config_get_number(CONFIG_SECTION_ML, "num samples to lag", 3);

    while (train_heartbeat())
        train_charts(num_samples, train_every, diff_n, smooth_n, lag_n);

    netdata_thread_cleanup_pop(1);
    return NULL;
}
