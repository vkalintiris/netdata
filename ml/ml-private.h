#ifndef ML_PRIVATE_H
#define ML_PRIVATE_H

#include "daemon/common.h"

#define ML_OK                     1
#define ML_ERR_ZERO_DIMS          2
#define ML_ERR_SET_UPDATE         3
#define ML_ERR_DIM_UPDATE         4
#define ML_ERR_NO_STORAGE_NUMBER  5
#define ML_ERR_NOT_ENOUGH_SAMPLES 6
#define ML_OK_NOT_YET             7

struct ml_thread_info {
    RRDHOST *host;
    RRDSET *set;

    size_t train_every;
    size_t num_samples, num_dims_per_sample;
    size_t diff_n, smooth_n, lag_n;

    size_t bytes_per_feature;
    calculated_number *train_data;

    time_t curr_training_time;

    /* Fields that allow us to log errors, track perf, etc */
    FILE *log_fp;

    size_t loop_counter;
    struct timeval curr_loop_begin;
    struct timeval curr_loop_end;
    usec_t max_loop_duration;

    struct timeval update_begin;
    struct timeval update_end;
    usec_t max_update_duration;

    struct timeval train_begin;
    struct timeval train_end;
    usec_t max_train_duration;

    size_t status;

    time_t dim_latest_time;
    time_t dim_oldest_time;
    size_t num_collected_samples;
    const char *dim_name;

    size_t num_total_charts;
    size_t num_trained_charts;

    size_t max_feature_size;
};

extern struct ml_thread_info mti;

void ml_kmeans(void);

#endif /* ML_PRIVATE_H */
