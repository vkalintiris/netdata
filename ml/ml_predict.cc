// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Database.h"

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

#if 0
    std::map<RRDHOST *, Host *> &Hosts = DB.Hosts;
#endif

    while (!netdata_exit) {
        heartbeat_next(&HB, 1 * USEC_PER_SEC);
        continue;

#if 0
        netdata_rwlock_rdlock(&Cfg.HostsLock);
        for (auto &P : Hosts) {
            unsigned NumPredicted = 0, NumUnits = 0;

            struct timeval BTV, ETV;
            now_monotonic_high_precision_timeval(&BTV);

            Host *H = P.second;
            H->rdLock();
            for (auto &P : H->ChartsMap) {
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
            H->unLock();

            now_monotonic_high_precision_timeval(&ETV);

            info("Predicted %u/%u units in %llu usec", NumPredicted, NumUnits,
                 dt_usec(&ETV, &BTV));
        }
        netdata_rwlock_unlock(&Cfg.HostsLock);

        heartbeat_next(&HB, 1 * USEC_PER_SEC);
#endif
    }

    netdata_thread_cleanup_pop(1);
}
