// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_UNIT_H
#define ML_UNIT_H

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

class Host;

template <class BaseT>
class DetectableDimension {
public:
    bool detect() {
        BaseT &Dim = static_cast<BaseT &>(*this);
        bool AnomalyBit = Dim.predict().second;

        BitCounter += AnomalyBit;
        RBC.insert(AnomalyBit);

        return AnomalyBit;
    }

    void reset() {
        BitCounter = RBC.numSetBits();
    }

    double anomalyRate(size_t WindowLength) {
        double Rate = static_cast<double>(BitCounter) / WindowLength;
        BitCounter = RBC.numSetBits();
        return Rate;
    }

private:
    RollingBitCounter RBC{static_cast<size_t>(Cfg.DiffN)};
    size_t BitCounter{0};
};

template <class BaseT>
class TrainableDimension : public DetectableDimension<BaseT> {
public:
    RRDDIM *getRD() const {
        BaseT& Derived = static_cast<BaseT&>(*this);
        return Derived.getRD();
    }

    MLError train(TimePoint &Now);
    std::pair<MLError, bool> predict();

    bool getAnomalyBit() { return AnomalyBit; }

private:
    std::pair<CalculatedNumber *, size_t>
    getCalculatedNumbers(size_t MinN, size_t MaxN);

private:
    KMeans KM;

    // TODO: Add a couple seconds because the 1st getCalculatedNumbers will fail.
    TimePoint LastTrainedAt{SteadyClock::now()};
    bool HasModel{false};

    CalculatedNumber AnomalyScore{0.0};
    std::atomic<bool> AnomalyBit{false};

    std::mutex Mutex;
};

class Dimension : public TrainableDimension<Dimension> {
public:
    Dimension(RRDDIM *RD) : RD(RD), Ops(&RD->state->query_ops) {}

    RRDDIM *getRD() const { return RD; }

    const char *getID() const { return RD->id; }
    const char *getName() const { return RD->name; }
    Seconds updateEvery() const { return Seconds{RD->update_every}; }

    time_t latestTime() { return Ops->latest_time(RD); }
    time_t oldestTime() { return Ops->oldest_time(RD); }

private:
    RRDDIM *RD;
    struct rrddim_volatile::rrddim_query_ops *Ops;

    friend class Host;
};

using Unit = Dimension;

} // namespace ml

#endif /* ML_UNIT_H */
