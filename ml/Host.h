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

private:
    void predictUnits();
    void trainUnits();

private:
    RRDHOST *RH;

    std::thread TrainingThread;
    std::thread PredictionThread;

    std::mutex Mutex;
    std::vector<Unit *> Units;
};

}

#endif /* ML_HOST_H */
