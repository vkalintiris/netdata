// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_CHART_H
#define ML_CHART_H

#include "ml-private.h"

namespace ml {

class Chart {
public:
    Chart(RRDSET *RS) : RS(RS), MLRS(nullptr) {
        netdata_rwlock_init(&UnitsLock);
    }

    size_t numUnits() const { return UnitsMap.size(); }

    void updateUnits(time_t TrainSecs, time_t TrainEvery,
                     unsigned DiffN, unsigned SmoothN, unsigned LagN);

    void updateMLChart();

public:
    RRDSET *RS;
    RRDSET *MLRS;

    std::map<RRDDIM *, Unit *> UnitsMap;
    netdata_rwlock_t UnitsLock;
};

};

#endif /* ML_CHART_H */
