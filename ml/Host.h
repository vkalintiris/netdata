// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"
#include "Unit.h"
#include "Database.h"

namespace ml {

class AnomalyStatusChart {
public:
    AnomalyStatusChart(const std::string Name);

    void update(collected_number NumTotalUnits, collected_number NumAnomalousUnits);

private:
    RRDSET *RS;

    RRDDIM *NumTotalUnitsRD;
    RRDDIM *NumAnomalousUnitsRD;
    RRDDIM *AnomalyRateRD;
};

class Host {
public:
    Host(RRDHOST *RH) : RH(RH) {}

    void addUnit(Unit *U);
    void removeUnit(Unit *U);

    void runMLThreads();
    void stopMLThreads();

private:
    void trainUnits();
    void detectAnomalies();

private:
    RRDHOST *RH;

    std::mutex Mutex;
    std::map<RRDDIM *, Unit *> UnitsMap;

    std::thread TrainingThread;
    std::thread AnomalyDetectionThread;
};

}

#endif /* ML_HOST_H */
