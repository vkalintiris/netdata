// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

static void
ml_thread_cleanup(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up thread...");
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

void
ml_read_conf(struct ml_conf *mlc) {
    mlc->enabled = config_get_boolean(CONFIG_SECTION_ML, "enabled", 1);

    mlc->num_samples = config_get_number(CONFIG_SECTION_ML, "num samples to train", 300);
    mlc->train_every = config_get_number(CONFIG_SECTION_ML, "train every secs", 30);

    mlc->diff_n = config_get_number(CONFIG_SECTION_ML, "num samples to diff", 1);
    mlc->smooth_n = config_get_number(CONFIG_SECTION_ML, "num samples to smooth", 3);
    mlc->lag_n = config_get_number(CONFIG_SECTION_ML, "num samples to lag", 5);

    heartbeat_init(&mlc->hb);

    mlc->loop_counter = 0;

    mlc->fp = fopen(ML_LOG_FILE, "a");
    if (!mlc->fp)
        fatal("Could not open log file %s", ML_LOG_FILE);
}

bool
ml_should_ignore_set(RRDSET *st) {
    const char *name = st->name ? st->name : "unnamed";

    size_t num_dims = 0;
    for (RRDDIM *dim = st->dimensions; dim; dim = dim->next)
        num_dims++;

    if (num_dims == 0) {
        info("ignoring set \"%s\" because it has 0 dims", name);
        return true;
    }

    if (st->update_every != 1) {
        info("will not predict set \"%s\" because it updates every %d secs",
             name, st->update_every);
        return true;
    }

    return false;
}

static bool
ml_heartbeat(struct ml_conf *mlc) {
    if (mlc->fp)
        fflush(mlc->fp);

    usec_t dt = heartbeat_next(&mlc->hb, USEC_PER_SEC);
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

calculated_number *
ml_get_calculated_numbers(struct ml_conf *mlc, RRDSET *st, size_t *ns, size_t *ndps) {
    RRDR *res = get_rrdr(st, mlc->num_samples);
    if (!res)
        return NULL;

    *ns = res->rows;
    *ndps = res->d;

    size_t bytes_per_feature = sizeof(calculated_number) * (*ndps) * (mlc->lag_n + 1);

    calculated_number *cns = callocz(res->rows, bytes_per_feature);
    memcpy(cns, res->v, sizeof(calculated_number) * (*ndps) * (*ns));
    rrdr_free(res);

    return cns;
}

void *
ml_loop(void *ptr) {
    netdata_thread_cleanup_push(ml_thread_cleanup, ptr);

    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;
    bool is_train_thread = !strcmp(thr->name, "MLTRAIN");

    struct ml_conf mlc;
    ml_read_conf(&mlc);
    if (!mlc.enabled)
        goto EXIT_THREAD;

    while (ml_heartbeat(&mlc))
        is_train_thread ?  train_charts(&mlc) : predict_charts(&mlc);

EXIT_THREAD:
    netdata_thread_cleanup_pop(1);
    return NULL;
}
