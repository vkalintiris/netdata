// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_UNIT_H
#define ML_UNIT_H

#include "ml-private.h"

#include "Config.h"

namespace ml {

class Host;

class Unit {
public:
    Unit(RRDDIM *RD) :
        RD(RD),
        KM(KMeans()), AnomalyScore(0.0),
        HasModel(false), ShouldTrain(false),
        LastTrainedAt(SteadyClock::now()) { }

    int updateEvery() const {
        return RD->update_every;
    }

    bool isAnomalous() {
        return AnomalyScore > Cfg.AnomalyScoreThreshold;
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

    std::mutex Mutex;

    friend class Host;
};

}

#endif /* ML_UNIT_H */
