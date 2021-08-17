// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Dimension.h"
#include "RollingBitCounter.h"

#include "json.hpp"

using namespace ml;
using namespace nlohmann;

static void updateMLChart(RRDHOST *RH,
                          collected_number NumTotalDimensions,
                          collected_number NumAnomalousDimensions,
                          collected_number AnomalyRate) {
    static thread_local RRDSET *MLRS = nullptr;
    static thread_local RRDDIM *NumTotalDimensionsRD = nullptr;
    static thread_local RRDDIM *NumAnomalousDimensionsRD = nullptr;
    static thread_local RRDDIM *AnomalyRateRD = nullptr;

    if (!MLRS) {
        MLRS = rrdset_create(
            RH,
            "ml_prediction_info",
            "host_anomaly_status",
            NULL,
            "ml_prediction_info",
            NULL,
            "Number of anomalous units",
            "number of units",
            "ml_units",
            NULL,
            39183,
            1,
            RRDSET_TYPE_LINE
        );

        NumTotalDimensionsRD = rrddim_add(MLRS, "num_total_dimensions", NULL,
                1, 1, RRD_ALGORITHM_ABSOLUTE);
        NumAnomalousDimensionsRD = rrddim_add(MLRS, "num_anomalous_dimensions", NULL,
                1, 1, RRD_ALGORITHM_ABSOLUTE);
        AnomalyRateRD = rrddim_add(MLRS, "anomaly_rate", NULL,
                1, 1, RRD_ALGORITHM_ABSOLUTE);
    } else
        rrdset_next(MLRS);

    rrddim_set_by_pointer(MLRS, NumTotalDimensionsRD, NumTotalDimensions);
    rrddim_set_by_pointer(MLRS, NumAnomalousDimensionsRD, NumAnomalousDimensions);
    rrddim_set_by_pointer(MLRS, AnomalyRateRD, AnomalyRate);

    rrdset_done(MLRS);
}

static void updateADChart(RRDHOST *RH,
                          std::pair<RollingBitWindow::Edge, size_t> P,
                          bool ResetBitCounter,
                          bool NewAnomalyEvent,
                          collected_number AnomalyRate) {
    static thread_local RRDSET *ADRS = nullptr;
    static thread_local RRDDIM *WindowLengthRD = nullptr;
    static thread_local RRDDIM *AboveThresholdRD = nullptr;
    static thread_local RRDDIM *ResetBitCounterRD = nullptr;
    static thread_local RRDDIM *NewAnomalyEventRD = nullptr;
    static thread_local RRDDIM *AnomalyRateRD = nullptr;

    if (!ADRS) {
        ADRS = rrdset_create(
            RH,
            "ml_detector_info",
            "host_anomaly_status",
            NULL,
            "ml_detector_info",
            NULL,
            "Anomaly detection info",
            "info",
            "info",
            NULL,
            39184,
            1,
            RRDSET_TYPE_LINE
        );

        WindowLengthRD = rrddim_add(ADRS, "window_length", NULL,
                1, 1, RRD_ALGORITHM_ABSOLUTE);
        AnomalyRateRD = rrddim_add(ADRS, "anomaly_rate", NULL,
                1, 1, RRD_ALGORITHM_ABSOLUTE);
        AboveThresholdRD = rrddim_add(ADRS, "above_threshold", NULL,
                1, 1, RRD_ALGORITHM_ABSOLUTE);
        ResetBitCounterRD = rrddim_add(ADRS, "reset_bit_counter", NULL,
                1, 1, RRD_ALGORITHM_ABSOLUTE);
        NewAnomalyEventRD = rrddim_add(ADRS, "new_anomaly_event", NULL,
                1, 1, RRD_ALGORITHM_ABSOLUTE);
    } else
        rrdset_next(ADRS);

    RollingBitWindow::Edge E = P.first;
    bool AboveThreshold = E.second == RollingBitWindow::State::AboveThreshold;
    size_t WindowLength = P.second;

    rrddim_set_by_pointer(ADRS, WindowLengthRD, WindowLength);
    rrddim_set_by_pointer(ADRS, AnomalyRateRD, AnomalyRate);
    rrddim_set_by_pointer(ADRS, AboveThresholdRD, AboveThreshold);
    rrddim_set_by_pointer(ADRS, ResetBitCounterRD, ResetBitCounter);
    rrddim_set_by_pointer(ADRS, NewAnomalyEventRD, NewAnomalyEvent);

    rrdset_done(ADRS);
}

void RrdHost::addDimension(Dimension *D) {
    std::lock_guard<std::mutex> Lock(Mutex);
    DimensionsMap[D->getRD()] = D;
}

void RrdHost::removeDimension(Dimension *D) {
    std::lock_guard<std::mutex> Lock(Mutex);
    DimensionsMap.erase(D->getRD());
}

void TrainableHost::trainOne(TimePoint &Now) {
    for (auto &DP : DimensionsMap) {
        Dimension *D = DP.second;
        MLError Result = D->trainModel(Now);

        switch (Result) {
        case MLError::TryLockFailed:
            continue;
        case MLError::ShouldNotTrainNow:
            continue;
        case MLError::MissingData:
            continue;
        case MLError::Success:
            return;
        default:
            fatal("Unhandled MLError enumeration value");
        }
    }
}

void TrainableHost::train() {
    Duration<double> MaxSleepFor = Seconds{1};

    while (!netdata_exit) {
        size_t NumDimensions;
        Duration<double> RealDuration;

        {
            std::lock_guard<std::mutex> Lock(Mutex);

            TimePoint StartTP = SteadyClock::now();
            trainOne(StartTP);
            TimePoint EndTP = SteadyClock::now();

            NumDimensions = DimensionsMap.size();
            RealDuration = EndTP - StartTP;
        }

        Duration<double> AllottedDuration = Duration<double>{Cfg.TrainEvery} / (NumDimensions + 1);
        if (RealDuration >= AllottedDuration)
            continue;

        Duration<double> SleepFor = AllottedDuration - RealDuration;
        std::this_thread::sleep_for(std::min(SleepFor, MaxSleepFor));
    }
}

void DetectableHost::detectOnce() {
    auto P = RBW.insert(AnomalyRate >= Cfg.HostAnomalyRateThreshold);
    RollingBitWindow::Edge E = P.first;
    size_t WindowLength = P.second;

    bool ResetBitCounter = (E.first == RollingBitWindow::State::BelowThreshold) &&
                           (E.second == RollingBitWindow::State::BelowThreshold);
    bool NewAnomalyEvent = (E.first == RollingBitWindow::State::AboveThreshold) &&
                           (E.second == RollingBitWindow::State::BelowThreshold);

    std::vector<std::pair<double, std::string>> AnomalousDimensions;

    {
        std::lock_guard<std::mutex> Lock(Mutex);

        AnomalousDimensions.reserve(DimensionsMap.size());

        for (auto &DP : DimensionsMap) {
            Dimension *D = DP.second;

            auto P = D->detect(WindowLength, ResetBitCounter);
            bool IsAnomalous = P.first;
            double AnomalyRate = P.second;

            if (IsAnomalous)
                AnomalousDimensions.push_back({ AnomalyRate, D->getID() });
        }

        AnomalyRate = 0;
        if (AnomalousDimensions.size() != 0)
            AnomalyRate = static_cast<double>(AnomalousDimensions.size()) / DimensionsMap.size();

        error("Host anomaly: "
              "rate=%lf, length=%zu,"
              "anomalous-dimensions=%zu, total-dimensions= %zu",
              AnomalyRate, WindowLength,
              AnomalousDimensions.size(), DimensionsMap.size());

        updateMLChart(getRH(), DimensionsMap.size(), AnomalousDimensions.size(), 100 * AnomalyRate);
        updateADChart(getRH(), P, ResetBitCounter, NewAnomalyEvent, 100 * AnomalyRate);
    }

    if (!NewAnomalyEvent)
        return;

    std::sort(AnomalousDimensions.begin(), AnomalousDimensions.end());
    std::reverse(AnomalousDimensions.begin(), AnomalousDimensions.end());

    nlohmann::json J = AnomalousDimensions;
    time_t Now = now_realtime_sec();
    DB.insertAnomaly("AD1", 1, getUUID(), Now - WindowLength, Now, J.dump(4));
}

void DetectableHost::detect() {
    std::this_thread::sleep_for(Seconds{10});

    while (!netdata_exit) {
        TimePoint StartTP = SteadyClock::now();
        detectOnce();
        TimePoint EndTP = SteadyClock::now();
        Duration<double> Dur1 = EndTP - StartTP;

        error("Detection took %lf seconds", Dur1.count());

        std::this_thread::sleep_for(Seconds{1});
    }
}

void DetectableHost::startAnomalyDetectionThreads() {
    TrainableHost *TH = dynamic_cast<TrainableHost *>(this);
    TrainingThread = std::thread(&TrainableHost::train, TH);

    DetectableHost *DH = dynamic_cast<DetectableHost *>(this);
    DetectionThread = std::thread(&DetectableHost::detect, DH);
}

void DetectableHost::stopAnomalyDetectionThreads() {
    TrainingThread.join();
    DetectionThread.join();
}
