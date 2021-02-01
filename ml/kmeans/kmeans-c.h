// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef KMEANS_C_H
#define KMEANS_C_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef long double calculated_number;

typedef struct KMeans *kmeans_ref;

kmeans_ref
kmeans_new(size_t num_centers);

void
kmeans_train(kmeans_ref km_ref, calculated_number *calc_nums,
             size_t num_samples, size_t num_dims_per_sample,
             size_t diff_n, size_t smooth_n, size_t lag_n);

calculated_number
kmeans_anomaly_score(kmeans_ref km_ref, calculated_number *calc_nums,
                     size_t num_samples, size_t num_dims_per_sample,
                     size_t diff_n, size_t smooth_n, size_t lag_n);

calculated_number
kmeans_min_distance(kmeans_ref km_ref);

calculated_number
kmeans_max_distance(kmeans_ref km_ref);

void
kmeans_delete(kmeans_ref km_ref);

#ifdef __cplusplus
};
#endif

#endif /* KMEANS_C_H */
