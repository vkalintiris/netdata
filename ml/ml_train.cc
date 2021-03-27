// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

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

    return Units;
}

void trainMain(struct netdata_static_thread *Thread) {
    netdata_thread_cleanup_push(cleanupTrainThread, Thread);

    sleep_usec(Cfg.UpdateEvery);

    size_t LoopCounter = 0;

    while (!netdata_exit) {
        info("\nStarting training loop %zu", LoopCounter++);
        SPDR_COUNTER1(Cfg.SPDR, "cat", "training-loop", SPDR_INT("iteration", LoopCounter));

        /*
         * Update hosts, charts & units.
         */

        Cfg.updateHosts();

        /*
         * Collect units
        */
        std::vector<Unit *> Units = collectUnits(Cfg.Hosts);

        SPDR_COUNTER1(Cfg.SPDR, "cat", "num-units", SPDR_INT("count", Units.size()));

        SPDR_BEGIN(Cfg.SPDR, "cat", "sleep");
        sleep_usec(Cfg.UpdateEvery);
        SPDR_END(Cfg.SPDR, "cat", "sleep");
    }

    netdata_thread_cleanup_pop(1);
}

};
