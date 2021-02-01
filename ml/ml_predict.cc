// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

static void cleanupPredictThread(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up predict thread");
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

void ml::predictMain(struct netdata_static_thread *Thread) {
    netdata_thread_cleanup_push(cleanupPredictThread, Thread);

    heartbeat_t HB;
    heartbeat_init(&HB);

    Host H = Host(localhost, Cfg.ChartsMap);

    while (!netdata_exit) {
        unsigned NumPredicted = 0, NumUnits = 0;

        netdata_rwlock_rdlock(&Cfg.ChartsMapLock);

        for (auto &P : H.ChartsMap) {
            Chart *C = P.second;

            for (auto &P : C->UnitsMap) {
                Unit *U = P.second;

                NumUnits++;

                if (U->rdTryLock())
                    continue;

                if (U->predict())
                    NumPredicted++;

                U->unLock();
            }

            C->updateMLChart();
        }

        netdata_rwlock_unlock(&Cfg.ChartsMapLock);

        info("Predicted %u/%u units", NumPredicted, NumUnits);

        heartbeat_next(&HB, 1 * USEC_PER_SEC);
    }

    netdata_thread_cleanup_pop(1);
}
