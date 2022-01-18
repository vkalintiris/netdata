// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "BitRateWindow.h"
#include "Config.h"
#include "Database.h"
#include "Dimension.h"

#include "ml-private.h"

namespace ml {

class RrdHost {
public:
    RrdHost(RRDHOST *RH) : RH(RH) {
        if (Cfg.EnableDBEngine) {
            AnomalyRateRS = rrdset_create(
                    RH,
                    "ar_type",
                    "ar_id",
                    NULL, // name
                    "ar_family",
                    NULL, // ctx
                    "ar_title",
                    "ar_units",
                    "ar_plugin",
                    "ar_module",
                    39189,
                    Cfg.DBEngineAnomalyRateEvery,
                    RRDSET_TYPE_LINE
                    );

            rrdset_flag_set(AnomalyRateRS, RRDSET_FLAG_HIDDEN);
        }
    }

    RRDHOST *getRH() { return RH; }

    unsigned updateEvery() { return RH->rrd_update_every; }

    std::string getUUID() {
        char S[UUID_STR_LEN];
        uuid_unparse_lower(RH->host_uuid, S);
        return S;
    }

    void addDimension(Dimension *D);
    void removeDimension(Dimension *D);

    void getConfigAsJson(nlohmann::json &Json) const;

    virtual ~RrdHost() {};

protected:
    RRDHOST *RH;
    RRDSET *AnomalyRateRS;

    // Protect dimension and lock maps
    std::mutex Mutex;

    std::mutex TRUMutex;
    struct rusage TrainingRU;

    std::unordered_map<RRDDIM *, Dimension *> DimensionsMap;
    std::unordered_map<Dimension *, std::mutex> LocksMap;
};

class TrainableHost : public RrdHost {
public:
    TrainableHost(RRDHOST *RH) : RrdHost(RH) {}

    void train();

private:
    std::pair<Dimension *, Duration<double>> findDimensionToTrain(const TimePoint &NowTP);
    void trainDimension(Dimension *D, const TimePoint &NowTP);

};

class DetectableHost : public TrainableHost {
public:
    DetectableHost(RRDHOST *RH) : TrainableHost(RH) {
        std::pair<int, int> TheLastSavedRange;
        if(DB.getTheLastSavedAnomalyInfoRange(TheLastSavedRange, getUUID())) {
            LastSavedBefore = TheLastSavedRange.second;
        }
    }

    void startAnomalyDetectionThreads();
    void stopAnomalyDetectionThreads();

    template<typename ...ArgTypes>
    bool getAnomalyInfo(ArgTypes&&... Args) {
        return DB.getAnomalyInfo(Args...);
    }

    template<typename ...ArgTypes>
    bool getAnomaliesInRange(ArgTypes&&... Args) {
        return DB.getAnomaliesInRange(Args...);
    }

    template<typename ...ArgTypes>
    bool getAnomalyRateInfoInRange(ArgTypes&&... Args) {
        return DB.getAnomalyRateInfoInRange(Args...);
    }

    void getDetectionInfoAsJson(nlohmann::json &Json) const;

    time_t getLastSavedBefore() { return LastSavedBefore; }
    void setLastSavedBefore(time_t lastSavedBefore) { LastSavedBefore = lastSavedBefore; }
    void getAnomalyRateInfoCurrentRange(std::vector<std::pair<std::string, double>> &V, time_t After, time_t Before);
    void getAnomalyRateInfoMixedRange(std::vector<std::pair<std::string, double>> &V, std::string HostUUID, time_t After, time_t Before);

private:
    void detect();
    void detectOnce();

private:
    std::thread TrainingThread;
    std::thread DetectionThread;

    BitRateWindow BRW{
        static_cast<size_t>(Cfg.ADMinWindowSize),
        static_cast<size_t>(Cfg.ADMaxWindowSize),
        static_cast<size_t>(Cfg.ADIdleWindowSize),
        static_cast<size_t>(Cfg.ADMinWindowSize * Cfg.ADWindowRateThreshold)
    };

    CalculatedNumber WindowAnomalyRate{0.0};

    size_t NumAnomalousDimensions{0};
    size_t NumNormalDimensions{0};
    size_t NumTrainedDimensions{0};

    unsigned AnomalyRateTimer{0};

    Database DB{Cfg.AnomalyDBPath};

    /*the counter variable to downcount the time window for anomaly bit counting*/
    size_t AnomalyBitCounterWindow{Cfg.SQLiteAnomalyRateEvery};
    time_t LastSavedBefore{0};
};

using Host = DetectableHost;

} // namespace ml

#endif /* ML_HOST_H */
