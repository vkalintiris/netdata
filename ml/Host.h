// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"
#include "Unit.h"

namespace ml {

class Host {
public:
    Host(RRDHOST *RH) : RH(RH) { }

    void runMLThreads();
    void stopMLThreads();

    void addUnit(Unit *U);
    void removeUnit(Unit *U);

    char *findAnomalyEvents(time_t After, time_t Before);

private:
    void trainUnits();
    void trackAnomalyStatus();

private:
    RRDHOST *RH;

    std::thread TrainingThread;
    std::thread TrackAnomalyStatusThread;

    std::mutex Mutex;
    std::map<RRDDIM *, Unit *> UnitsMap;
};

}

#endif /* ML_HOST_H */
