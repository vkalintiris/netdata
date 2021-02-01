// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

static void
cleanup_predict_thread(void *ptr)
{
    struct netdata_static_thread *thr = ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up prediction thread");
    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

void
ml_predict_main(struct netdata_static_thread *thr)
{
    netdata_thread_cleanup_push(cleanup_predict_thread, thr);

    while (ml_heartbeat(ml_cfg.predict_heartbeat)) {
        info("---");
        ml_dict_predict();
    }

    netdata_thread_cleanup_pop(1);
}
