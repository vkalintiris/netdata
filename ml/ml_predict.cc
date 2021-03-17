// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Database.h"
#include "Unit.h"

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

    while (!netdata_exit) {
        DB.predictUnits();
        sleep_usec(1 * USEC_PER_SEC);
        //heartbeat_next(&HB, 1 * USEC_PER_SEC);
    }

    netdata_thread_cleanup_pop(1);
}
