// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Unit.h"
#include "RollingBitCounter.h"

#include "json.hpp"

using namespace ml;
using namespace nlohmann;

static void updateMLChart(collected_number NumTotalDimensions,
                          collected_number NumAnomalousDimensions,
                          collected_number AnomalyRate) {
    static thread_local RRDSET *RS = nullptr;
    static thread_local RRDDIM *NumTotalDimensionsRD = nullptr;
    static thread_local RRDDIM *NumAnomalousDimensionsRD = nullptr;
    static thread_local RRDDIM *AnomalyRateRD = nullptr;

    if (!RS) {
        RS = rrdset_create_localhost(
            "ml",
            "host_anomaly_status",
            NULL,
            "ml",
            NULL,
            "Number of anomalous units",
            "number of units",
            "ml_units",
            NULL,
            39183,
            1,
            RRDSET_TYPE_LINE);

        NumTotalDimensionsRD = rrddim_add(RS, "num_total_dimensions", NULL,
                                     1, 1, RRD_ALGORITHM_ABSOLUTE);
        NumAnomalousDimensionsRD = rrddim_add(RS, "num_anomalous_dimensions", NULL,
                                              1, 1, RRD_ALGORITHM_ABSOLUTE);
        AnomalyRateRD = rrddim_add(RS, "anomaly_rate", NULL,
                                   1, 1, RRD_ALGORITHM_ABSOLUTE);
    } else 
        rrdset_next(RS);

    rrddim_set_by_pointer(RS, NumTotalDimensionsRD, NumTotalDimensions);
    rrddim_set_by_pointer(RS, NumAnomalousDimensionsRD, NumAnomalousDimensions);
    rrddim_set_by_pointer(RS, AnomalyRateRD, AnomalyRate);

    rrdset_done(RS);
}

void Host::addDimension(Dimension *D) {
    {
        std::lock_guard<std::mutex> Lock(Mutex);
        DimensionsMap[D->getRD()] = D;
    }

    NumDimensions++;
}

void Host::removeDimension(Dimension *D) {
    {
        std::lock_guard<std::mutex> Lock(Mutex);
        DimensionsMap.erase(D->getRD());
    }

    NumDimensions--;
}

void Host::forEachDimension(std::function<bool(Dimension *)> Func) {
    std::lock_guard<std::mutex> Lock(Mutex);
    for (auto &DP : DimensionsMap) {
        Dimension *Dim = DP.second;

        if (Func(Dim))
            break;
    }
}

template<>
void TrainableHost<Host>::trainOne(TimePoint &Now) {
    Host *H = static_cast<Host *>(this);

    H->forEachDimension([&](Dimension *D) {
        MLError Result = D->train(Now);

        switch (Result) {
        case MLError::TryLockFailed:
            return false;
        case MLError::ShouldNotTrainNow:
            return false;
        case MLError::MissingData:
            return false;
        case MLError::Success:
            return true;
        default:
            fatal("Unhandled MLError enumeration value");
        }
    });
}

template<>
void TrainableHost<Host>::train() {
    Host *H = static_cast<Host *>(this);

    while (!netdata_exit) {
        TimePoint StartTP = SteadyClock::now();
        trainOne(StartTP);
        TimePoint EndTP = SteadyClock::now();

        Duration<double> RealDuration = EndTP - StartTP;
        Duration<double> AllottedDuration = Duration<double>{Cfg.TrainEvery} / (H->getNumDimensions() + 1);

        if (RealDuration >= AllottedDuration)
            continue;

        std::this_thread::sleep_for(AllottedDuration - RealDuration);
    }
}

template<>
void DetectableHost<Host>::detectOnce() {
    Host *H = static_cast<Host *>(this);

    auto P = RBW.insert(AnomalyRate >= Cfg.AnomalyRateThreshold);
    RollingBitWindow::Edge E = P.first;
    size_t WindowLength = P.second;

    bool ResetBitCounter = (E.first == RollingBitWindow::State::BelowThreshold) &&
                           (E.second == RollingBitWindow::State::BelowThreshold);

    size_t NumAnomalousDimensions = 0;
    size_t NumTotalDimensions = H->getNumDimensions();

    H->forEachDimension([&](Dimension *D) {
        if (ResetBitCounter)
            D->reset();

        NumAnomalousDimensions += D->detect();
        return false;
    });

    error_log_limit_unlimited();

    AnomalyRate = 0;
    if (NumAnomalousDimensions != 0)
        AnomalyRate = static_cast<CalculatedNumber>(NumAnomalousDimensions) / NumTotalDimensions;
    updateMLChart(NumTotalDimensions, NumAnomalousDimensions, AnomalyRate * 100.0);
    error("anomaly rate: %lf", AnomalyRate);

    bool NewAnomalyEvent = (E.first == RollingBitWindow::State::AboveThreshold) &&
                           (E.second == RollingBitWindow::State::BelowThreshold);

    if (!NewAnomalyEvent)
        return;

    error("new anomaly length: %zu", WindowLength);

    std::vector<std::pair<double, std::string>> AnomalousUnits;
    H->forEachDimension([&](Dimension *D) {
        double DimAnomalyRate = D->anomalyRate(WindowLength);
        if (DimAnomalyRate < Cfg.ADUnitRateThreshold)
            return false;

        AnomalousUnits.push_back({DimAnomalyRate, D->getID()});

        return false;
    });

    if (AnomalousUnits.size() == 0) {
        error("Found anomaly event without any dimensions");
        return;
    }

    std::sort(AnomalousUnits.begin(), AnomalousUnits.end());
    std::reverse(AnomalousUnits.begin(), AnomalousUnits.end());

    nlohmann::json J = AnomalousUnits;
    time_t Now = now_realtime_sec();
    DB.insertAnomaly("AD1", 1, H->getUUID(), Now - WindowLength, Now, J.dump(4));

    error("num anomalous units: %zu", AnomalousUnits.size());
}

template<>
void DetectableHost<Host>::detect() {
    std::this_thread::sleep_for(Seconds{10});

    while (!netdata_exit) {
        detectOnce();
        std::this_thread::sleep_for(Seconds{1});
    }
}

template<>
void DetectableHost<Host>::startAnomalyDetectionThreads() {
    Host *H = static_cast<Host *>(this);
    TrainingThread = std::thread(&Host::train, H);
    DetectionThread = std::thread(&Host::detect, H);
}

template<>
void DetectableHost<Host>::stopAnomalyDetectionThreads() {
    TrainingThread.join();
    DetectionThread.join();
}
