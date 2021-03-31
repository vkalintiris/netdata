// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Database.h"
#include "Host.h"
#include "Chart.h"
#include "Unit.h"

#if defined(TRACING_ENABLED)
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
#else
static void cleanupTrainThread(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up train thread");
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}
#endif

namespace ml {

void trainMain(struct netdata_static_thread *Thread) {
    netdata_thread_cleanup_push(cleanupTrainThread, Thread);

    size_t LoopCounter = 1;

    while (!netdata_exit) {
        info("[%zu] Training loop start", LoopCounter);

        SPDR_COUNTER1(Cfg.SPDR, "cat", "training-loop", SPDR_INT("iteration", LoopCounter));
        DB.trainUnits();
        SPDR_END(Cfg.SPDR, "cat", "train-units");

        info("[%zu] Training loop end", LoopCounter++);
    }

    netdata_thread_cleanup_pop(1);
}

};
