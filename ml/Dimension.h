// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_DIMENSION_H
#define ML_DIMENSION_H

#include "ml-private.h"

#include "Config.h"
#include "RollingBitCounter.h"

namespace ml {

const std::error_category &MLErrorCategory();

enum class MLError {
    Success = 0,
    TryLockFailed,
    MissingData,
    ShouldNotTrainNow,
    NoModel,
};

inline std::error_code make_error_code(MLError E) {
    return std::error_code(static_cast<int>(E), MLErrorCategory());
};

} // namespace ml

namespace std {

template<>
struct is_error_code_enum<ml::MLError> : std::true_type {};

} // namespace std

namespace ml {

class RrdDimension {
public:
    RrdDimension(RRDDIM *RD) : RD(RD), Ops(&RD->state->query_ops) {}

    RRDDIM *getRD() const { return RD; }

    std::string getID() const {
        std::stringstream SS;
        SS << RD->rrdset->id << "|" << getName();
        return SS.str();
    }

    const char *getName() const { return RD->name; }
    Seconds updateEvery() const { return Seconds{RD->update_every}; }

    time_t latestTime() { return Ops->latest_time(RD); }
    time_t oldestTime() { return Ops->oldest_time(RD); }

    std::pair<CalculatedNumber *, size_t>
    getCalculatedNumbers(size_t MinN, size_t MaxN);

    virtual ~RrdDimension() {}

protected:
    std::mutex Mutex;

private:
    RRDDIM *RD;
    struct rrddim_volatile::rrddim_query_ops *Ops;

};

class TrainableDimension : public RrdDimension {
public:
    TrainableDimension(RRDDIM *RD) : RrdDimension(RD) {}

    MLError trainModel(TimePoint &Now);
    CalculatedNumber computeAnomalyScore(SamplesBuffer &SB);

private:
    std::pair<CalculatedNumber *, unsigned> getNumbersForTraining() {
        unsigned MinN = Cfg.MinTrainSecs / updateEvery();
        unsigned MaxN = Cfg.TrainSecs / updateEvery();

        return RrdDimension::getCalculatedNumbers(MinN, MaxN);
    }

protected:
    std::atomic<bool> HasModel{false};

private:
    KMeans KM;
    TimePoint LastTrainedAt{SteadyClock::now()};
};

class PredictableDimension : public TrainableDimension {
public:
    PredictableDimension(RRDDIM *RD) : TrainableDimension(RD) {}

    std::pair<MLError, bool> predict();

    bool isAnomalous() { return AnomalyBit; }

    void addValue(CalculatedNumber Value, bool Exists);

private:
    CalculatedNumber AnomalyScore{0.0};
    std::atomic<bool> AnomalyBit{false};

    std::vector<CalculatedNumber> CNs;
    std::mutex CNsMutex;
};

class DetectableDimension : public PredictableDimension {
public:
    DetectableDimension(RRDDIM *RD) : PredictableDimension(RD) {}

    std::pair<bool, double> detect(size_t WindowLength, bool Reset) {
        bool AnomalyBit = isAnomalous();

        if (Reset)
            NumSetBits = RBC.numSetBits();

        NumSetBits += AnomalyBit;
        RBC.insert(AnomalyBit);

        double AnomalyRate = static_cast<double>(NumSetBits) / WindowLength;
        return { AnomalyBit, AnomalyRate };
    }

private:
    RollingBitCounter RBC{static_cast<size_t>(Cfg.ADWindowSize)};
    size_t NumSetBits{0};
};

using Dimension = DetectableDimension;

} // namespace ml

#endif /* ML_DIMENSION_H */
