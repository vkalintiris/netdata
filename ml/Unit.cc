// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

using namespace ml;

/*
 * Run KMeans on the unit.
 */
bool ml::Unit::train() {
    unsigned NumSamples = Cfg.TrainEvery / updateEvery();

    Window W = Window(this, NumSamples);
    CalculatedNumber *CNs = W.getCalculatedNumbers();

    LastTrainedAt = now_realtime_sec();

    if (W.ratioFilled() < 0.8) {
        info("%s - sparse training window: %lf", c_uid(), W.ratioFilled());
        Trained = false;
    } else {
        SamplesBuffer SB = SamplesBuffer(CNs, W.NumCollected, 1,
                                         Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);
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

    unsigned NumSamples = Cfg.DiffN + Cfg.SmoothN + Cfg.LagN;

    Window W = Window(this, NumSamples);
    CalculatedNumber *CNs = W.getCalculatedNumbers();

    if (W.NumCollected != W.NumSamples) {
        info("%s - sparse prediction window: %lf", c_uid(), W.ratioFilled());
        Predicted = false;
    } else {
        SamplesBuffer SB = SamplesBuffer(CNs, W.NumCollected, 1,
                                         Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);

        // Waiting for the next iteration is fine.
        AnomalyScore = KM.anomalyScore(SB);
        Predicted = true;
    }

    delete[] CNs;
    return true;
}
