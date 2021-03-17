// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_CHART_H
#define ML_CHART_H

#include "ml-private.h"

namespace ml {

class Unit;

class Chart {
public:
    Chart(RRDSET *RS) : RS(RS), MLRS(nullptr) {}

    void updateUnits(Millis TrainSecs, Millis TrainEvery,
                     unsigned DiffN, unsigned SmoothN, unsigned LagN);

    void updateMLChart();

public:
    RRDSET *RS;
    RRDSET *MLRS;

    std::map<RRDDIM *, Unit *> UnitsMap;
};

}

#endif /* ML_CHART_H */
