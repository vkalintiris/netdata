// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_UNIT_H
#define ML_UNIT_H

#include "ml-private.h"

#include "Config.h"
#include "RollingBitCounter.h"

namespace ml {

class Host;

class Unit {
public:
    Unit(RRDDIM *RD) :
        RD(RD),
        KM(KMeans()), AnomalyScore(0.0),
        HasModel(false), ShouldTrain(false),
        LastTrainedAt(SteadyClock::now()),
        RBC(5), BitCounter(0) {}

    int updateEvery() const {
        return RD->update_every;
    }

    bool isAnomalous() {
        return AnomalyScore > Cfg.AnomalyScoreThreshold;
    }

    double anomalyRate(size_t WindowLength) {
        double Rate = static_cast<double>(BitCounter) / WindowLength;
        BitCounter = RBC.numSetBits();
        return Rate;
    }

    std::pair<CalculatedNumber *, unsigned>
    getCalculatedNumbers(unsigned N, unsigned MinN);

    bool train(TimePoint &Now);
    void predict();

private:
    RRDDIM *RD;

    KMeans KM;
    CalculatedNumber AnomalyScore;

    bool HasModel;
    bool ShouldTrain;

    TimePoint LastTrainedAt;

    RollingBitCounter RBC;
    size_t BitCounter;

    std::mutex Mutex;

    // RBC{MinLength} + Counter = 0
    // if !RBC.isFilled() -> RBC.insert() and Counter += AnomalyBit;
    // else -> RBC.insert() and Counter += AnomalyBit;
    // Above -> Below: (Counter / WindowLength) and (Counter = RBC.numSetBits())

    friend class Host;
};

}

#endif /* ML_UNIT_H */
