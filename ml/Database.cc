// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Database.h"
#include "Host.h"
#include "Chart.h"
#include "Unit.h"

using namespace ml;

void Database::updateHosts() {
    SPDR_BEGIN(Cfg.SPDR, "cat", "update-hosts");
    rrd_rdlock();
    SPDR_BEGIN(Cfg.SPDR, "cat", "rrd-locked");

    RRDHOST *RH;
    rrdhost_foreach_read(RH) {
        rrdhost_rdlock(RH);
        SPDR_BEGIN(Cfg.SPDR, "cat", RH->hostname);

        std::map<RRDHOST *, Host *>::iterator It = HostsMap.find(RH);

        if (rrdhost_flag_check(RH, RRDHOST_FLAG_ARCHIVED)) {
            // TODO: Remove obsolete hosts.
            fatal("Found archived host %s", RH->hostname);
        } else {
            if (It == HostsMap.end()) {
                info("Creating new host %s", RH->hostname);
                HostsMap[RH] = new Host(RH);
            }
        }

        SPDR_END(Cfg.SPDR, "cat", RH->hostname);
        rrdhost_unlock(RH);
    }

    SPDR_END(Cfg.SPDR, "cat", "rrd-locked");
    rrd_unlock();
    SPDR_END(Cfg.SPDR, "cat", "update-hosts");
}

void Database::updateCharts() {
    SPDR_BEGIN(Cfg.SPDR, "cat", "update-charts");
    const auto Now = SteadyClock::now();
    for (auto &HP : DB.HostsMap) {
        Host *H = HP.second;

        const auto D = Now - H->CreationTime;
        if (D > Cfg.UpdateEvery)
            H->updateCharts();
    }
    SPDR_END(Cfg.SPDR, "cat", "update-charts");
}

void Database::updateUnits() {
    SPDR_BEGIN(Cfg.SPDR, "cat", "update-units");
    for (auto &HP : DB.HostsMap) {
        Host *H = HP.second;

        SPDR_BEGIN(Cfg.SPDR, "cat", H->c_uid());
        for (auto &CP : H->ChartsMap) {
            Chart *C = CP.second;

            C->updateUnits(Cfg.TrainSecs, Cfg.TrainEvery,
                           Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);
        }
        SPDR_END(Cfg.SPDR, "cat", H->c_uid());
    }
    SPDR_END(Cfg.SPDR, "cat", "update-units");
}

std::vector<Unit *> Database::getUnits() {
    std::vector<Unit *> Units;

    SPDR_BEGIN(Cfg.SPDR, "cat", "collect-units");
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
    SPDR_END(Cfg.SPDR, "cat", "collect-units");

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

    SPDR_BEGIN(Cfg.SPDR, "cat", "heapify-units");
    std::make_heap(Units.begin(), Units.end(), UnitComp());
    SPDR_END(Cfg.SPDR, "cat", "heapify-units");

    if (Units.size() == 0) {
        SPDR_BEGIN(Cfg.SPDR, "cat", "train-sleep");
        std::this_thread::sleep_for(Cfg.UpdateEvery);
        SPDR_END(Cfg.SPDR, "cat", "train-sleep");
        return;
    }

    TimePoint StartTrainingTP = SteadyClock::now();
    Duration<double> AvailableUnitTrainingDuration = Cfg.TrainEvery / Units.size();

    SPDR_BEGIN(Cfg.SPDR, "cat", "train-units");
    for (Unit *U : Units) {
        if (U->uid().compare("system.cpu.user") != 0)
            continue;

        SPDR_BEGIN(Cfg.SPDR, "cat", U->c_spdr_id());
        TimePoint STP = SteadyClock::now();

        if (U->train())
            SPDR_EVENT1(Cfg.SPDR, "cat", "trained", SPDR_STR(U->c_spdr_id(), "true"));
        else
            SPDR_EVENT1(Cfg.SPDR, "cat", "trained", SPDR_STR(U->c_spdr_id(), "false"));

        TimePoint ETP = SteadyClock::now();
        SPDR_END(Cfg.SPDR, "cat", U->c_spdr_id());

        if (ETP - StartTrainingTP > Cfg.UpdateEvery)
            break;

        Duration<double> UnitTrainingDuration = ETP - STP;
        if (AvailableUnitTrainingDuration > UnitTrainingDuration) {
            SPDR_BEGIN(Cfg.SPDR, "cat", "train-sleep");
            std::this_thread::sleep_for(AvailableUnitTrainingDuration - UnitTrainingDuration);
            SPDR_END(Cfg.SPDR, "cat", "train-sleep");
        }
    }
    SPDR_END(Cfg.SPDR, "cat", "train-units");
}

void Database::predictUnits() {
    {
        std::unique_lock<std::mutex> Lock(Mutex);

        for (Unit *U : getUnits())
            U->predict();
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
