// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

#define KMEANS_CHECK

static bool
should_train_set(struct ml_conf *mlc, RRDSET *st) {
    if (ml_should_ignore_set(st)) {
        fprintf(mlc->fp, "[%zu][%s] - should ignore set\n",
                mlc->loop_counter, st->name ? st->name : "unnamed");
        return false;
    }

    time_t now = now_realtime_sec();

    if (st->last_trained_at == 0)
        st->last_trained_at = now;

    return (st->last_trained_at + mlc->train_every) < now;
}

static bool
train_chart(struct ml_conf *mlc, RRDSET *st) {
    if (!should_train_set(mlc, st)) {
        fprintf(mlc->fp, "[%zu][%s] - should not train set\n",
                mlc->loop_counter, st->name ? st->name : "unnamed");
        return false;
    }

    size_t ns, ndps;

    calculated_number *cns = ml_get_calculated_numbers(mlc, st, &ns, &ndps);
    if (!cns)
        return false;

    if (!st->km_ref)
        st->km_ref = kmeans_new(2);
    kmeans_train(st->km_ref, cns, ns, ndps, mlc->diff_n, mlc->smooth_n, mlc->lag_n);
    freez(cns);

    fprintf(mlc->fp, "[%zu][%s] - trained\n",
            mlc->loop_counter, st->name ? st->name : "unnamed");

    st->last_trained_at = now_realtime_sec();
    return true;
}

#if 0
void
train_charts(struct ml_conf *mlc) {
    size_t num_trained_charts = 0;

    rrdhost_rdlock(localhost);

    RRDSET *st;
    rrdset_foreach_read(st, localhost) {
        fprintf(mlc->fp, "[%zu][%s] - Loop start\n",
                mlc->loop_counter, st->name ? st->name : "unnamed");

        if (train_chart(mlc, st))
            num_trained_charts++;

        fprintf(mlc->fp, "[%zu][%s] - Loop end\n",
                mlc->loop_counter, st->name ? st->name : "unnamed");

        fflush(mlc->fp);
    }

    rrdhost_unlock(localhost);

    info("Trained %zu charts", num_trained_charts);
}
#endif
