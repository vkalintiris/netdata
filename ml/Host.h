// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"
#include "Dimension.h"
#include "Chart.h"
#include "Database.h"

namespace ml {

template<typename BaseT>
class DetectableHost {
public:
    void detect();
    void detectOnce();

    void startAnomalyDetectionThreads();
    void stopAnomalyDetectionThreads();

    void startQueryThread();
    void stopQueryThread();
    void query();

private:
    std::thread TrainingThread;
    std::thread DetectionThread;
    std::thread QueryThread;

    RollingBitWindow RBW{
        static_cast<size_t>(Cfg.ADWindowSize),
        static_cast<size_t>(Cfg.ADWindowSize * Cfg.ADWindowRateThreshold)
    };
    CalculatedNumber AnomalyRate{0.0};

    Database DB{Cfg.AnomalyDBPath};
};

template<typename BaseT>
class TrainableHost : public DetectableHost<BaseT> {
public:
    void train();
    void trainOne(TimePoint &Now);

    CalculatedNumber predict();
};

class Host : public TrainableHost<Host> {
public:
    Host(RRDHOST *RH) : RH(RH) {}

    RRDHOST *getRH() { return RH; }

    std::string getUUID() {
        char S[UUID_STR_LEN];
        uuid_unparse_lower(RH->host_uuid, S);
        return S;
    }

    void addChart(Chart *C);
    void removeChart(Chart *C);
    bool forEachDimension(std::function<bool(Dimension *)> Func);

    void updateMLCharts();

public:
    std::atomic<size_t> NumDimensions{0};

private:
    RRDHOST *RH;

    std::mutex Mutex;
    std::map<RRDSET *, Chart *> ChartsMap;
};

}

#endif /* ML_HOST_H */
