// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Unit.h"
#include "RollingBitCounter.h"

#include "json.hpp"

using namespace ml;
using namespace nlohmann;

void Host::addUnit(Unit *U) {
    std::lock_guard<std::mutex> Lock(Mutex);
    UnitsMap[U->RD] = U;
}

void Host::removeUnit(Unit *U) {
    std::lock_guard<std::mutex> Lock(Mutex);
    UnitsMap.erase(U->RD);
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

void Host::trackAnomalyStatus() {
    std::this_thread::sleep_for(Seconds{10});

    RRDSET *HostAnomalyRS = nullptr;
    std::string SetId = "host_anomaly_status";

    HostAnomalyRS = rrdset_create_localhost(
        "ml",
        SetId.c_str(),
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

    RRDDIM *NumTotalUnitsRD = rrddim_add(HostAnomalyRS, "num_total_units",
                                         NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
    RRDDIM *NumAnomalousUnitsRD = rrddim_add(HostAnomalyRS, "num_anomalous_units",
                                             NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
    AnomalyRateRD = rrddim_add(HostAnomalyRS, "anomaly_rate",
                               NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);

    std::vector<std::pair<double, std::string>> AnomalousUnits;
    size_t WindowLength = 0;

    auto Callback = [this, &AnomalousUnits, &WindowLength](size_t Length) {
        WindowLength = Length;
        error("New anomaly length: %zu", Length);

        for (auto &UP : UnitsMap) {
            Unit *U = UP.second;
            AnomalousUnits.push_back({U->anomalyRate(Length), U->RD->id});
        }

        return false;
    };
    RollingBitWindow RBW{5, 3, Callback};

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

            RBW.insert(NumAnomalousUnits > 4);
        }

        CalculatedNumber AnomalyRate = 0;
        if (NumAnomalousUnits != 0)
            AnomalyRate = (100.0 * NumAnomalousUnits) / NumTotalUnits;

        rrddim_set_by_pointer(HostAnomalyRS, NumTotalUnitsRD, NumTotalUnits);
        rrddim_set_by_pointer(HostAnomalyRS, NumAnomalousUnitsRD, NumAnomalousUnits);
        rrddim_set_by_pointer(HostAnomalyRS, AnomalyRateRD, AnomalyRate);
        rrdset_done(HostAnomalyRS);
        rrdset_next(HostAnomalyRS);

        if (AnomalousUnits.size() == 0)
            continue;

        json J = AnomalousUnits;
        time_t Now = now_realtime_sec();
        DB.insertIntoAnomalyEvents("AD1", 1, RH->host_uuid, Now - WindowLength, Now, J);

        WindowLength = 0;
        AnomalousUnits.clear();
    }
}

void Host::runMLThreads() {
    TrainingThread = std::thread(&Host::trainUnits, this);
    TrackAnomalyStatusThread = std::thread(&Host::trackAnomalyStatus, this);
}

void Host::stopMLThreads() {
    TrainingThread.join();
    TrackAnomalyStatusThread.join();
}
