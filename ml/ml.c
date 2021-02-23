// SPDX-License-Identifier: GPL-3.0-or-later

#include "daemon/common.h"

extern void GoMLMain(void);

static void
ml_thread_cleanup(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up thread...");
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

void *
ml_main(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    netdata_thread_cleanup_push(ml_thread_cleanup, thr);

    if (!strcmp(thr->name, "MLTRAIN"))
        GoMLMain();

    netdata_thread_cleanup_pop(1);
    return NULL;
}
