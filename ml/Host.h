// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"

namespace ml {

class Chart;
class Unit;

class Host {
public:
    Host(RRDHOST *RH) : RH(RH) {
        CreationTime = SteadyClock::now();
    }

    std::string uid() const { return RH->hostname; }
    const char *c_uid() const { return RH->hostname; }

    std::vector<Unit *> getUnits();

    void train();
    void predict();

private:
    void updateCharts();
    void updateMLStats();

private:
    RRDHOST *RH;
    std::map<RRDSET *, Chart *> ChartsMap;
    std::mutex Mutex;

    TimePoint CreationTime;

    collected_number NumUnits;
    collected_number NumTrainedUnits;
    collected_number NumPredictedUnits;
    collected_number NumAnomalies;

    usec_t PredictionDuration;
};

}

#endif /* ML_HOST_H */
