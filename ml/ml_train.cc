// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Database.h"
#include "Host.h"
#include "Chart.h"
#include "Unit.h"

static void dumpSpdr(const char *string, void *user_data) {
    (void) user_data;

    ml::Cfg.LogFp << string << std::endl;
}


static void cleanupTrainThread(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up train thread");

    spdr_report(ml::Cfg.SPDR, SPDR_CHROME_REPORT, dumpSpdr, nullptr);
    ml::Cfg.LogFp.close();

    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

namespace ml {

static std::vector<Unit *> collectUnits(std::map<RRDHOST *, Host *> &Hosts) {
    std::vector<Unit *> Units;

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

    return Units;
}

void trainMain(struct netdata_static_thread *Thread) {
    netdata_thread_cleanup_push(cleanupTrainThread, Thread);

    Database *DB = Cfg.DB;

    std::this_thread::sleep_for(Cfg.UpdateEvery);

    size_t LoopCounter = 0;

    while (!netdata_exit) {
        info("Starting training loop %zu", ++LoopCounter);
        SPDR_COUNTER1(Cfg.SPDR, "cat", "training-loop", SPDR_INT("iteration", LoopCounter));

        /*
         * Update hosts.
         */
        SPDR_BEGIN(Cfg.SPDR, "cat", "update-hosts");
        DB->updateHosts();
        SPDR_END(Cfg.SPDR, "cat", "update-hosts");

        /*
         * Update charts.
         */
        SPDR_BEGIN(Cfg.SPDR, "cat", "update-charts");
        const auto Now = SteadyClock::now();
        for (auto &HP : DB->Hosts) {
            Host *H = HP.second;

            const auto D = Now - H->CreationTime;
            if (D > Cfg.UpdateEvery)
                H->updateCharts();
        }
        SPDR_END(Cfg.SPDR, "cat", "update-charts");

        /*
         * Update units.
         */
        SPDR_BEGIN(Cfg.SPDR, "cat", "update-units");
        for (auto &HP : DB->Hosts) {
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

        /*
         * Collect units.
         */
        SPDR_BEGIN(Cfg.SPDR, "cat", "collect-units");
        std::vector<Unit *> Units = collectUnits(DB->Hosts);
        SPDR_END(Cfg.SPDR, "cat", "collect-units");

        info("Found %zu units in %zu hosts", Units.size(), DB->Hosts.size());

        /*
         * Heapify units.
         */
        SPDR_BEGIN(Cfg.SPDR, "cat", "heapify-units");
        std::make_heap(Units.begin(), Units.end(), UnitComp());
        SPDR_END(Cfg.SPDR, "cat", "heapify-units");

        /*
         * Train units.
         */
        if (Units.size() == 0) {
            SPDR_BEGIN(Cfg.SPDR, "cat", "train-sleep");
            std::this_thread::sleep_for(Cfg.UpdateEvery);
            SPDR_END(Cfg.SPDR, "cat", "train-sleep");
            continue;
        }

        TimePoint StartTrainingTP = SteadyClock::now();
        Duration<double> AvailableUnitTrainingDuration = Cfg.TrainEvery / Units.size();

        SPDR_BEGIN(Cfg.SPDR, "cat", "train-units");
        for (Unit *U : Units) {
            if (U->uid().compare("system.cpu.user") != 0)
                continue;

            /*
             * Train unit
             */

            SPDR_BEGIN(Cfg.SPDR, "cat", U->c_spdr_id());
            TimePoint STP = SteadyClock::now();

            if (U->train())
                SPDR_EVENT1(Cfg.SPDR, "cat", "trained", SPDR_STR(U->c_spdr_id(), "true"));
            else
                SPDR_EVENT1(Cfg.SPDR, "cat", "trained", SPDR_STR(U->c_spdr_id(), "false"));

            TimePoint ETP = SteadyClock::now();
            SPDR_END(Cfg.SPDR, "cat", U->c_spdr_id());

            /*
             * Figure out how long we have to sleep.
             */
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

        info("---");
    }

    netdata_thread_cleanup_pop(1);
}

};
