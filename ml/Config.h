// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_CONFIG_H
#define ML_CONFIG_H

#include "ml-private.h"

namespace ml {

class Config {
public:
    Millis TrainSecs;
    Millis MinTrainSecs;
    Millis TrainEvery;

    unsigned DiffN, SmoothN, LagN;

    SIMPLE_PATTERN *SP_HostsToSkip;
    SIMPLE_PATTERN *SP_ChartsToSkip;

    double AnomalyScoreThreshold;
    double AnomalousHostRateThreshold;

    double ADWindowSize;
    double ADWindowRateThreshold;
    double ADUnitRateThreshold;
};

extern Config Cfg;

}

#endif /* ML_CONFIG_H */
