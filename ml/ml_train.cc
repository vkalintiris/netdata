// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

static void cleanupTrainThread(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up train thread");
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

namespace ml {

void trainMain(struct netdata_static_thread *Thread) {
    netdata_thread_cleanup_push(cleanupTrainThread, Thread);

    heartbeat_t HB;
    heartbeat_init(&HB);

    while (!netdata_exit) {
        Cfg.updateHosts();

        for (auto &P : Cfg.Hosts) {
            Host *H = P.second;
            H->updateCharts();

            for (auto &P: H->ChartsMap) {
                Chart *C = P.second;

                for (auto &P : C->UnitsMap) {
                    Unit *U = P.second;
                    U->train();
                }
            }

        }

        heartbeat_next(&HB, 10 * USEC_PER_SEC);
    }

    netdata_thread_cleanup_pop(1);
}

};
