// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_UNIT_H
#define ML_UNIT_H

#include "ml-private.h"
#include "Host.h"

#include "Config.h"

namespace ml {

class Unit {
public:
    Unit(RRDDIM *RD) : RD(RD) {
        KM = KMeans();
        AnomalyScore = 0.0;

        HasModel = false;
        Trained = false;
        Predicted = false;

        LastTrainedAt = SteadyClock::now();
    }

    RRDDIM *getDim() const {
        return RD;
    }

    std::string getFamily() const {
        return RD->rrdset->family;
    }

    int updateEvery() const {
        return RD->update_every;
    }

    CalculatedNumber getAnomalyScore() const {
        return AnomalyScore;
    }

    bool isAnomalous() const {
        return AnomalyScore > Cfg.AnomalyScoreThreshold;
    }

    bool isTrained() const {
        return Trained;
    }

    bool isPredicted() const {
        return Predicted;
    }

    bool shouldTrain() const {
        return (LastTrainedAt + Cfg.TrainEvery) < SteadyClock::now();
    }

    KMeans &getKMeansRef() { return KM; }

    std::pair<CalculatedNumber *, unsigned>
    getCalculatedNumbers(unsigned N, unsigned MinN);

    void train();
    void predict();

private:
    RRDDIM *RD;

    KMeans KM;
    CalculatedNumber AnomalyScore;
    bool Trained;
    bool Predicted;
    bool HasModel;

    TimePoint LastTrainedAt;
};

}

#endif /* ML_UNIT_H */
