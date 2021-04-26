// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Unit.h"
#include "Chart.h"

using namespace ml;

void Chart::updateMLChart() {
    if (MLRS) {
        rrdset_next(MLRS);
        for (auto &P : UnitsMap) {
            Unit *U = P.second;
            U->updateMLUnit(MLRS);
        }
        rrdset_done(MLRS);
        return;
    }

    std::string Name = std::string(RS->name);
    std::string Dot(".");
    std::size_t Pos = Name.find(Dot);

    if (Pos == std::string::npos) {
        info("Could not find set name: %s", RS->name);
        return;
    }

    Name = Name.substr(Pos + 1, Name.npos) + "_km";

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

    for (auto &P : UnitsMap) {
        Unit *U = P.second;
        U->updateMLUnit(MLRS);
    }
}

void Chart::updateUnits() {
    rrdset_rdlock(RS);

    RRDDIM *RD;
    rrddim_foreach_read(RD, RS) {
        std::map<RRDDIM *, Unit *>::iterator It = UnitsMap.find(RD);

        bool IsObsolete = rrddim_flag_check(RD, RRDDIM_FLAG_ARCHIVED) ||
                          rrddim_flag_check(RD, RRDDIM_FLAG_OBSOLETE);
        if (IsObsolete) {
            if (It != UnitsMap.end()) {
                error("Found obsolete dim %s.%s.%s", RS->rrdhost->hostname, RS->id, RD->id);
                UnitsMap.erase(RD);
            }
        } else {
            if (It == UnitsMap.end())
                UnitsMap[RD] = new Unit(RD);
        }
    }

    rrdset_unlock(RS);
}
