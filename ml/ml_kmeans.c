// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"
#include "ml/kmeans/kmeans-c.h"

static bool
kmeans_update_dim(RRDDIM *dim, size_t dim_idx) {
    struct rrddim_query_ops *query_ops = &dim->state->query_ops;
    struct rrddim_query_handle query_handle;

    time_t latest_time = mti.curr_training_time - 1;
    time_t oldest_time = mti.curr_training_time - mti.num_samples;

    size_t num_collected_samples = 0;

    query_ops->init(dim, &query_handle, oldest_time, latest_time);
    do {
        storage_number st_num;
        time_t curr_time;

        st_num = query_ops->next_metric(&query_handle, &curr_time);
        if (!does_storage_number_exist(st_num)) {
            mti.dim_name = dim->name ? dim->name : "unnamed";
            mti.dim_latest_time = latest_time;
            mti.dim_oldest_time = oldest_time;
            mti.status = ML_ERR_NO_STORAGE_NUMBER;
            return false;
        }

        size_t offset = (num_collected_samples * mti.num_dims_per_sample) + dim_idx;
        mti.set->train_data[offset] = unpack_storage_number(st_num);;

        num_collected_samples++;
    } while (!query_ops->is_finished(&query_handle));
    query_ops->finalize(&query_handle);

    if (num_collected_samples != mti.num_samples) {
        mti.dim_name = dim->name ? dim->name : "unnamed";
        mti.dim_latest_time = latest_time;
        mti.dim_oldest_time = oldest_time;
        mti.num_collected_samples = num_collected_samples;
        mti.status = ML_ERR_NOT_ENOUGH_SAMPLES;
        return false;
    }

    return true;
}

void ml_kmeans(void) {
    // Fill ML thread info struct with data specific to this chart
    mti.chart_name = mti.set->name ? mti.set->name : "unnamed";

    if (mti.set->update_every != 1) {
        mti.status = ML_ERR_SET_UPDATE;
        return;
    }

    mti.num_dims_per_sample = 0;
    for (RRDDIM *dim = mti.set->dimensions; dim; dim = dim->next) {
        mti.num_dims_per_sample++;

        if (dim->update_every != 1) {
            mti.dim_name = dim->name ? dim->name : "unnamed";
            mti.status = ML_ERR_DIM_UPDATE;
            return;
        }
    }

    if (mti.num_dims_per_sample == 0) {
        mti.status = ML_ERR_ZERO_DIMS;
        return;
    }

    mti.curr_training_time = now_realtime_sec();

    // Make sure this is the right time to train this set
    if (now_realtime_sec() < (mti.set->last_trained_at + mti.train_every)) {
        mti.status = ML_OK;
        return;
    }

    // Collect updated sample data
    now_monotonic_high_precision_timeval(&mti.update_begin);

    mti.bytes_per_feature =
        sizeof(calculated_number) * mti.num_dims_per_sample * (mti.lag_n + 1);

    mti.train_data = callocz(mti.num_samples, mti.bytes_per_feature);

    size_t dim_idx = 0;
    for (RRDDIM *dim = mti.set->dimensions; dim; dim = dim->next) {
        if (!kmeans_update_dim(dim, dim_idx))
            return;

        dim_idx++;
    }

    now_monotonic_high_precision_timeval(&mti.update_end);

    // Run the actual k-means clustering
    now_monotonic_high_precision_timeval(&mti.train_begin);

    kmeans_ref km_ref = kmeans_new(2);
    kmeans_train(km_ref, mti.set->train_data,
                 mti.num_samples, mti.num_dims_per_sample,
                 mti.diff_n, mti.smooth_n, mti.lag_n);
    kmeans_delete(km_ref);

    // Save training time
    mti.set->last_trained_at = now_realtime_sec();

    now_monotonic_high_precision_timeval(&mti.train_end);
}
