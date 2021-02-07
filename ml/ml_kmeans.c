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
            fprintf(mti.log_fp, "\"%s.%s\" missing storage number in [%ld, %ld]\n",
                    mti.chart_name, dim->name ? dim->name : "unnamed",
                    oldest_time, latest_time);
            mti.num_skipped_charts++;
            return false;
        }

        size_t offset = (num_collected_samples * mti.num_dims_per_sample) + dim_idx;
        mti.set->train_data[offset] = unpack_storage_number(st_num);;

        num_collected_samples++;
    } while (!query_ops->is_finished(&query_handle));
    query_ops->finalize(&query_handle);

    if (num_collected_samples != mti.num_samples) {
        fprintf(mti.log_fp, "\"%s.%s\" has only %ld samples in [%ld, %ld]\n",
                mti.chart_name, dim->name ? dim->name : "unnamed",
                num_collected_samples, oldest_time, latest_time);
        mti.num_skipped_charts++;
        return false;
    }

    return true;
}

void ml_kmeans(void) {
    mti.chart_name = mti.set->name ? mti.set->name : "unnamed";

    // Skip charts with update_every != 1
    if (mti.set->update_every != 1) {
        fprintf(mti.log_fp, "\"%s\" has update_every != 1\n", mti.chart_name);
        mti.num_skipped_charts++;
        return;
    }

    // Find the number of dims in this chart
    mti.num_dims_per_sample = 0;
    for (RRDDIM *dim = mti.set->dimensions; dim; dim = dim->next) {
        mti.num_dims_per_sample++;

        // Skip charts with dims that have update every != 1
        if (dim->update_every != 1) {
            fprintf(mti.log_fp, "\"%s.%s\" has update_every != 1\n",
                    mti.chart_name, dim->name ? dim->name : "unnamed");
            mti.num_skipped_charts++;
            return;
        }
    }

    // Skip charts with no dims
    if (mti.num_dims_per_sample == 0) {
        fprintf(mti.log_fp, "\"%s\" has 0 dims\n", mti.chart_name);
        mti.num_skipped_charts++;
        return;
    }

    // Make sure this is the right time to train this set
    if (now_realtime_sec() < (mti.set->last_trained_at + mti.train_every - 1)) {
        fprintf(mti.log_fp, "Skipping because %s's last trained at %ld\n",
                mti.chart_name, mti.set->last_trained_at);
        mti.num_skipped_charts++;
        return;
    }

    // Collect updated sample data
    now_monotonic_high_precision_timeval(&mti.update_begin);

        mti.curr_training_time = now_realtime_sec();

        mti.bytes_per_feature =
            sizeof(calculated_number) * mti.num_dims_per_sample * (mti.lag_n + 1);

        mti.set->train_data = callocz(mti.num_samples, mti.bytes_per_feature);

        size_t dim_idx = 0;
        for (RRDDIM *dim = mti.set->dimensions; dim; dim = dim->next) {
            if (!kmeans_update_dim(dim, dim_idx)) {
                freez(mti.set->train_data);
                return;
            }

            dim_idx++;
        }

    // Run the actual k-means clustering
    now_monotonic_high_precision_timeval(&mti.update_end);

        now_monotonic_high_precision_timeval(&mti.train_begin);

        kmeans_ref km_ref = kmeans_new(2);
        kmeans_train(km_ref, mti.set->train_data,
                     mti.num_samples, mti.num_dims_per_sample,
                     mti.diff_n, mti.smooth_n, mti.lag_n);
        kmeans_delete(km_ref);
        freez(mti.set->train_data);

        // Save training time
        mti.set->last_trained_at = now_realtime_sec();
        mti.num_trained_charts++;

    now_monotonic_high_precision_timeval(&mti.train_end);

    usec_t update_dt = dt_usec(&mti.update_end, &mti.update_begin);
    if (update_dt > mti.max_update_duration)
        mti.max_update_duration = update_dt;

    usec_t train_dt = dt_usec(&mti.train_end, &mti.train_begin);
    if (train_dt > mti.max_train_duration)
        mti.max_train_duration = train_dt;

    fprintf(mti.log_fp, "update_dt = %Lu usec, train_dt = %Lu usec\n",
            update_dt, train_dt);
}
