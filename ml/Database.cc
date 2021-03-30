// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Chart.h"
#include "Database.h"

using namespace ml;

void Database::updateHosts() {
    SPDR_BEGIN(Cfg.SPDR, "cat", "update-hosts");
    rrd_rdlock();
    SPDR_BEGIN(Cfg.SPDR, "cat", "rrd-locked");

    RRDHOST *RH;
    rrdhost_foreach_read(RH) {
        rrdhost_rdlock(RH);
        SPDR_BEGIN(Cfg.SPDR, "cat", RH->hostname);

        std::map<RRDHOST *, Host *>::iterator It = Hosts.find(RH);

        if (rrdhost_flag_check(RH, RRDHOST_FLAG_ARCHIVED)) {
            // TODO: Remove obsolete hosts.
            fatal("Found archived host %s", RH->hostname);
        } else {
            if (It == Hosts.end()) {
                info("Creating new host %s", RH->hostname);
                Hosts[RH] = new Host(RH);
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
    for (auto &HP : DB.Hosts) {
        Host *H = HP.second;

        const auto D = Now - H->CreationTime;
        if (D > Cfg.UpdateEvery)
            H->updateCharts();
    }
    SPDR_END(Cfg.SPDR, "cat", "update-charts");
}

void Database::updateUnits() {
    SPDR_BEGIN(Cfg.SPDR, "cat", "update-units");
    for (auto &HP : DB.Hosts) {
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

void Database::update() {
    updateHosts();
    updateCharts();
    updateUnits();
}

std::vector<Unit *> Database::getUnits() {
    std::vector<Unit *> Units;

    SPDR_BEGIN(Cfg.SPDR, "cat", "collect-units");
    for (auto &HP : Hosts) {
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

    info("Found %zu units in %zu hosts", Units.size(), DB.Hosts.size());

    return Units;
}
