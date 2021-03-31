// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Unit.h"
#include "Chart.h"

using namespace ml;

/*
 * Create/Update the ML set that will contain the anomaly scores of each
 * unit in this chart.
 */
void Chart::updateMLChart() {
    if (MLRS) {
        // Update each dimension with the anomaly score of each unit.
        rrdset_next(MLRS);
        for (auto &P : UnitsMap) {
            Unit *U = P.second;
            U->updateMLUnit(MLRS);
        }
        rrdset_done(MLRS);
        return;
    }

    /*
     * Create a new ML set.
     */

    std::string Name = std::string(RS->name);
    std::string Dot(".");
    std::size_t Pos = Name.find(Dot);

    if (Pos == std::string::npos) {
        info("Could not find set name: %s", RS->name);
        return;
    }

    Name = Name.substr(Pos + 1, Name.npos) + "_km";

    // Use properties of the wrapped set to make the ML set appear
    // next to the wrapped set.
    MLRS = rrdset_create(
            RS->rrdhost,        // host
            RS->type,           // type
            Name.c_str(),       // id
            NULL,               // name
            RS->family,         // family
            NULL,               // context
            "Anomaly score",    // title
            "percentage",       // units
            RS->plugin_name,    // plugin
            RS->module_name,    // module
            RS->priority,       // priority
            1,                  // update_every
            RRDSET_TYPE_LINE    // chart_type
            );

    // Create a dim for each unit in the chart.
    for (auto &P : UnitsMap) {
        Unit *U = P.second;
        U->updateMLUnit(MLRS);
    }
}

/*
 * Update the units referenced by the chart.
 */
void Chart::updateUnits(Millis TrainSecs, Millis TrainEvery,
                        unsigned DiffN, unsigned SmoothN, unsigned LagN) {
    rrdset_rdlock(RS);

    RRDDIM *RD;
    rrddim_foreach_read(RD, RS) {
        bool IsObsolete = rrddim_flag_check(RD, RRDDIM_FLAG_ARCHIVED) ||
                          rrddim_flag_check(RD, RRDDIM_FLAG_OBSOLETE);
        if (IsObsolete)
            fatal("Found obsolete dim %s.%s.%s", RS->rrdhost->hostname, RS->id, RD->id);

        std::map<RRDDIM *, Unit *>::iterator It = UnitsMap.find(RD);
        if (It == UnitsMap.end())
            UnitsMap[RD] = new Unit(RD, TrainSecs, TrainEvery,
                                    DiffN, SmoothN, LagN);
    }

    rrdset_unlock(RS);
}
