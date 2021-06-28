// SPDX-License-Identifier: GPL-3.0-or-later

#include "Unit.h"
#include "Chart.h"

using namespace ml;

void Chart::addDimension(Dimension *D) {
    DimensionsMap[D->getRD()] = D;
}

void Chart::removeDimension(Dimension *D) {
    DimensionsMap.erase(D->getRD());
}

void Chart::updateMLChart() {
    if (MLRS) {
        rrdset_next(MLRS);
        for (auto &P : DimensionsMap) {
            Unit *U = P.second;
            U->updateMLRD(MLRS);
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
}
