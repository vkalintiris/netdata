// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

static void
ml_chart_update_set(ml_unit_t *unit) {
    RRDSET *set = unit->set;

    char id_buf[ML_UNIT_MAX_ID];
    snprintfz(id_buf, ML_UNIT_MAX_ID, "%s.%s", ML_CHART_PREFIX, set->id);

    if (!unit->ml_chart) {
        unit->ml_chart = rrdset_create_localhost(
            set->type, id_buf, id_buf, set->family, NULL, "Anomaly score", set->units, set->plugin_name,
            set->module_name, set->priority + 1, 1, RRDSET_TYPE_LINE);

        unit->ml_dim = rrddim_add(unit->ml_chart, "anomaly score", NULL, 1, 10000, RRD_ALGORITHM_ABSOLUTE);
        return;
    }

    rrdset_next(unit->ml_chart);
    rrddim_set_by_pointer(unit->ml_chart, unit->ml_dim, unit->anomaly_score * 10000.0);
    rrdset_done(unit->ml_chart);
}

static void
ml_chart_update_dim(ml_unit_t *unit) {
    RRDSET *set = unit->dim->rrdset;

    char id_buf[ML_UNIT_MAX_ID];
    snprintfz(id_buf, ML_UNIT_MAX_ID, "%s.%s", ML_CHART_PREFIX, set->id);

    if (!unit->ml_chart) {
        // type, id, name, family, context, title, units, plugin,
        // module, priority, update_every, chart_type
        //
        // st_aclkstats = rrdset_create_localhost(
        //      "netdata", "aclk_status", NULL, "aclk", NULL, "ACLK/Cloud connection status",
        //      "connected", "netdata", "stats", 200000, localhost->rrd_update_every, RRDSET_TYPE_LINE);

        RRDSET *ml_set = rrdset_create_localhost(
            set->type, id_buf, id_buf, set->family, NULL, "Anomaly score (Per dim)", set->units, set->plugin_name,
            set->module_name, set->priority + 1, 1, RRDSET_TYPE_LINE);

        rrdset_rdlock(set);

        RRDDIM *dim;
        rrddim_foreach_read(dim, set) {
            char id_buf[ML_UNIT_MAX_ID];
            snprintfz(id_buf, ML_UNIT_MAX_ID, "%s.%s", set->id, dim->id);

            ml_unit_t *u = dictionary_get(ml_cfg.predict_dict, id_buf);
            if (!u)
                continue;

            assert(!u->ml_chart && !u->ml_dim);

            u->ml_chart = ml_set;
            u->ml_dim = rrddim_add(ml_set, id_buf, NULL, 1, 10000, RRD_ALGORITHM_ABSOLUTE);
        }

        rrdset_unlock(set);
        return;
    }

    rrdset_rdlock(set);
    rrdset_next(unit->ml_chart);

    RRDDIM *dim;
    rrddim_foreach_read(dim, set) {
        char id_buf[ML_UNIT_MAX_ID];
        snprintfz(id_buf, ML_UNIT_MAX_ID, "%s.%s", set->id, dim->id);

        ml_unit_t *u = dictionary_get(ml_cfg.predict_dict, id_buf);
        if (!u)
            continue;

        rrddim_set_by_pointer(u->ml_chart, u->ml_dim, u->anomaly_score * 10000.0);
        u->ml_chart_updated = true;
    }

    rrdset_done(unit->ml_chart);
    rrdset_unlock(set);
}

void
ml_chart_update_unit(ml_unit_t *unit)
{
    unit->set ? ml_chart_update_set(unit) : ml_chart_update_dim(unit);
}
