// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Chart.h"
#include "Unit.h"

using namespace ml;

void Host::updateMLStats() {
    /* Units stats chart */
    static thread_local RRDSET *StatsRS = nullptr;
    static thread_local RRDDIM *NumUnitsRD, *NumTrainedUnitsRD, *NumPredictedUnitsRD, *NumAnomaliesRD;

    if (!StatsRS) {
        std::string SetId = uid() + "_" + "units";

        StatsRS = rrdset_create_localhost(
                "ml",
                SetId.c_str(),
                NULL,
                "ml",
                NULL,
                "Number of trained/predicted/anomalous units",
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
        NumAnomaliesRD = rrddim_add(StatsRS, "anomalous units",
                                    NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
    } else {
        rrdset_next(StatsRS);
    }

    rrddim_set_by_pointer(StatsRS, NumUnitsRD, NumUnits);
    rrddim_set_by_pointer(StatsRS, NumTrainedUnitsRD, NumTrainedUnits);
    rrddim_set_by_pointer(StatsRS, NumPredictedUnitsRD, NumPredictedUnits);
    rrddim_set_by_pointer(StatsRS, NumAnomaliesRD, NumAnomalies);
    rrdset_done(StatsRS);

    /* Prediction time chart */

    static thread_local RRDSET *PredictionTimeRS = nullptr;
    static thread_local RRDDIM *PredictionTimeRD;

    if (!PredictionTimeRS) {
        std::string SetId = uid() + "_" + "ptime";

        PredictionTimeRS = rrdset_create_localhost(
                "ml",
                SetId.c_str(),
                NULL,
                "ml",
                NULL,
                "Time it took to predict units",
                "Milliseconds",
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

    /* Family chart */
    static thread_local RRDSET *FamilyRS = nullptr;

    typedef std::pair<RRDDIM *, unsigned> DimCountPair;
    typedef std::map<std::string, DimCountPair> FamilyCountMap;
    static thread_local FamilyCountMap FCM;

    if (!FamilyRS) {
        std::string SetId = uid() + "_" + "famas";

        FamilyRS = rrdset_create_localhost(
                "ml",
                SetId.c_str(),
                NULL,
                "ml",
                NULL,
                "Anomalous units by family",
                "Number of anomalous units",
                "famas",
                NULL,
                39185,
                1,
                RRDSET_TYPE_STACKED);
    } else {
        rrdset_next(FamilyRS);
    }

    // Reset each family count.
    for (auto &FCP : FCM)
        FCM[FCP.first].second = 0;

    // Bump family count for each dim.
    for (auto &CP : ChartsMap) {
        Chart *C = CP.second;

        for (auto &UP : C->UnitsMap) {
            Unit *U = UP.second;

            std::string Family = U->getFamily();

            FamilyCountMap::iterator It = FCM.find(Family);
            if (It == FCM.end())
                continue;

            if (U->isAnomalous(Cfg.AnomalyScoreThreshold))
                FCM[Family].second++;
        }
    }

    // Update dims.
    for (auto &FCP : FCM) {
        DimCountPair &DCP = FCP.second;
        rrddim_set_by_pointer(FamilyRS, DCP.first, DCP.second);
    }

    // End updating old units.
    rrdset_done(FamilyRS);

    // Add new units.
    for (auto &CP : ChartsMap) {
        Chart *C = CP.second;

        std::string Family = C->getFamily();

        FamilyCountMap::iterator It = FCM.find(Family);
        if (It != FCM.end())
            continue;

        RRDDIM *RD = rrddim_add(FamilyRS, Family.c_str(),
                NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
        FCM[Family] = std::make_pair(RD, 0);
    }
}

/*
 * Update the charts referenced by the host.
 */
void Host::updateCharts() {
    if (SteadyClock::now() - CreationTime < Cfg.UpdateEvery)
        return;

    std::unique_lock<std::mutex> Lock(Mutex);

    rrdhost_rdlock(RH);

    RRDSET *RS;
    rrdset_foreach_read(RS, RH) {
        rrdset_rdlock(RS);

        std::map<RRDSET *, Chart *>::iterator It = ChartsMap.find(RS);

        bool IsObsolete = rrdset_flag_check(RS, RRDSET_FLAG_ARCHIVED) ||
                          rrdset_flag_check(RS, RRDSET_FLAG_OBSOLETE);

        if (IsObsolete) {
            if (It != ChartsMap.end()) {
                // TODO: Remove obsolete charts.
                error("Found obsolete chart %s.%s", RS->rrdhost->hostname, RS->id);
                ChartsMap.erase(RS);
            }
        } else {
            if (It == ChartsMap.end()) {
                bool shouldSkip = false;

                // Skip if update every != 1 sec
                shouldSkip |= RS->update_every != 1;

                // Skip if this is a KMeans chart
                shouldSkip |= strstr(RS->id, "_km") != NULL;

                // Skip if this is an ML chart
                shouldSkip |= !strcmp(RS->family, "ml");

                // Skip if our users want
                shouldSkip |= simple_pattern_matches(Cfg.SP_ChartsToSkip, RS->name) != 0;

                if (!shouldSkip)
                    ChartsMap[RS] = new Chart(RS);
            }
        }

        rrdset_unlock(RS);
    }

    rrdhost_unlock(RH);

    for (auto &CP : ChartsMap) {
        Chart *C = CP.second;
        C->updateUnits(Cfg.TrainSecs, Cfg.TrainEvery, Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);
    }
}

std::vector<Unit *> Host::getUnits() {
    std::vector<Unit *> Units;

    for (auto &CP : ChartsMap) {
        Chart *C = CP.second;

        for (auto &UP : C->UnitsMap)
            Units.push_back(UP.second);
    }

    return Units;
}

void Host::train() {
    while (true) {
        updateCharts();

        std::vector<Unit *> Units = getUnits();

        if (Units.size() == 0) {
            std::this_thread::sleep_for(Cfg.UpdateEvery);
            continue;
        }

        Duration<double> AvgUnitTrainingDuration = Cfg.TrainEvery / Units.size();

        TimePoint UnitsTrainingStartTP = SteadyClock::now();
        for (Unit *U : Units) {
            // Train unit
            TimePoint STP = SteadyClock::now();
            if (!U->train())
                continue;
            TimePoint ETP = SteadyClock::now();

            // Break if we have to update charts again
            if (ETP - UnitsTrainingStartTP > Cfg.UpdateEvery)
                break;

            // Sleep if training this unit took less than the average time
            Duration<double> UnitTrainingDuration = ETP - STP;
            if (AvgUnitTrainingDuration > UnitTrainingDuration)
                std::this_thread::sleep_for(AvgUnitTrainingDuration - UnitTrainingDuration);
        }
        TimePoint UnitsTrainingEndTP = SteadyClock::now();

        // Sleep if we processed all the units too quickly
        Duration<double> TrainingDuration = UnitsTrainingEndTP - UnitsTrainingStartTP;
        if (TrainingDuration < Cfg.UpdateEvery)
            std::this_thread::sleep_for(Cfg.UpdateEvery - TrainingDuration);
    }
}

void Host::predict() {
    struct timeval StartTV, EndTV;

    std::this_thread::sleep_for(Cfg.UpdateEvery);

    while (true) {
        NumUnits = 0;
        NumTrainedUnits = 0;
        NumPredictedUnits = 0;
        NumAnomalies = 0;

        now_monotonic_high_precision_timeval(&StartTV);
        {
            std::unique_lock<std::mutex> Lock(Mutex);

            std::vector<Unit *> Units = getUnits();

            NumUnits = Units.size();

            for (Unit *U : Units) {
                U->predict();

                NumTrainedUnits += U->isTrained() ? 1 : 0;
                NumPredictedUnits += U->isPredicted() ? 1 : 0;
                NumAnomalies += U->isAnomalous(Cfg.AnomalyScoreThreshold);
            }

            for (auto &CP : ChartsMap) {
                Chart *C = CP.second;
                C->updateMLChart();
            }

            updateMLStats();
        }
        now_monotonic_high_precision_timeval(&EndTV);

        PredictionDuration = dt_usec(&EndTV, &StartTV);
        std::this_thread::sleep_for(Millis{1000});
    }
}
