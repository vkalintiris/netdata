// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Database.h"
#include "Host.h"
#include "Chart.h"
#include "Unit.h"

using namespace ml;

void Database::updateHosts() {
    rrd_rdlock();

    RRDHOST *RH;
    rrdhost_foreach_read(RH) {
        rrdhost_rdlock(RH);

        std::map<RRDHOST *, Host *>::iterator It = HostsMap.find(RH);

        if (rrdhost_flag_check(RH, RRDHOST_FLAG_ARCHIVED)) {
            // TODO: Remove obsolete hosts.
            fatal("Found archived host %s", RH->hostname);
        } else {
            if (It == HostsMap.end())
                HostsMap[RH] = new Host(RH);
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

    std::make_heap(Units.begin(), Units.end(), UnitComp());

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
    {
        std::unique_lock<std::mutex> Lock(Mutex);

        for (Unit *U : getUnits()) {
            U->predict();
        }
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
}
