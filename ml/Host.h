// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"

namespace ml {

class Host {
public:
    Host(RRDHOST *RH) : RH(RH) { }

    void runMLThreads();
    void stopMLThreads();

private:
    void predictUnits();
    void trainUnits();

private:
    RRDHOST *RH;

    std::thread TrainingThread;
    std::thread PredictionThread;
};

}

#endif /* ML_HOST_H */
