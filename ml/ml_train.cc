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

void trainMain(struct netdata_static_thread *Thread) {
    netdata_thread_cleanup_push(cleanupTrainThread, Thread);

    std::this_thread::sleep_for(Cfg.UpdateEvery);

    size_t LoopCounter = 0;

    while (!netdata_exit) {
        info("Starting training loop %zu", LoopCounter++);
        SPDR_COUNTER1(Cfg.SPDR, "cat", "training-loop", SPDR_INT("iteration", LoopCounter));

        /*
         * Update hosts.
         */
        SPDR_BEGIN(Cfg.SPDR, "cat", "update-hosts");
        Cfg.updateHosts();
        SPDR_END(Cfg.SPDR, "cat", "update-hosts");

        /*
         * Update charts.
         */
        SPDR_BEGIN(Cfg.SPDR, "cat", "update-charts");
        TimePoint Now = SteadyClock::now();
        for (auto &HP : Cfg.Hosts) {
            Host *H = HP.second;

            if (Duration<Seconds>(Now - H->CreationTime) > Cfg.UpdateEvery)
                H->updateCharts();
        }
        SPDR_END(Cfg.SPDR, "cat", "update-charts");

        SPDR_BEGIN(Cfg.SPDR, "cat", "sleep");
        std::this_thread::sleep_for(Cfg.UpdateEvery);
        SPDR_END(Cfg.SPDR, "cat", "sleep");
        info("---");
    }

    netdata_thread_cleanup_pop(1);
}

};
