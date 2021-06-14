// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"
#include "Unit.h"
#include "Database.h"

namespace ml {

class Host {
public:
    Host(RRDHOST *RH) :
        RH(RH), AnomalyRateRD(nullptr), DB(Cfg.AnomalyDBPath) {}

    void addUnit(Unit *U);
    void removeUnit(Unit *U);

    void runMLThreads();
    void stopMLThreads();

private:
    void trainUnits();
    void trackAnomalyStatus();

private:
    RRDHOST *RH;
    RRDDIM *AnomalyRateRD;

    Database DB;

    std::mutex Mutex;
    std::map<RRDDIM *, Unit *> UnitsMap;

    std::thread TrainingThread;
    std::thread TrackAnomalyStatusThread;
};

}

#endif /* ML_HOST_H */
