// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Database.h"
#include "Host.h"
#include "Chart.h"
#include "Unit.h"

static void cleanupTrainThread(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up train thread");
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

namespace ml {

void trainMain(struct netdata_static_thread *Thread) {
    netdata_thread_cleanup_push(cleanupTrainThread, Thread);

    size_t LoopCounter = 1;

    while (!netdata_exit) {
        info("[%zu] Training loop start", LoopCounter);
        DB.trainUnits();
        info("[%zu] Training loop end", LoopCounter++);
    }

    netdata_thread_cleanup_pop(1);
}

};
