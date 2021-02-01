// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

static void
ml_anomaly_score_unit_dim(ml_unit_t *unit, calculated_number *cns,
                          unsigned ns, size_t bpf)
{
    RRDSET *set = unit->dim->rrdset;
    RRDDIM *dim;

    int dim_idx = -1;

    rrddim_foreach_read(dim, set) {
        dim_idx++;

        // Get the unit for this dim.
        char id_buf[ML_UNIT_MAX_ID];
        snprintfz(id_buf, ML_UNIT_MAX_ID, "%s.%s", set->id, dim->id);

        ml_unit_t *u = dictionary_get(ml_cfg.predict_dict, id_buf);
        if (!u)
            fatal("Could not find dim unit %s", id_buf);

        // Mark unit as predicted to avoid anomaly score recalculation.
        u->predicted = true;

        // FIXME: this should be locked.
        if (!u->dim_opts)
            continue;

        // Skip dims that were not selected in the training step.
        if ((u->dim_opts[dim_idx] & RRDR_DIMENSION_SELECTED) == 0) {
            error("Skipping dim[%d]: %s", dim_idx, id_buf);
            continue;
        }

        // Run query ops on dim. Specify a zero dim index because we are
        // predicting just dim.
        unsigned collected_ns = ml_query_dim(u->dim, 0, cns, ns, 1);

        if (collected_ns != ns) {
            error("Failed run query on dim %s: %u/%u", id_buf, collected_ns, ns);
            continue;
        }

        // Calculate anomaly score.
        calculated_number score = kmeans_anomaly_score(
            u->km_ref, cns, ns, 1,
            ml_cfg.diff_n, ml_cfg.smooth_n, ml_cfg.lag_n
        );

        if (isfinite(score))
            u->anomaly_score = score;
        else
            info("%s anomaly score: %Lf", id_buf, score);

        // Clear the samples buffer for the next iteration.
        memset(cns, 0, ns * bpf);
    }

    // Free the memory buffer now that we are done.
    freez(cns);
}

static void
ml_anomaly_score_unit_set(ml_unit_t *unit, calculated_number *cns,
                          unsigned ns, size_t bpf)
{
    (void) bpf;

    RRDSET *set = unit->set;
    RRDDIM *dim;

    int dim_idx = -1;

    rrddim_foreach_read(dim, set) {
        dim_idx++;

        // Just for consistency with dim units. It's not really needed for
        // set units.
        unit->predicted = true;

        // FIXME: this should be locked.
        if (!unit->dim_opts)
            continue;

        // Skip dims that were not selected in the training step. This is
        // okay for sets because the samples buffer is zero initialized.
        if ((unit->dim_opts[dim_idx] & RRDR_DIMENSION_SELECTED) == 0)
            continue;

        // Run query ops on dim.
        unsigned collected_ns = ml_query_dim(dim, dim_idx, cns, ns, unit->num_dims);

        // Skip prediction if we can't get the number of samples we need.
        if (collected_ns != ns)
            goto FREE_CNS;
    }

    // Calculate anomaly score.
    calculated_number score = kmeans_anomaly_score(
        unit->km_ref, cns, ns, unit->num_dims,
        ml_cfg.diff_n, ml_cfg.smooth_n, ml_cfg.lag_n
    );

    if (isfinite(score))
        unit->anomaly_score = score;
    else
        info("%s anomaly score: %Lf", set->id, score);

FREE_CNS:
    freez(cns);
}

void
ml_anomaly_score_unit(ml_unit_t* unit)
{
    RRDSET *set = unit->set ? unit->set : unit->dim->rrdset;

    rrdset_rdlock(set);

    // Count current number of dims in set.
    int num_dims = 0;

    RRDDIM *dim;
    rrddim_foreach_read(dim, set) {
        num_dims++;
    }

    // Make sure set has no new dims.
    if (num_dims != unit->num_dims) {
        error("Set %s - %d dims prediction vs %d dims training",
              set->id, num_dims, unit->num_dims);
        goto UNLOCK_SET;
    }

    // Figure number of samples we need.
    unsigned ns = ml_cfg.diff_n + ml_cfg.smooth_n + ml_cfg.lag_n;
    unsigned ndps = unit->set ? num_dims : 1;

    // Bytes required per feature.
    size_t bpf = sizeof(calculated_number) * ndps * (ml_cfg.lag_n + 1);

    // Allocate samples buffer.
    calculated_number *cns = callocz(ns, bpf);

    // Pass control to the appropriate function based on the unit's type.
    if (unit->set)
        ml_anomaly_score_unit_set(unit, cns, ns, bpf);
    else
        ml_anomaly_score_unit_dim(unit, cns, ns, bpf);

UNLOCK_SET:
    rrdset_unlock(set);
}
