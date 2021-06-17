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

void AnomalyStatusChart::update(collected_number NumTotalUnits, collected_number NumAnomalousUnits) {
    rrddim_set_by_pointer(RS, NumTotalUnitsRD, NumTotalUnits);
    rrddim_set_by_pointer(RS, NumAnomalousUnitsRD, NumAnomalousUnits);

    CalculatedNumber AnomalyRate = 0;
    if (NumAnomalousUnits != 0)
        AnomalyRate = (100.0 * NumAnomalousUnits) / NumTotalUnits;
    rrddim_set_by_pointer(RS, AnomalyRateRD, AnomalyRate);

    rrdset_done(RS);
    rrdset_next(RS);
}

void Host::addUnit(Unit *U) {
    std::lock_guard<std::mutex> Lock(Mutex);
    UnitsMap[U->getRD()] = U;
}

void Host::removeUnit(Unit *U) {
    std::lock_guard<std::mutex> Lock(Mutex);
    UnitsMap.erase(U->getRD());
}

void Host::trainUnits() {
    std::this_thread::sleep_for(Seconds{10});

    while (!netdata_exit) {
        Duration<double> AvailableUnitTrainingDuration;

        TimePoint TrainingStartTP = SteadyClock::now();
        {
            std::lock_guard<std::mutex> Lock(Mutex);

            for (auto &UP : UnitsMap) {
                Unit *U = UP.second;

                if (U->train(TrainingStartTP))
                    break;
            }

            AvailableUnitTrainingDuration = Cfg.TrainEvery / (UnitsMap.size() + 1);
        }
        Duration<double> UnitTrainingDuration = SteadyClock::now() - TrainingStartTP;

        if (AvailableUnitTrainingDuration > UnitTrainingDuration)
            std::this_thread::sleep_for(AvailableUnitTrainingDuration - UnitTrainingDuration);
        else
            fatal("AvailableUnitTrainingDuration < UnitTrainingDuration");
    }
}

void Host::detectAnomalies() {
    std::this_thread::sleep_for(Seconds{10});

    AnomalyStatusChart ASC{"host_anomaly_status"};

    RollingBitWindow RBW{5, 3};
    Database DB{Cfg.AnomalyDBPath};

    std::vector<std::pair<double, std::string>> AnomalousUnits;

    while (!netdata_exit) {
        std::this_thread::sleep_for(Seconds{1});

        collected_number NumTotalUnits = 0;
        collected_number NumAnomalousUnits = 0;

        {
            std::lock_guard<std::mutex> Lock(Mutex);

            NumTotalUnits = UnitsMap.size();

            for (auto &UP : UnitsMap) {
                Unit *U = UP.second;

                if (U->isAnomalous())
                    NumAnomalousUnits++;
            }
        }

        ASC.update(NumTotalUnits, NumAnomalousUnits);

#if 0
        auto P = RBW.insert(NumAnomalousUnits > 4);

        RollingBitWindow::Edge E = P.first;
        if (E.first == RollingBitWindow::State::BelowThreshold &&
            E.second == RollingBitWindow::State::BelowThreshold) {
            {
                std::lock_guard<std::mutex> Lock(Mutex);

                for (auto &UP : UnitsMap) {
                    Unit *U = UP.second;
                    U->BitCounter = U->RBC.numSetBits();
                }
            }
        }

        if (E.first != RollingBitWindow::State::AboveThreshold ||
            E.second != RollingBitWindow::State::BelowThreshold)
            continue;

        size_t WindowLength = P.second;
        error("New anomaly length: %zu", WindowLength);

        {
            std::lock_guard<std::mutex> Lock(Mutex);

            for (auto &UP : UnitsMap) {
                Unit *U = UP.second;
                AnomalousUnits.push_back({U->anomalyRate(WindowLength), U->RD->id});
            }
        }

        if (AnomalousUnits.size() == 0)
            continue;

        nlohmann::json J = AnomalousUnits;
        time_t Now = now_realtime_sec();
        DB.insertAnomaly("AD1", 1, RH->host_uuid, Now - WindowLength, Now, J.dump(4));

        WindowLength = 0;
        AnomalousUnits.clear();
#endif
    }
}

void Host::runMLThreads() {
    TrainingThread = std::thread(&Host::trainUnits, this);
    AnomalyDetectionThread = std::thread(&Host::detectAnomalies, this);
}

void Host::stopMLThreads() {
    TrainingThread.join();
    AnomalyDetectionThread.join();
}
