// SPDX-License-Identifier: GPL-3.0-or-later

#include "daemon/common.h"

static void ml_thread_cleanup(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up thread...\n");
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

void *
ml_main(void *ptr) {
    netdata_thread_cleanup_push(ml_thread_cleanup, ptr);

    heartbeat_t hb;
    heartbeat_init(&hb);

    while (!netdata_exit) {
        netdata_thread_testcancel();

        usec_t hb_step = 1 * USEC_PER_SEC;
        heartbeat_next(&hb, hb_step);

        if (netdata_exit)
            break;

        info("Running ML loop");
    }

    netdata_thread_cleanup_pop(1);
    return NULL;
}
