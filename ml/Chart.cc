// SPDX-License-Identifier: GPL-3.0-or-later

#include "Chart.h"

using namespace ml;

void Chart::addDimension(Dimension *D) {
    std::lock_guard<std::mutex> Lock(Mutex);
    DimensionsMap[D->getRD()] = D;
}

void Chart::removeDimension(Dimension *D) {
    std::lock_guard<std::mutex> Lock(Mutex);
    DimensionsMap.erase(D->getRD());
}

bool Chart::forEachDimension(std::function<bool(Dimension *)> Func) {
    std::lock_guard<std::mutex> Lock(Mutex);

    for (auto &DP : DimensionsMap) {
        Dimension *D = DP.second;

        if (Func(D))
            return true;
    }

    return false;
}

void Chart::updateMLChart() {
    if (MLRS) {
        rrdset_next(MLRS);
        for (auto &P : DimensionsMap) {
            Dimension *D = P.second;
            D->updateMLRD(MLRS);
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
