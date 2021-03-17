// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_UNIT_H
#define ML_UNIT_H

#include "ml-private.h"

namespace ml {

struct UnitComp;

/*
 * A ML unit wraps the pointer to the dimension that we want to train/predict.
*/
class Unit {
public:
    Unit(RRDDIM *RD, Millis TrainSecs, Millis TrainEvery,
         unsigned DiffN, unsigned SmoothN, unsigned LagN) :
        RD(RD), SetPtr(reinterpret_cast<uintptr_t>(RD->rrdset)),
        TrainSecs(TrainSecs), TrainEvery(TrainEvery),
        DiffN(DiffN), SmoothN(SmoothN), LagN(LagN),
        MLRD(nullptr),
        KM(KMeans()), AnomalyScore(0.0),
        Trained(false), Predicted(false) {
        LastTrainedAt = SteadyClock::now() + TrainSecs;

        std::stringstream UidSS;
        UidSS << RD->rrdset->id << "." << RD->id;
        UniqueID = UidSS.str();
    }

    std::string uid() const { return UniqueID; }
    const char *c_uid() const { return UniqueID.c_str(); }

    bool isObsolete() const {
        return rrddim_flag_check(RD, RRDDIM_FLAG_ARCHIVED) ||
               rrddim_flag_check(RD, RRDDIM_FLAG_OBSOLETE);
    }

    int updateEvery() const { return RD->update_every; };
    CalculatedNumber getAnomalyScore() const { return AnomalyScore; };
    RRDDIM *getDim() { return RD; }

    void updateMLUnit(RRDSET *MLRS);

    bool shouldTrain() const;
    bool train();
    bool predict();

    friend UnitComp;

private:
    RRDDIM *RD;
    uintptr_t SetPtr;

    Millis TrainSecs;
    Millis TrainEvery;

    unsigned DiffN;
    unsigned SmoothN;
    unsigned LagN;

    RRDDIM *MLRD;

    KMeans KM;
    CalculatedNumber AnomalyScore;
    bool Trained;
    bool Predicted;

    TimePoint LastTrainedAt;
    std::string UniqueID;
};

struct UnitComp {
    bool operator()(const Unit *LHS, const Unit *RHS) {
        if (LHS->SetPtr != RHS->SetPtr)
            return LHS->SetPtr > RHS->SetPtr;

        // make_heap returns a max heap
        return LHS->LastTrainedAt > RHS->LastTrainedAt;
    }
};

}

#endif /* ML_UNIT_H */
