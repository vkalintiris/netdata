// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Unit.h"
#include "json.hpp"

using namespace ml;
using Json = nlohmann::json;

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

using AnomalyEvent = std::pair<int, int>;

static std::vector<AnomalyEvent>
getAnomalyEvents(std::vector<bool> AnomalyStatus, unsigned MinSize, double MinRate) {
    std::vector<AnomalyEvent> V;

    if (AnomalyStatus.size() < MinSize)
        return V;

    int WindowStart = 0;
    int WindowEnd = MinSize - 1;

    double Counter = 0;
    for (unsigned Idx = 0; Idx != MinSize; Idx++)
        Counter += AnomalyStatus[Idx];

    double Rate = Counter / MinSize;
    if (Rate >= MinRate)
        V.push_back(std::make_pair(WindowStart, WindowEnd));

    for (unsigned Idx = MinSize; Idx != AnomalyStatus.size(); Idx++) {
        WindowStart++;
        WindowEnd++;

        Counter += AnomalyStatus[Idx] - AnomalyStatus[Idx - MinSize];
        Rate = Counter / MinSize;

        if (Rate >= MinRate)
            V.push_back(std::make_pair(WindowStart, WindowEnd));
    }

    if (V.size() == 0)
        return V;

    int NumAnomalyEvents = 1;
    AnomalyEvent &AE = V[0];

    for (unsigned Idx = 1; Idx != V.size(); Idx++) {
        AnomalyEvent CurrAE = V[Idx];

        if (CurrAE.first <= AE.second) {
            AE.second = CurrAE.second;
        } else {
            V[NumAnomalyEvents] = AE;
            AE = CurrAE;
            NumAnomalyEvents += 1;
        }
    }

    V.resize(NumAnomalyEvents);
    return V;
}

std::string Host::findAnomalyEvents(time_t AfterT, time_t BeforeT) {
    struct rrddim_volatile::rrddim_query_ops *Ops = &AnomalyRateRD->state->query_ops;
    struct rrddim_query_handle Handle;

    (void) AfterT;
    (void) BeforeT;

    std::vector<bool> NodeAnomalyStatus(BeforeT - AfterT + 1, false);

    Ops->init(AnomalyRateRD, &Handle, AfterT, BeforeT);
    while (!Ops->is_finished(&Handle)) {
        time_t CurrT;

        storage_number SN = Ops->next_metric(&Handle, &CurrT);
        NodeAnomalyStatus.push_back(SN & SN_ANOMALOUS);
    }

    Json J;
    J["anomaly_events"] = getAnomalyEvents(NodeAnomalyStatus, 30, 0.01);
    return J.dump(4);
}
