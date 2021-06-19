// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Unit.h"
#include "RollingBitCounter.h"

#include "json.hpp"

using namespace ml;
using namespace nlohmann;

AnomalyStatusChart::AnomalyStatusChart(const std::string Name) {
    RS = rrdset_create_localhost(
        "ml",
        Name.c_str(),
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

    NumTotalUnitsRD = rrddim_add(RS, "num_total_units", NULL,
                                 1, 1, RRD_ALGORITHM_ABSOLUTE);
    NumAnomalousUnitsRD = rrddim_add(RS, "num_anomalous_units", NULL,
                                     1, 1, RRD_ALGORITHM_ABSOLUTE);
    AnomalyRateRD = rrddim_add(RS, "anomaly_rate", NULL,
                               1, 1, RRD_ALGORITHM_ABSOLUTE);
}

void AnomalyStatusChart::update(collected_number NumTotalUnits,
                                collected_number NumAnomalousUnits,
                                collected_number AnomalyRate)
{
    rrddim_set_by_pointer(RS, NumTotalUnitsRD, NumTotalUnits);
    rrddim_set_by_pointer(RS, NumAnomalousUnitsRD, NumAnomalousUnits);
    rrddim_set_by_pointer(RS, AnomalyRateRD, AnomalyRate);

    rrdset_done(RS);
    rrdset_next(RS);
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
void DetectableHost<Host>::detect() {
    Host *H = static_cast<Host *>(this);

    auto P = RBW.insert(AnomalyRate >= (3.0 / 5.0));
    RollingBitWindow::Edge E = P.first;

    bool ResetBitCounter = (E.first == RollingBitWindow::State::BelowThreshold) &&
                           (E.second == RollingBitWindow::State::BelowThreshold);

    size_t NumAnomalousDimensions = 0;
    size_t NumTotalDimensions = H->getNumDimensions();

    H->forEachDimension([&](Dimension *D) {
        if (ResetBitCounter)
            D->reset();

        NumAnomalousDimensions = D->detect();

        return false;
    });

    AnomalyRate = 0;
    if (NumAnomalousDimensions != 0)
        AnomalyRate = (100.0 * NumAnomalousDimensions) / NumTotalDimensions;

    bool NewAnomalyEvent = (E.first == RollingBitWindow::State::AboveThreshold) &&
                           (E.second == RollingBitWindow::State::BelowThreshold);

    if (!NewAnomalyEvent)
        return;

    size_t WindowLength = P.second;
    error("New anomaly length: %zu", WindowLength);

    std::vector<std::pair<double, std::string>> AnomalousUnits;
    AnomalousUnits.reserve(NumTotalDimensions);
    H->forEachDimension([&](Dimension *D) {
        AnomalousUnits.push_back({D->anomalyRate(WindowLength), D->getID()});
        return false;
    });

    error("Num anomalous units: %zu", AnomalousUnits.size());
}

template<>
void DetectableHost<Host>::startAnomalyDetectionThreads() {
    Host *H = static_cast<Host *>(this);
    TrainingThread = std::thread(&Host::train, H);
}

template<>
void DetectableHost<Host>::stopAnomalyDetectionThreads() {
    TrainingThread.join();
}
