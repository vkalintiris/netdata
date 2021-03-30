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

void trainMain(struct netdata_static_thread *Thread) {
    netdata_thread_cleanup_push(cleanupTrainThread, Thread);

    size_t LoopCounter = 0;

    while (!netdata_exit) {
        info("Starting training loop %zu", ++LoopCounter);
        SPDR_COUNTER1(Cfg.SPDR, "cat", "training-loop", SPDR_INT("iteration", LoopCounter));

        // Update DB and collect units.
        DB.update();
        std::vector<Unit *> Units = DB.getUnits();

        // Heapify units.
        SPDR_BEGIN(Cfg.SPDR, "cat", "heapify-units");
        std::make_heap(Units.begin(), Units.end(), UnitComp());
        SPDR_END(Cfg.SPDR, "cat", "heapify-units");

        // Nothing to do if we don't have any units.
        if (Units.size() == 0) {
            SPDR_BEGIN(Cfg.SPDR, "cat", "train-sleep");
            std::this_thread::sleep_for(Cfg.UpdateEvery);
            SPDR_END(Cfg.SPDR, "cat", "train-sleep");
            continue;
        }

        /*
         * Train units.
        */

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

        info("---");
    }

    netdata_thread_cleanup_pop(1);
}

};
