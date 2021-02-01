// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_PRIVATE_H
#define ML_PRIVATE_H

#include "kmeans/KMeans.h"

#include <algorithm>
#include <cmath>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

extern "C" {

#include "daemon/common.h"

};

namespace ml {

class Unit;
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

/*
 * A ML unit wraps the pointer to the dimension that we want to train/predict.
*/
class Unit {
public:
    Unit(RRDDIM *RD) :
        RD(RD), MLRD(nullptr), KM(KMeans()), AnomalyScore(0.0),
        LastTrainedAt(now_realtime_sec() + Cfg.TrainSecs) {
        netdata_rwlock_init(&RwLock);

        std::stringstream SS;
        SS << RD->rrdset->id << "." << RD->id;
        UniqueID = SS.str();
    };

    std::string uid() const {
        return UniqueID;
    }

    const char *c_uid() const {
        return UniqueID.c_str();
    }

    bool isObsolete() const {
        return rrddim_flag_check(RD, RRDDIM_FLAG_ARCHIVED) ||
               rrddim_flag_check(RD, RRDDIM_FLAG_OBSOLETE);
    }

    int updateEvery() const {
        return RD->update_every;
    };

    bool shouldTrain() const {
        return LastTrainedAt + Cfg.TrainEvery < now_realtime_sec();
    }

    bool operator<(const Unit& RHS) const {
        return RD < RHS.RD;
    }

    bool operator==(const Unit& RHS) const {
        return RD == RHS.RD;
    }

    bool operator!=(const Unit& RHS) const {
        return RD == RHS.RD;
    }

    bool train();
    bool predict();

    CalculatedNumber getAnomalyScore() const {
        return AnomalyScore;
    };

    void wrLock() {
        netdata_rwlock_wrlock(&RwLock);
    }

    int rdTryLock() {
        return netdata_rwlock_tryrdlock(&RwLock);
    }

    void unLock() {
        netdata_rwlock_unlock(&RwLock);
    }

    RRDDIM *getDim() {
        return RD;
    }

    void updateMLUnit(RRDSET *MLRS) {
        if (MLRD) {
            rrddim_set_by_pointer(MLRS, MLRD, getAnomalyScore() * 10000.0);
            return;
        }

        MLRD = rrddim_add(MLRS, RD->id, NULL, 1, 10000, RRD_ALGORITHM_ABSOLUTE);

        rrddim_flag_clear(MLRD, RRDDIM_FLAG_HIDDEN);
        if (rrddim_flag_check(RD, RRDDIM_FLAG_HIDDEN))
            rrddim_flag_set(MLRD, RRDDIM_FLAG_HIDDEN);
    };

private:
    RRDDIM *RD;
    RRDDIM *MLRD;

    KMeans KM;
    CalculatedNumber AnomalyScore;
    time_t LastTrainedAt;

    std::string UniqueID;
    bool Trained, Predicted;

    netdata_rwlock_t RwLock;
};

class Chart {
public:
    Chart(RRDSET *RS) : RS(RS), MLRS(nullptr) {
        netdata_rwlock_init(&UnitsLock);
    }

    void updateUnits();
    void updateMLChart();

public:
    RRDSET *RS;
    RRDSET *MLRS;

    std::map<RRDDIM *, Unit *> UnitsMap;
    netdata_rwlock_t UnitsLock;
};

class Host {
public:
    Host(RRDHOST *RH, std::map<RRDSET *, Chart *> &ChartsMap)
        : RH(RH), ChartsMap(ChartsMap) {};

    void updateCharts();

public:
    RRDHOST *RH;
    std::map<RRDSET *, Chart *> &ChartsMap;
};

class Window {
public:
    Window(Unit *U, unsigned NumSamples) :
        U(U), NumSamples(NumSamples),
        NumCollected(0), NumEmpty(0), NumReset(0) {};

    CalculatedNumber *getCalculatedNumbers();

    double ratioFilled() const {
        return static_cast<double>(NumCollected) / NumSamples;
    }

public:
    Unit *U;

    unsigned NumSamples;
    unsigned NumCollected;
    unsigned NumEmpty;
    unsigned NumReset;
};

void trainMain(struct netdata_static_thread *Thread);
void predictMain(struct netdata_static_thread *Thread);

};

#endif /* ML_PRIVATE_H */
