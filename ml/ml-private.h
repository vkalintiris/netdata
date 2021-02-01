// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_PRIVATE_H
#define ML_PRIVATE_H

#include "ml.h"
#include "kmeans/kmeans-c.h"
#include "daemon/common.h"
#include <stdbool.h>

// Prefix id for anomaly score charts
#define ML_CHART_PREFIX   "kmeans"

// for dim identification we concatenate the set's id with the dim's id
#define ML_UNIT_MAX_ID ((2 * RRD_ID_LENGTH_MAX) + 1)

// maximum number of dims
#define ML_UNIT_MAX_NUM_DIMS_PER_SET 100

typedef struct ml_unit
{
    // An ML unit is either a set or a dim.
    RRDSET *set;
    RRDDIM *dim;

    // Dim-only fields
    bool owns_dim_opts;
    bool predicted;

    /*
     * Shared fields used by both set and dim units.
    */

    // Number of dims in a set.
    int num_dims;

    // Used to track which dimensions can be trained/predicted.
    RRDR_DIMENSION_FLAGS *dim_opts;

    // Opaque reference to our K-Means implementation.
    kmeans_ref km_ref;

    // The anomaly score of this unit.
    calculated_number anomaly_score;

    // When this unit was last trained.
    time_t last_trained_at;

    // Lock shared between training and prediction threads.
    netdata_rwlock_t rwlock;

    // Anomaly score charts
    RRDSET *ml_chart;
    RRDDIM *ml_dim;
    bool ml_chart_updated;
} ml_unit_t;

typedef struct ml_config
{
    bool initialized;

    time_t train_secs;
    time_t train_every;

    size_t diff_n;
    size_t smooth_n;
    size_t lag_n;

    size_t train_heartbeat;
    size_t predict_heartbeat;

    SIMPLE_PATTERN *skip_charts;
    SIMPLE_PATTERN *train_per_dim;

    SIMPLE_PATTERN *anomaly_score_charts;

    DICTIONARY *train_dict;

    netdata_rwlock_t predict_dict_rwlock;
    DICTIONARY *predict_dict;

    DICTIONARY *ml_charts_dict;
} ml_config_t;

extern ml_config_t ml_cfg;

bool ml_heartbeat(size_t secs);

void ml_train_main(struct netdata_static_thread *thr);
void ml_predict_main(struct netdata_static_thread *thr);

ml_unit_t *ml_dict_get_unit_dim(DICTIONARY *dict, RRDDIM *dim);
ml_unit_t *ml_dict_get_unit_set(DICTIONARY *dict, RRDSET *set);

void ml_dict_train(void);
void ml_dict_predict(void);

void ml_unit_train(ml_unit_t *unit);
void ml_unit_predict(ml_unit_t *unit);

void ml_anomaly_score_unit(ml_unit_t *unit);

unsigned ml_query_dim(RRDDIM *dim, int dim_idx,
                      calculated_number *cns, unsigned ns, unsigned ndps);

RRDR *ml_rrdr_for_unit(ml_unit_t *unit);

void ml_kmeans_unit(ml_unit_t *unit);

void ml_chart_update_unit(ml_unit_t *unit);

#endif /* ML_PRIVATE_H */
