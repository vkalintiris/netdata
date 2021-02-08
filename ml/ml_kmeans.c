// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"
#include "ml/kmeans/kmeans-c.h"

static bool
should_train_now(void) {
    // Never train sets with update every != 1s
    if (mti.set->update_every != 1)
        return false;

    // Never train sets with dims that have update every != 1s
    mti.num_dims_per_sample = 0;
    for (RRDDIM *dim = mti.set->dimensions; dim; dim = dim->next) {
        if (dim->update_every != 1)
            return false;

        mti.num_dims_per_sample++;
    }

    // Nothing to train if set has 0 dims
    if (mti.num_dims_per_sample == 0)
        return false;

    time_t now = now_realtime_sec();

    if (mti.set->last_trained_at == 0)
        mti.set->last_trained_at = now;

    if (now < (mti.set->last_trained_at + mti.train_every))
        return false;

    mti.set->last_trained_at = now;
    return true;
}

static bool
kmeans_update_dim(RRDDIM *dim, size_t dim_idx) {
    struct rrddim_query_ops *query_ops = &dim->state->query_ops;
    struct rrddim_query_handle query_handle;

    time_t latest_time = mti.set->last_trained_at - 1;
    time_t oldest_time = mti.set->last_trained_at - mti.num_samples;

    size_t num_collected_samples = 0;

    query_ops->init(dim, &query_handle, oldest_time, latest_time);
    do {
        storage_number st_num;
        time_t curr_time;

        st_num = query_ops->next_metric(&query_handle, &curr_time);
        if (!does_storage_number_exist(st_num)) {
            num_collected_samples++;
            continue;
        }

        size_t offset = (num_collected_samples * mti.num_dims_per_sample) + dim_idx;
        mti.set->train_data[offset] = unpack_storage_number(st_num);;

        num_collected_samples++;
    } while (!query_ops->is_finished(&query_handle));
    query_ops->finalize(&query_handle);

    if (num_collected_samples != mti.num_samples) {
        fprintf(mti.log_fp, "\"%s.%s\" has only %ld samples in [%ld, %ld]\n",
                mti.set->name ? mti.set->name : "unnamed",
                dim->name ? dim->name : "unnamed",
                num_collected_samples, oldest_time, latest_time);
        return false;
    }

    return true;
}

static bool
collect_sample_data(void) {
    mti.bytes_per_feature = sizeof(calculated_number) * mti.num_dims_per_sample * (mti.lag_n + 1);
    mti.set->train_data = callocz(mti.num_samples, mti.bytes_per_feature);

    if (mti.max_feature_size < mti.bytes_per_feature)
        mti.max_feature_size = mti.bytes_per_feature;

    size_t dim_idx = 0;
    for (RRDDIM *dim = mti.set->dimensions; dim; dim = dim->next) {
        if (!kmeans_update_dim(dim, dim_idx)) {
            freez(mti.set->train_data);
            return false;
        }

        dim_idx++;
    }

    return true;
}

static bool
run_kmeans(void) {
    kmeans_ref km_ref = kmeans_new(2);

    kmeans_train(km_ref, mti.set->train_data,
            mti.num_samples, mti.num_dims_per_sample,
            mti.diff_n, mti.smooth_n, mti.lag_n);

    kmeans_delete(km_ref);
    freez(mti.set->train_data);

    mti.num_trained_charts++;
}

void ml_kmeans(void) {
    // Check if we shouldn't train this set
    if (!should_train_now())
        return;

    info("updating chart: \"%s\"", mti.set->name ? mti.set->name : "unnamed");

    // Run Update
    now_monotonic_high_precision_timeval(&mti.update_begin);
    if (!collect_sample_data())
        return;
    now_monotonic_high_precision_timeval(&mti.update_end);

    usec_t update_dt = dt_usec(&mti.update_end, &mti.update_begin);
    if (update_dt > mti.max_update_duration)
        mti.max_update_duration = update_dt;

    info("running kmeans on chart: \"%s\"", mti.set->name ? mti.set->name : "unnamed");

    // Run K-Means
    now_monotonic_high_precision_timeval(&mti.train_begin);
    if (!run_kmeans())
        return;
    now_monotonic_high_precision_timeval(&mti.train_end);

    usec_t train_dt = dt_usec(&mti.train_end, &mti.train_begin);
    if (train_dt > mti.max_train_duration)
        mti.max_train_duration = train_dt;

    fprintf(mti.log_fp, "\"%s\": update_dt = %Lu usec, train_dt = %Lu usec\n",
            mti.set->name ? mti.set->name : "unnamed", update_dt, train_dt);
}
