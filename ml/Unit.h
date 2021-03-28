// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_UNIT_H
#define ML_UNIT_H

#include "ml-private.h"

namespace ml {

static unsigned Counter = 0;
struct UnitComp;

/*
 * A ML unit wraps the pointer to the dimension that we want to train/predict.
*/
class Unit {
public:
    Unit(RRDDIM *RD, time_t TrainSecs, time_t TrainEvery,
         unsigned DiffN, unsigned SmoothN, unsigned LagN) :
        RD(RD), SetPtr(reinterpret_cast<uintptr_t>(RD->rrdset)),
        TrainSecs(TrainSecs), TrainEvery(TrainEvery),
        DiffN(DiffN), SmoothN(SmoothN), LagN(LagN),
        MLRD(nullptr),
        KM(KMeans()), AnomalyScore(0.0) {
        netdata_rwlock_init(&RwLock);

        Counter += 1;
        Counter %= TrainEvery;
        LastTrainedAt = now_realtime_sec() + TrainSecs + Counter;

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

    bool shouldTrain() const;

    bool train();

    bool predict();

    friend UnitComp;

private:
    RRDDIM *RD;
    uintptr_t SetPtr;

    time_t TrainSecs;
    time_t TrainEvery;

    unsigned DiffN;
    unsigned SmoothN;
    unsigned LagN;

    RRDDIM *MLRD;

    KMeans KM;
    CalculatedNumber AnomalyScore;
    time_t LastTrainedAt;

    std::string UniqueID;
    bool Trained, Predicted;

    netdata_rwlock_t RwLock;
};

struct UnitComp {
    bool operator()(const Unit *LHS, const Unit *RHS) {
        if (LHS->SetPtr != RHS->SetPtr)
            return LHS->RD->rrdset < RHS->RD->rrdset;

        return LHS < RHS;
    }
};

};

#endif /* ML_UNIT_H */
