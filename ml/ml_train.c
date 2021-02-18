// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

#define KMEANS_CHECK

static bool
should_train_set(RRDSET *st, size_t train_every) {
    if (ml_should_ignore_set(st))
        return false;

    time_t now = now_realtime_sec();

    if (st->last_trained_at == 0)
        st->last_trained_at = now;

    return (st->last_trained_at + train_every) < now;
}

static bool
train_chart(struct ml_conf *mlc, RRDSET *st) {
    if (!should_train_set(st, mlc->train_every))
        return false;

    size_t ns, ndps;

    calculated_number *cns = ml_get_calculated_numbers(mlc, st, &ns, &ndps);
    kmeans_train(st->km_ref, cns, ns, ndps,
                 mlc->diff_n, mlc->smooth_n, mlc->lag_n);
    freez(cns);

    st->last_trained_at = now_realtime_sec();
    return true;
}

void
train_charts(struct ml_conf *mlc) {
    size_t num_trained_charts = 0;

    rrdhost_rdlock(localhost);

    RRDSET *st;
    rrdset_foreach_read(st, localhost) {
        if (train_chart(mlc, st))
            num_trained_charts++;
    }

    rrdhost_unlock(localhost);

    info("Trained %zu charts", num_trained_charts);
}
