// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_ANOMALY_DETECTOR_H
#define ML_ANOMALY_DETECTOR_H

#include "ml-private.h"

namespace ml {

using AnomalyEvent = std::pair<time_t, time_t>;

typedef struct {
    std::string Name;
    std::vector<char> AnomalyStatus;
    CalculatedNumber AnomalyRate;
} AnomalyEventInfo;

class AnomalyDetector {
public:
    AnomalyDetector(time_t AfterT, time_t BeforeT)
        : AfterT(AfterT), BeforeT(BeforeT) { }

    std::vector<AnomalyEvent>
    getAnomalyEvents(RRDDIM *RD, unsigned MinSize, double MinRate);

    AnomalyEventInfo getAnomalyEventInfo(RRDDIM *RD);

private:
    std::vector<bool> getAnomalyBitVector(RRDDIM *RD, bool IsAnomalyRateRD = false);

private:
    time_t AfterT;
    time_t BeforeT;
};

}

#endif /* ML_ANOMALY_DETECTOR_H */
