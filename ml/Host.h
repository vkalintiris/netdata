// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"
#include "Unit.h"

namespace ml {

class Host {
public:
    Host(RRDHOST *RH) : RH(RH), AnomalyRateRD(nullptr) { }

    void addUnit(Unit *U);
    void removeUnit(Unit *U);

    void runMLThreads();
    void stopMLThreads();

    std::string getAnomalyEventsJson(time_t After, time_t Before);
    std::string getAnomalyEventInfoJson(time_t After, time_t Before);

private:
    void trainUnits();
    void trackAnomalyStatus();

private:
    RRDHOST *RH;
    RRDDIM *AnomalyRateRD;

    std::mutex Mutex;
    std::map<RRDDIM *, Unit *> UnitsMap;

    std::thread TrainingThread;
    std::thread TrackAnomalyStatusThread;
};

}

#endif /* ML_HOST_H */
