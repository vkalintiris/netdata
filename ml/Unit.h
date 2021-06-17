// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_UNIT_H
#define ML_UNIT_H

#include "ml-private.h"

#include "Config.h"
#include "RollingBitCounter.h"

namespace ml {

class Host;

template <class BaseT>
class Detectable {
public:
    void detect() {
        BaseT &Derived = static_cast<BaseT&>(*this);

        bool isAnomalous = Derived.isAnomalous();
        BitCounter += isAnomalous;
        RBC.insert(isAnomalous);
        error("ID: %s, BitCounter: %zu, RBC: %zu", Derived.getName(), BitCounter, RBC.numSetBits());
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
class Trainable : public Detectable<BaseT> {
public:
    RRDDIM *getRD() const {
        BaseT& Derived = static_cast<BaseT&>(*this);
        return Derived.getRD();
    }

    bool train(TimePoint &Now);
    void predict();

    bool isAnomalous() {
        return AnomalyScore >= Cfg.AnomalyScoreThreshold;
    }

private:
    std::pair<CalculatedNumber *, size_t>
    getCalculatedNumbers(size_t MinN, size_t MaxN);

private:
    KMeans KM;
    CalculatedNumber AnomalyScore{0.0};

    bool HasModel{false};
    bool ShouldTrain{false};

    TimePoint LastTrainedAt;

    std::mutex Mutex;
};

class Dimension : public Trainable<Dimension> {
public:
    Dimension(RRDDIM *RD) : RD(RD), Ops(&RD->state->query_ops) {}

    RRDDIM *getRD() const { return RD; }

    const char *getID() const { return RD->id; }
    const char *getName() const { return RD->name; }
    size_t updateEvery() const { return static_cast<size_t>(RD->update_every); }

    time_t latestTime() { return Ops->latest_time(RD); }
    time_t oldestTime() { return Ops->oldest_time(RD); }

private:
    RRDDIM *RD;
    struct rrddim_volatile::rrddim_query_ops *Ops;

    friend class Host;
};

using Unit = Dimension;

}

#endif /* ML_UNIT_H */
