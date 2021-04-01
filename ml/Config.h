// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_CONFIG_H
#define ML_CONFIG_H

#include "ml-private.h"

namespace ml {

/*
 * Global configuration shared between the prediction and the training
 * threads.
 */
class Config {
public:
    bool Initialized;

    Millis UpdateEvery;

    // Time window over which we should train our models.
    Millis TrainSecs;

    // How often we want to retrain our models.
    Millis TrainEvery;

    // Feature extraction parameters.
    unsigned DiffN, SmoothN, LagN;

    // Simple expression that allows us to skip certain hosts from training.
    SIMPLE_PATTERN *SP_HostsToSkip;

    // Simple expression that allows us to skip certain charts from training.
    SIMPLE_PATTERN *SP_ChartsToSkip;
};

extern Config Cfg;

}

#endif /* ML_CONFIG_H */
