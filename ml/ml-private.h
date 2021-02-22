#ifndef ML_COMMON_H
#define ML_COMMON_H

#include "ml.h"
#include "kmeans/kmeans-c.h"
#include "daemon/common.h"
#include <stdbool.h>

#define ML_LOG_FILE "/tmp/ml.log"

struct ml_conf {
    int enabled;

    size_t num_samples;
    size_t train_every;

    size_t diff_n;
    size_t smooth_n;
    size_t lag_n;

    heartbeat_t hb;
    size_t loop_counter;
    FILE *fp;
};

void ml_read_conf(struct ml_conf *mlc);

calculated_number *ml_get_calculated_numbers(struct ml_conf *mlc, RRDSET *st,
                                             size_t *ns, size_t *ndps);
void train_charts(struct ml_conf *mlc);
void predict_charts(struct ml_conf *mlc);

#endif /* ML_COMMON_H */
