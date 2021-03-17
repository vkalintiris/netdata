// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_UNIT_H
#define ML_UNIT_H

#include "ml-private.h"

namespace ml {

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

};

#endif /* ML_UNIT_H */
