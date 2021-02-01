// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

static void
ml_rrdr_update_dim_opts(ml_unit_t *unit, RRDR *res)
{
    /*
     * Fixup values in the result buffer that can break K-Means.
    */

    for (long row_idx = 0; row_idx != res->rows; row_idx++) {
        calculated_number *cn = &res->v[row_idx * res->d];
        RRDR_VALUE_FLAGS *vf = &res->o[row_idx * res->d];

        // Do not select dims that contain values that are either infinite or
        // overflown.
        for (long dim_idx = 0; dim_idx != res->d; dim_idx++) {
            bool ve = vf[dim_idx] & RRDR_VALUE_EMPTY;
            bool vr = vf[dim_idx] & RRDR_VALUE_RESET;

            // mark dim as not selected
            if (ve || vr || !isfinite(cn[dim_idx]))
                res->od[dim_idx] &= ~RRDR_DIMENSION_SELECTED;
        }
    }

    // Use a sane default value for dims we didn't select.
    for (long dim_idx = 0; dim_idx != res->d; dim_idx++) {
        if (res->od[dim_idx] & RRDR_DIMENSION_SELECTED)
            continue;

        for (long row_idx = 0; row_idx != res->rows; row_idx++)
            res->v[(row_idx * res->d) + dim_idx] = 0.0L;
    }

    /*
     * Make the dimension flags available to every unit.
    */

    // Create a copy of the dimension flags.
    RRDR_DIMENSION_FLAGS *od = callocz(res->d, sizeof(RRDR_DIMENSION_FLAGS));
    memcpy(od, res->od, res->d * sizeof(RRDR_DIMENSION_FLAGS));

    // Handle set units.
    if (unit->set) {
        if (unit->dim_opts)
            freez(unit->dim_opts);

        unit->dim_opts = od;
        return;
    }

    // Handle dim units.
    RRDSET *set = unit->dim->rrdset;
    RRDDIM *dim;

    rrddim_foreach_read(dim, set) {
        char id_buf[ML_UNIT_MAX_ID];
        snprintfz(id_buf, ML_UNIT_MAX_ID, "%s.%s", set->id, dim->id);

        ml_unit_t *u = dictionary_get(ml_cfg.train_dict, id_buf);
        if (!u)
            fatal("Could not find dim unit %s", id_buf);

        // Free the previous owner's dimension flags
        if (u->owns_dim_opts) {
            freez(u->dim_opts);
            u->owns_dim_opts = false;
        }

        u->dim_opts = od;
    }

    unit->owns_dim_opts = true;
}

// TODO: we should track this information somewhere
static bool
ml_rrdr_okay_to_use(ml_unit_t *unit, RRDR *res,
                    time_t time_after, time_t time_before)
{
    RRDSET *set = unit->set ? unit->set : unit->dim->rrdset;

    if (!res) {
        error("rrd result is null (%s, [%ld,%ld])\n", set->id, time_after, time_before);
        return false;
    }

    assert(time_after < time_before);

    assert(res->has_st_lock);

    if (res->d != unit->num_dims) {
        fatal("Unit dims: %d, Result dims: %d (in set %s)",
              unit->num_dims, res->d, set->id);
        rrdr_free(res);
        return false;
    }

    if (!res->rows) {
        error("[%ld - %ld, %ld - %ld] got back %ld rows for %s",
              time_after, res->after, time_before, res->before, res->rows, res->st->id);
        rrdr_free(res);
        return false;
    }

    // Bump because rrd2rrdr's left-hand side interval is open-ended
    time_after += 1;

    assert(res->after >= time_after || res->before <= time_before);

    double req_duration = time_before - time_after;
    double res_duration = res->before - res->after;

    double update_every = res->st->update_every;
    double max_rows = floor(req_duration / update_every);
    double res_rows = res_duration / update_every;
    double ratio = res_rows / max_rows;

    // Under *heavy* system load, rrd2rrdr sometimes will return results with
    // res->before << time_before. This assert makes sure that we won't
    // proceed unless we have at least 80% of the results we are asking for.
    assert(ratio >= 0.8);

    long min_req_rows = ml_cfg.diff_n + ml_cfg.smooth_n + ml_cfg.lag_n;
    if (res->rows < min_req_rows) {
        error("Few rows for kmeans on %s: %ld/%ld", set->id, res->rows, min_req_rows);
        rrdr_free(res);
        return false;
    }

    return true;
}

RRDR *
ml_rrdr_for_unit(ml_unit_t *unit)
{
    // Setup time window.
    time_t time_before = now_realtime_sec() - 1;
    time_t time_after = time_before - ml_cfg.train_secs;

    // Run the query.
    RRDSET *set = unit->set ? unit->set : unit->dim->rrdset;
    RRDR *res = rrd2rrdr(
        set,                    /* set */
        0,                      /* points requested */
        time_after,             /* after */
        time_before,            /* before */
        RRDR_GROUPING_AVERAGE,  /* grouping method */
        0,                      /* resampling time */
        0,                      /* grouping options */
        NULL,                   /* dimensions */
        NULL                    /* context params*/
    );

    // Run sanity checks on the result.
    if (!ml_rrdr_okay_to_use(unit, res, time_after, time_before))
        return NULL;

    // Figure out which dims we should select
    ml_rrdr_update_dim_opts(unit, res);

    return res;
}
