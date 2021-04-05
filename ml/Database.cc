// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Database.h"
#include "Host.h"
#include "Chart.h"
#include "Unit.h"

using namespace ml;

static void updateMLStats(std::vector<Unit *> Units,
                          usec_t PredictionDuration) {
    /* Units stats chart */

    static RRDSET *StatsRS = nullptr;
    static RRDDIM *NumUnitsRD, *NumTrainedUnitsRD, *NumPredictedUnitsRD;

    collected_number NumUnits, NumTrainedUnits, NumPredictedUnits;

    if (unlikely(!StatsRS)) {
        StatsRS = rrdset_create_localhost(
                "ml",
                "units",
                NULL,
                "ml_units",
                NULL,
                "Number of units trained/predicted",
                "number of units",
                "ml_units",
                NULL,
                39183,
                1,
                RRDSET_TYPE_LINE);

        NumUnitsRD = rrddim_add(StatsRS, "total units",
                                NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
        NumTrainedUnitsRD = rrddim_add(StatsRS, "trained units",
                                       NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
        NumPredictedUnitsRD = rrddim_add(StatsRS, "predicted units",
                                         NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
    } else {
        rrdset_next(StatsRS);
    }

    NumUnits = Units.size();
    rrddim_set_by_pointer(StatsRS, NumUnitsRD, NumUnits);

    NumTrainedUnits = 0, NumPredictedUnits = 0;
    for (const Unit *U : Units) {
        NumTrainedUnits += U->isTrained() ? 1 : 0;
        NumPredictedUnits += U->isPredicted() ? 1 : 0;
    }
    rrddim_set_by_pointer(StatsRS, NumTrainedUnitsRD, NumTrainedUnits);
    rrddim_set_by_pointer(StatsRS, NumPredictedUnitsRD, NumPredictedUnits);

    rrdset_done(StatsRS);

    /* Prediction time chart */

    static RRDSET *PredictionTimeRS = nullptr;
    static RRDDIM *PredictionTimeRD;

    if (unlikely(!PredictionTimeRS)) {
        PredictionTimeRS = rrdset_create_localhost(
                "ml",
                "prediction_time",
                NULL,
                "prediction_time",
                NULL,
                "Time it took to predict units",
                "barfoo",
                "prediction_time",
                NULL,
                39184,
                1,
                RRDSET_TYPE_LINE);

        PredictionTimeRD = rrddim_add(PredictionTimeRS, "prediction tread iteration time",
                                      NULL, 1, USEC_PER_MS, RRD_ALGORITHM_ABSOLUTE);
    } else {
        rrdset_next(PredictionTimeRS);
    }

    rrddim_set_by_pointer(PredictionTimeRS, PredictionTimeRD, PredictionDuration);
    rrdset_done(PredictionTimeRS);
}

void Database::updateHosts() {
    rrd_rdlock();

    RRDHOST *RH;
    rrdhost_foreach_read(RH) {
        rrdhost_rdlock(RH);

        if (!simple_pattern_matches(Cfg.SP_HostsToSkip, RH->hostname)) {
            std::map<RRDHOST *, Host *>::iterator It = HostsMap.find(RH);

            if (rrdhost_flag_check(RH, RRDHOST_FLAG_ARCHIVED)) {
                // TODO: Remove obsolete hosts.
                fatal("Found archived host %s", RH->hostname);
            } else {
                if (It == HostsMap.end())
                    HostsMap[RH] = new Host(RH);
            }
        }

        rrdhost_unlock(RH);
    }

    rrd_unlock();
}

void Database::updateCharts() {
    const auto Now = SteadyClock::now();
    for (auto &HP : DB.HostsMap) {
        Host *H = HP.second;

        const auto D = Now - H->CreationTime;
        if (D > Cfg.UpdateEvery)
            H->updateCharts();
    }
}

void Database::updateUnits() {
    for (auto &HP : DB.HostsMap) {
        Host *H = HP.second;

        for (auto &CP : H->ChartsMap) {
            Chart *C = CP.second;

            C->updateUnits(Cfg.TrainSecs, Cfg.TrainEvery,
                           Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);
        }
    }
}

std::vector<Unit *> Database::getUnits() {
    std::vector<Unit *> Units;

    for (auto &HP : HostsMap) {
        Host *H = HP.second;

        for (auto &CP : H->ChartsMap) {
            Chart *C = CP.second;

            for (auto &UP : C->UnitsMap) {
                    Unit *U = UP.second;

                    Units.push_back(U);
            }
        }
    }

    info("Found %zu units in %zu hosts", Units.size(), DB.HostsMap.size());

    return Units;
}

void Database::trainUnits() {
    {
        std::unique_lock<std::mutex> Lock(Mutex);

        updateHosts();
        updateCharts();
        updateUnits();
    }

    std::vector<Unit *> Units = getUnits();

    if (Units.size() == 0) {
        std::this_thread::sleep_for(Cfg.UpdateEvery);
        return;
    }

    TimePoint StartTrainingTP = SteadyClock::now();
    Duration<double> AvailableUnitTrainingDuration = Cfg.TrainEvery / Units.size();

    for (Unit *U : Units) {
        TimePoint STP = SteadyClock::now();

        if (!U->train())
            continue;

        TimePoint ETP = SteadyClock::now();

        if (ETP - StartTrainingTP > Cfg.UpdateEvery)
            break;

        Duration<double> UnitTrainingDuration = ETP - STP;
        if (AvailableUnitTrainingDuration > UnitTrainingDuration)
            std::this_thread::sleep_for(AvailableUnitTrainingDuration - UnitTrainingDuration);
    }

    TimePoint EndTrainingTP = SteadyClock::now();
    Duration<double> TrainingDuration = EndTrainingTP - StartTrainingTP;
    if (TrainingDuration < Cfg.UpdateEvery)
        std::this_thread::sleep_for(Cfg.UpdateEvery - TrainingDuration);
}

void Database::predictUnits() {
    static usec_t PredictionDuration;
    struct timeval StartTV, EndTV;

    now_monotonic_high_precision_timeval(&StartTV);
    {
        std::unique_lock<std::mutex> Lock(Mutex);

        std::vector<Unit *> Units = getUnits();

        for (Unit *U : Units)
            U->predict();

        updateMLStats(Units, PredictionDuration);
    }

    {
        std::unique_lock<std::mutex> Lock(Mutex);

        for (auto &HP : HostsMap) {
            Host *H = HP.second;

            for (auto &CP: H->ChartsMap) {
                Chart *C = CP.second;
                C->updateMLChart();
            }
        }
    }
    now_monotonic_high_precision_timeval(&EndTV);

    PredictionDuration = dt_usec(&EndTV, &StartTV);

    double Count = static_cast<double>(dt_usec(&EndTV, &StartTV)) / USEC_PER_SEC;
    info("Prediction duration: %f", Count);
}
