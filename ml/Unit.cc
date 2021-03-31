// SPDX-License-Identifier: GPL-3.0-or-later

#include "Unit.h"
#include "Window.h"

using namespace ml;

void Unit::updateMLUnit(RRDSET *MLRS) {
    if (MLRD) {
        rrddim_set_by_pointer(MLRS, MLRD, getAnomalyScore() * 10000.0);
        return;
    }

    MLRD = rrddim_add(MLRS, RD->id, NULL, 1, 10000, RRD_ALGORITHM_ABSOLUTE);

    rrddim_flag_clear(MLRD, RRDDIM_FLAG_HIDDEN);
    if (rrddim_flag_check(RD, RRDDIM_FLAG_HIDDEN))
        rrddim_flag_set(MLRD, RRDDIM_FLAG_HIDDEN);
}

bool Unit::shouldTrain() const {
    return (LastTrainedAt + TrainEvery) < SteadyClock::now();
}

/*
 * Run KMeans on the unit.
 */
bool ml::Unit::train() {
    if (!shouldTrain())
        return false;

    unsigned NumSamples = TrainSecs / Millis{updateEvery() * 1000};

    Window W = Window(this, NumSamples);
    CalculatedNumber *CNs = W.getCalculatedNumbers();

    LastTrainedAt = SteadyClock::now();

    if (W.ratioFilled() < 0.8) {
        Trained = false;
        error("%s -%straining window: %lf, score: %lf",
              c_uid(), Trained ? " " : " sparse ", W.ratioFilled(), AnomalyScore);
    } else {
        SamplesBuffer SB = SamplesBuffer(CNs, W.NumCollected, 1,
                                         DiffN, SmoothN, LagN);
        KM.train(SB);
        Trained = true;
    }


    delete[] CNs;
    return Trained;
}

/*
 * Calculate the anomaly score of the unit.
 */
bool ml::Unit::predict() {
    if (!Trained)
        return false;

    unsigned NumSamples = DiffN + SmoothN + LagN;

    Window W = Window(this, NumSamples);
    CalculatedNumber *CNs = W.getCalculatedNumbers();

    if (W.NumCollected != W.NumSamples) {
        Predicted = false;
        error("%s -%sprediction window: %lf, score: %lf",
              c_uid(), Predicted ? " " : " sparse ", W.ratioFilled(), AnomalyScore);
    } else {
        SamplesBuffer SB = SamplesBuffer(CNs, W.NumCollected, 1,
                                         DiffN, SmoothN, LagN);

        AnomalyScore = KM.anomalyScore(SB);
        Predicted = true;
    }

    delete[] CNs;
    return true;
}
