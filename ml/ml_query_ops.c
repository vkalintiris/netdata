// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

#if 0
unsigned
ml_query_dim(RRDDIM *dim, calculated_number *cns, unsigned ns)
{
    assert(dim->update_every != 0);

    // Use the performant, low-level query ops API.
    struct rrddim_query_ops *ops = &dim->state->query_ops;
    struct rrddim_query_handle handle;

    // Our time window.
    time_t time_before = now_realtime_sec() - 1;
    time_t time_after = time_before - (ns * dim->update_every - 1);

    unsigned idx = 0;

    ops->init(dim, &handle, time_after, time_before);

    while (!ops->is_finished(&handle)) {
        time_t time_curr;

        storage_number sn = ops->next_metric(&handle, &time_curr);

        // Nothing we can do here, other than replacing the current number
        // with the previous one. Writing 0.0L is not correct because dims
        // that reach this function have had all their values available in
        // the training stage. For the time being, simply return and keep
        // the same anomaly score, until the next iteration of the prediction
        // thread.
        if (did_storage_number_reset(sn) || !does_storage_number_exist(sn))
            break;

        // Save the number in our buffer.
        cns[idx++] = unpack_storage_number(sn);
    }

    ops->finalize(&handle);

    return idx;
}
#endif

unsigned
ml_query_dim(RRDDIM *dim, int dim_idx,
             calculated_number *cns, unsigned ns, unsigned ndps)
{
    assert(dim->update_every != 0);

    // Use the performant, low-level query ops API.
    struct rrddim_query_ops *ops = &dim->state->query_ops;
    struct rrddim_query_handle handle;

    // Our time window.
    time_t time_before = now_realtime_sec() - 1;
    time_t time_after = time_before - (ns * dim->update_every - 1);

    unsigned row_idx = 0;

    ops->init(dim, &handle, time_after, time_before);

    while (!ops->is_finished(&handle)) {
        time_t time_curr;

        storage_number sn = ops->next_metric(&handle, &time_curr);

        // Nothing we can do here, other than replacing the current number
        // with the previously found one. Writing 0.0L is not correct because
        // dims that reach this function have had all their values available
        // in the training stage. For the time being, simply return and keep
        // the same anomaly score, until the next iteration of the prediction
        // thread.
        if (did_storage_number_reset(sn) || !does_storage_number_exist(sn))
            break;

        // Save the number in our buffer.
        cns[(row_idx * ndps) + dim_idx] = unpack_storage_number(sn);

        row_idx++;
        if (row_idx > ns)
            break;
    }

    ops->finalize(&handle);

    return row_idx;
}
