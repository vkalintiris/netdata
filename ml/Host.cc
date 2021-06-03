// SPDX-License-Identifier: GPL-3.0-or-later

#include "AnomalyDetector.h"
#include "Config.h"
#include "Host.h"
#include "Unit.h"

#include "json.hpp"

using namespace ml;

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

        CalculatedNumber AnomalyRate = 0;
        if (NumAnomalousUnits != 0)
            AnomalyRate = (100.0 * NumAnomalousUnits) / NumTotalUnits;

        rrddim_set_by_pointer(HostAnomalyRS, NumTotalUnitsRD, NumTotalUnits);
        rrddim_set_by_pointer(HostAnomalyRS, NumAnomalousUnitsRD, NumAnomalousUnits);
        rrddim_set_by_pointer(HostAnomalyRS, AnomalyRateRD, AnomalyRate);
        rrdset_done(HostAnomalyRS);
        rrdset_next(HostAnomalyRS);
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

std::string Host::getAnomalyEventsJson(time_t AfterT, time_t BeforeT) {
    AnomalyDetector AD = AnomalyDetector(AfterT, BeforeT);

    std::vector<AnomalyEvent> AEV =
        AD.getAnomalyEvents(AnomalyRateRD, Cfg.ADWindowSize, Cfg.ADWindowRateThreshold);

    nlohmann::json JsonResponse;
    JsonResponse["anomaly_events"] = AEV;
    return JsonResponse.dump(4);
}

std::string Host::getAnomalyEventInfoJson(time_t AfterT, time_t BeforeT) {
    std::vector<AnomalyEventInfo> AEIV;
    AnomalyDetector AD = AnomalyDetector(AfterT, BeforeT);

    {
        std::lock_guard<std::mutex> Lock(Mutex);

        for (const auto &UP : UnitsMap) {
            AnomalyEventInfo AEI = AD.getAnomalyEventInfo(UP.first);

            if (AEI.AnomalyRate >= Cfg.ADUnitRateThreshold)
                AEIV.push_back(AEI);
        }
    }

#if 0
    // TODO: add config opt.
    if (AEIV.size() > 20)
        AEIV.resize(20);
#endif
    auto CmpL =  [](const AnomalyEventInfo &LHS, const AnomalyEventInfo &RHS) {
        return (LHS.AnomalyRate > RHS.AnomalyRate);
    };
    std::sort(AEIV.begin(), AEIV.end(), CmpL);

    nlohmann::json JsonResponse;
    for (const AnomalyEventInfo &AEI : AEIV) {
        nlohmann::json JsonEntry;

        JsonEntry[AEI.Name]["anomaly_rate"] = AEI.AnomalyRate;
        JsonEntry[AEI.Name]["anomaly_status"] = AEI.AnomalyStatus;
        JsonResponse["dimensions"].push_back(JsonEntry);
    }

    return JsonResponse.dump(4);
}
