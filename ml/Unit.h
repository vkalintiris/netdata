// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_UNIT_H
#define ML_UNIT_H

#include "ml-private.h"

#include "Config.h"

namespace ml {

class Host;

class Unit {
public:
    Unit(RRDDIM *RD) : RD(RD) {
        KM = KMeans();
        AnomalyScore = 0.0;

        HasModel = false;
        Trained = false;
        Predicted = false;
        ShouldTrain = false;

        LastTrainedAt = SteadyClock::now();
    }

    int updateEvery() const {
        return RD->update_every;
    }

    bool isTrained() const {
        return Trained;
    }

    bool isPredicted() const {
        return Predicted;
    }

    bool isAnomalous() const {
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
    bool Trained;
    bool Predicted;
    bool HasModel;
    bool ShouldTrain;

    TimePoint LastTrainedAt;

    std::mutex Mutex;

    friend class Host;
};

}

#endif /* ML_UNIT_H */
