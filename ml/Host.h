// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"
#include "Unit.h"
#include "Database.h"

namespace ml {

class AnomalyStatusChart {
public:
    AnomalyStatusChart(const std::string Name);

    void update(collected_number NumTotalUnits,
                collected_number NumAnomalousUnits,
                collected_number AnomalyRate);

private:
    RRDSET *RS;

    RRDDIM *NumTotalUnitsRD;
    RRDDIM *NumAnomalousUnitsRD;
    RRDDIM *AnomalyRateRD;
};

template<typename BaseT>
class DetectableHost {
public:
    void detect();

    void startAnomalyDetectionThreads();
    void stopAnomalyDetectionThreads();

private:
    std::thread TrainingThread; // = std::thread(&Host::trainUnits, this);
    RollingBitWindow RBW{5, 3};
    CalculatedNumber AnomalyRate{0.0};
};

template<typename BaseT>
class TrainableHost : public DetectableHost<BaseT> {
public:
    void train();
    CalculatedNumber predict();

private:
    void trainOne(TimePoint &Now);

private:
    AnomalyStatusChart ASC{"host_anomaly_status"};
};

class Host : public TrainableHost<Host> {
public:
    Host(RRDHOST *RH) : RH(RH) {}

    void addDimension(Dimension *D);
    void removeDimension(Dimension *D);

    size_t getNumDimensions() const {
        return NumDimensions;
    }

    void forEachDimension(std::function<bool(Dimension *)> Func);

private:
    RRDHOST *RH;

    std::mutex Mutex;
    std::map<RRDDIM *, Dimension *> DimensionsMap;

    std::atomic<size_t> NumDimensions{0};
};

}

#endif /* ML_HOST_H */
