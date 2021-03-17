// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_CONFIG_H
#define ML_CONFIG_H

#include "ml-private.h"

namespace ml {

class Chart;

/*
 * Global configuration shared between the prediction and the training
 * threads.
 */
struct Config {
    // Time window over which we should train our models.
    time_t TrainSecs;

    // How often we want to retrain our models.
    time_t TrainEvery;

    // Feature extraction parameters.
    unsigned DiffN;
    unsigned SmoothN;
    unsigned LagN;

    // Every RRD set is mapped to a chart that will train each dimension
    // and commit the anomaly scores in a new RRD set.
    std::map<RRDSET *, Chart *> ChartsMap;

    // Lock to allow prediction/training threads to iterate the charts map
    // safely.
    netdata_rwlock_t ChartsMapLock;

    // Set of anomaly score sets.
    std::set<RRDSET *> MLSets;

    // Simple expression that allows us to skip certain charts from training.
    SIMPLE_PATTERN *SP_ChartsToSkip;

    // Option to allow us to disable the prediction thread.
    bool DisablePredictionThread;

    bool Initialized;
};

extern Config Cfg;

};

#endif /* ML_CONFIG_H */
