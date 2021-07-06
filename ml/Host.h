// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"

#include "Dimension.h"
#include "Database.h"

namespace ml {

class RrdHost {
public:
    RrdHost(RRDHOST *RH) : RH(RH) {}

    RRDHOST *getRH() { return RH; }

    std::string getUUID() {
        char S[UUID_STR_LEN];
        uuid_unparse_lower(RH->host_uuid, S);
        return S;
    }

    void addDimension(Dimension *D);
    void removeDimension(Dimension *D);

    virtual ~RrdHost() {}

public:
    RRDHOST *RH;

    std::mutex Mutex;
    std::map<RRDDIM *, Dimension *> DimensionsMap;
};

class TrainableHost : public RrdHost {
public:
    TrainableHost(RRDHOST *RH) : RrdHost(RH) {}

    void train();
    void trainOne(TimePoint &Now);
};

class DetectableHost : public TrainableHost {
public:
    DetectableHost(RRDHOST *RH) : TrainableHost(RH) {}

    void detect();
    void detectOnce();

    void startAnomalyDetectionThreads();
    void stopAnomalyDetectionThreads();

private:
    std::thread TrainingThread;
    std::thread DetectionThread;

    RollingBitWindow RBW{
        static_cast<size_t>(Cfg.ADWindowSize),
        static_cast<size_t>(Cfg.ADWindowSize * Cfg.ADWindowRateThreshold)
    };
    CalculatedNumber AnomalyRate{0.0};

    Database DB{Cfg.AnomalyDBPath};
};

using Host = DetectableHost;

}

#endif /* ML_HOST_H */
