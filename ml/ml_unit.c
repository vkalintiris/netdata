// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

static bool
ml_unit_should_train(ml_unit_t *unit)
{
    time_t now = now_realtime_sec();

    // We have never trained this.
    //
    // Set an artificial, future last_trained_at time. This will allow us to
    // ignore the unit, until enough data points have been populated.
    if (unit->last_trained_at == 0)
        unit->last_trained_at = now + ml_cfg.train_secs;

    // Train if "train every" secs have elapsed since "last training time".
    return (unit->last_trained_at + ml_cfg.train_every) < now;
}

void
ml_unit_train(ml_unit_t *unit)
{
    // Figure out if we should train this unit.
    bool should_train = ml_unit_should_train(unit);

    // Nothing else to do here, if we should not train the unit.
    if (!should_train)
        return;

    // Proceed with running K-Means.
    ml_kmeans_unit(unit);
}

void
ml_unit_predict(ml_unit_t *unit)
{
    ml_anomaly_score_unit(unit);
}
