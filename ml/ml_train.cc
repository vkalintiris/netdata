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

    heartbeat_t HB;
    heartbeat_init(&HB);

    std::chrono::time_point<std::chrono::steady_clock> StartClock, EndClock;

    while (!netdata_exit) {
        /*
         * Update hosts, charts & units.
         */

        StartClock = std::chrono::steady_clock::now();
        Cfg.updateHosts();
        EndClock = std::chrono::steady_clock::now();

        auto Duration = std::chrono::duration_cast<std::chrono::microseconds>(EndClock - StartClock);
        info("Updated %zu hosts in %ld usec", Cfg.Hosts.size(), Duration.count());

        heartbeat_next(&HB, 1 * USEC_PER_SEC);
    }

    netdata_thread_cleanup_pop(1);
}

};
