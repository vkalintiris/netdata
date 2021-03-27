// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_CONFIG_H
#define ML_CONFIG_H

#include "ml-private.h"

namespace ml {

using SteadyClock = std::chrono::steady_clock;
using TimePoint = std::chrono::time_point<SteadyClock>;
using Seconds = std::chrono::seconds;
template<typename T>
using Duration = std::chrono::duration<Seconds, T>;

class Host;

/*
 * Global configuration shared between the prediction and the training
 * threads.
 */
class Config {
public:
    std::vector<char> *Buffer;
    struct SPDR_Context *SPDR;
    std::ofstream LogFp;

    std::chrono::seconds UpdateEvery;

    // Time window over which we should train our models.
    time_t TrainSecs;

    // How often we want to retrain our models.
    time_t TrainEvery;

    // Feature extraction parameters.
    unsigned DiffN;
    unsigned SmoothN;
    unsigned LagN;

    // List of hosts that we want to train/predict.
    std::map<RRDHOST *, Host *> Hosts;

    // Lock to allow safe access to list of hosts between training/prediction thread.
    netdata_rwlock_t HostsLock;

    // Set of anomaly score sets.
    std::set<RRDSET *> MLSets;

    // Simple expression that allows us to skip certain charts from training.
    SIMPLE_PATTERN *SP_ChartsToSkip;

    // Option to allow us to disable the prediction thread.
    bool DisablePredictionThread;

    bool Initialized;

    void updateHosts();

    void updateCharts();
};

extern Config Cfg;

};

#endif /* ML_CONFIG_H */
