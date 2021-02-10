// SPDX-License-Identifier: GPL-3.0-or-later

#include "common.h"
#include "ml/kmeans/kmeans-c.h"

#define DIFF_N 1
#define SMOOTH_N 3
#define LAG_N 5

RRDR *get_rrdr(RRDSET *set, time_t time_after, time_t time_before) {
    if (time_after >= time_before)
        fatal("time_after >= time_before (%ld >= %ld)", time_after, time_before);

    RRDR *res = rrd2rrdr(
        set,
        0, /* points_requested */
        time_after, /* after */
        time_before, /* before */
        RRDR_GROUPING_AVERAGE, /* grouping method */
        0, /* resampling time */
        0, /* grouping options */
        NULL, /* dimensions */
        NULL /* context params */
    );

    if (!res) {
        fatal("RRDR result is empty\n");
    }

    size_t max_possible_rows = time_before - time_after;
    if (res->rows > max_possible_rows)
        fatal("res->rows > max_possible_rows (%ld > %zu)", res->rows, max_possible_rows);

    size_t row_diff = max_possible_rows - res->rows;
    if (row_diff > 2)
        fatal("Row diff = %zu", row_diff);

    info("result contains %ld rows", res->rows);
    for (long i = 0; i != res->rows; i++) {
        calculated_number *cn = &res->v[res->d * i];
        RRDR_VALUE_FLAGS *vf = &res->o[res->d * i];

        for (long j = 0; j != res->d; j++)
            if (vf[j] && RRDR_VALUE_EMPTY)
                fatal("Found empty value!");
    }

    return res;
}

void foobar(const char *hostname, time_t time_after, time_t time_before) {
    RRDHOST *host = rrdhost_find_by_hostname(hostname, simple_hash(hostname));
    RRDSET *set = rrdset_find_byname(host, "example_local1.random");

    info("Host: %s, Set: %s",
         host->hostname ? host->hostname : "unnamed",
         set->name ? set->name : "unnamed");

    RRDR *res = get_rrdr(set, time_after, time_before);

    size_t num_samples = res->rows;
    size_t num_dims_per_sample = res->d;
    size_t bytes_per_feature = sizeof(calculated_number) * num_dims_per_sample * (LAG_N + 1);

    calculated_number *cns = callocz(num_samples, bytes_per_feature);
    memcpy(cns, res->v, sizeof(calculated_number) * num_dims_per_sample * num_samples);

    for (long i = 0; i != res->rows; i++) {
        calculated_number *cno = &res->v[res->d * i];
        calculated_number *cnn = &cns[res->d * i];

        for (long j = 0; j != res->d; j++)
            if (cno[j] != cnn[j])
                fatal("cno[%ld][%ld] != cnn[%ld][%ld]: %Lf != %Lf", i, j, i, j, cno[j], cnn[j]);
    }

    freez(cns);
    rrdr_free(res);
}
