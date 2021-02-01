// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

static calculated_number *
ml_kmeans_get_cns(ml_unit_t *unit, RRDR *res, unsigned ns, unsigned ndps, int dim_idx)
{
    // Bytes required per feature.
    size_t bpf = sizeof(calculated_number) * ndps * (ml_cfg.lag_n + 1);

    // Allocate memory buffer.
    calculated_number *cns = callocz(ns, bpf);

    // Handle set unit.
    if (unit->set) {
        memcpy(cns, res->v, sizeof(calculated_number) * ns * ndps);
        return cns;
    }

    // Handle dim unit.
    for (long row_idx = 0; row_idx != res->rows; row_idx++)
        cns[row_idx] = res->v[(row_idx * res->d) + dim_idx];

    return cns;
}

// FIXME: investigate this more
void
ml_kmeans_unit_dim(ml_unit_t *unit)
{
    // Run rrd query for unit.
    RRDR *res = ml_rrdr_for_unit(unit);
    if (!res)
        return;

    time_t now = now_realtime_sec();

    unsigned ns = res->rows;
    unsigned ndps = 1;

    RRDSET *set = unit->dim->rrdset;
    RRDDIM *dim;
    int dim_idx = -1;

    rrddim_foreach_read(dim, set) {
        dim_idx++;

        char id_buf[ML_UNIT_MAX_ID];
        snprintfz(id_buf, ML_UNIT_MAX_ID, "%s.%s", set->id, dim->id);

        ml_unit_t *u = dictionary_get(ml_cfg.train_dict, id_buf);
        if (!u)
            fatal("Could not find dim unit %s", id_buf);

        // Mark this unit as trained.
        u->last_trained_at = now;

        // Skip dims whose result should not get trained.
        if ((u->dim_opts[dim_idx] & RRDR_DIMENSION_SELECTED) == 0) {
            error("Skipping dim[%d]: %s", dim_idx, id_buf);
            continue;
        }

        // Get calculated numbers from rrdr.
        calculated_number *cns = ml_kmeans_get_cns(u, res, ns, ndps, dim_idx);
        if (!cns)
            break;

        // Train our data.
        kmeans_train(u->km_ref, cns, ns, ndps,
                     ml_cfg.diff_n, ml_cfg.smooth_n, ml_cfg.lag_n);

        // Free samples buffer.
        freez(cns);
    }

    // We don't need RRDR result any more.
    rrdr_free(res);

    if (unit->last_trained_at == now)
        info("Trained %s per dim", set->id);
}

void
ml_kmeans_unit_set(ml_unit_t *unit)
{
    // Run rrd query for unit.
    RRDR *res = ml_rrdr_for_unit(unit);
    if (!res)
        return;

    unsigned ns = res->rows;
    unsigned ndps = unit->set ? res->d : 1;

    // Get calculated numbers from rrdr
    calculated_number *cns = ml_kmeans_get_cns(unit, res, ns, ndps, -1);

    // We don't need RRDR result any more.
    rrdr_free(res);

    // Train our data.
    if (cns) {
        kmeans_train(unit->km_ref, cns, ns, ndps,
                     ml_cfg.diff_n, ml_cfg.smooth_n, ml_cfg.lag_n);

        // Free samples buffer.
        freez(cns);


        info("Trained %s", unit->set->id);
    }

    // Mark this unit as trained.
    time_t now = now_realtime_sec();
    unit->last_trained_at = now;
}

void
ml_kmeans_unit(ml_unit_t *unit)
{
    unit->set ? ml_kmeans_unit_set(unit) : ml_kmeans_unit_dim(unit);
}
