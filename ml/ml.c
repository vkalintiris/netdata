// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

static void
ml_thread_cleanup(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up thread...");
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

static RRDR *
get_rrdr(struct ml_conf *mlc, RRDSET *st, size_t num_samples) {
    time_t time_before = now_realtime_sec();
    time_t time_after = time_before - num_samples;

    RRDR *res = rrd2rrdr(
        st,
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
        fprintf(mlc->fp, "[%zu][%s] - RRDR result is empty\n",
                mlc->loop_counter, st->name ? st->name : "unnamed");
        return NULL;
    }

    size_t max_possible_rows = time_before - time_after;
    if (res->rows > max_possible_rows)
        fatal("res->rows > max_possible_rows (%ld > %zu)", res->rows, max_possible_rows);

    size_t row_diff = max_possible_rows - res->rows;
    if (row_diff > 2) {
        fprintf(mlc->fp, "[%zu][%s] rrdr has only %zu rows\n",
                mlc->loop_counter, st->name ? st->name : "unnamed", res->rows);
        rrdr_free(res);
        return NULL;
    }

    fprintf(mlc->fp, "[%zu][%s] result contains %ld rows\n",
            mlc->loop_counter, st->name ? st->name : "unnamed", res->rows);

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
        fprintf(mlc->fp, "[%zu][%s] found %zu empty value(s) in rrd result\n",
                mlc->loop_counter, st->name ? st->name : "unnamed", num_empty_samples);
        rrdr_free(res);
        return NULL;
    }
#endif

    return res;
}

calculated_number *
ml_get_calculated_numbers(struct ml_conf *mlc, RRDSET *st, size_t *ns, size_t *ndps) {
    RRDR *res = get_rrdr(mlc, st, mlc->num_samples);
    if (!res) {
        fprintf(mlc->fp, "[%zu][%s] got null rrdr\n",
                mlc->loop_counter, st->name ? st->name : "unnamed");
        return NULL;
    }

    *ns = res->rows;
    *ndps = res->d;

    size_t bytes_per_feature = sizeof(calculated_number) * (*ndps) * (mlc->lag_n + 1);

    calculated_number *cns = callocz(res->rows, bytes_per_feature);
    memcpy(cns, res->v, sizeof(calculated_number) * (*ndps) * (*ns));
    rrdr_free(res);

    return cns;
}

extern void GoMLMain(void);

void *
ml_loop(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    netdata_thread_cleanup_push(ml_thread_cleanup, thr);

    if (!strcmp(thr->name, "MLTRAIN"))
        GoMLMain();

EXIT_THREAD:
    netdata_thread_cleanup_pop(1);
    return NULL;
}
