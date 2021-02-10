// SPDX-License-Identifier: GPL-3.0-or-later

#include "common.h"
#include "ml/kmeans/kmeans-c.h"
#include "ml.h"

size_t num_dims_per_sample;
size_t diff_n;
size_t smooth_n;
size_t lag_n;

static size_t size_t_envvar(const char *name) {
    const char *cbuf = getenv(name);
    if (!cbuf)
        fatal("Environment variable \"%s\" is unset", name);

    return atoi(cbuf);
}

void set_kmeans_conf_from_env(void) {
    num_dims_per_sample = size_t_envvar("NUM_DIMS_PER_SAMPLE");
    diff_n = size_t_envvar("DIFF_N");
    smooth_n = size_t_envvar("SMOOTH_N");
    lag_n = size_t_envvar("LAG_N");

    size_t sum = num_dims_per_sample + diff_n + smooth_n + lag_n;
    if (sum > (3600 * 24 * 7)) {
        fatal("Env values are probably wrong");
    }
}

static void
run_kmeans(calculated_number *cns,
           size_t num_samples, size_t num_dims_per_sample,
           size_t diff_n, size_t smooth_n, size_t lag_n) {

    info("Running kmeans with ns: %zu, ndps: %zu, dn: %zu, sn: %zu, ln: %zu",
         num_samples, num_dims_per_sample, diff_n, smooth_n, lag_n);

    kmeans_ref km_ref = kmeans_new(2);

    kmeans_train(km_ref, cns, num_samples, num_dims_per_sample,
                 diff_n, smooth_n, lag_n);

    kmeans_delete(km_ref);
};

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
#ifdef KMEANS_CHECKS
    for (long i = 0; i != res->rows; i++) {
        calculated_number *cn = &res->v[res->d * i];
        RRDR_VALUE_FLAGS *vf = &res->o[res->d * i];

        for (long j = 0; j != res->d; j++)
            if (vf[j] && RRDR_VALUE_EMPTY)
                fatal("Found empty value!");
    }
#endif

    return res;
}

static void dump_stats(usec_t sb_dt, usec_t km_dt, usec_t total_dt,
                       size_t num_samples) {
    FILE *fp = fopen("/tmp/stats.log", "a");
    if (!fp)
        fatal("Could not open stats log file");

    fprintf(fp, "%zu %zu %zu %zu %zu %Lu %Lu %Lu\n",
            num_samples, num_dims_per_sample,
            diff_n, smooth_n, lag_n,
            sb_dt / USEC_PER_MS,
            km_dt / USEC_PER_MS,
            total_dt / USEC_PER_MS);

    fflush(fp);
    fclose(fp);
}

void foobar(const char *hostname, time_t time_after, time_t time_before) {
    struct timeval tv_begin, tv_end;
    struct timeval tv_sb_begin, tv_sb_end;
    struct timeval tv_km_begin, tv_km_end;

    now_monotonic_high_precision_timeval(&tv_begin);
    now_monotonic_high_precision_timeval(&tv_sb_begin);

    RRDHOST *host = rrdhost_find_by_hostname(hostname, simple_hash(hostname));
    RRDSET *set = rrdset_find_byname(host, "example_local1.random");

    RRDR *res = get_rrdr(set, time_after, time_before);

    size_t num_samples = res->rows;
    size_t num_dims_per_sample = res->d;
    size_t bytes_per_feature = sizeof(calculated_number) * num_dims_per_sample * (lag_n + 1);

    calculated_number *cns = callocz(num_samples, bytes_per_feature);
    memcpy(cns, res->v, sizeof(calculated_number) * num_dims_per_sample * num_samples);

    now_monotonic_high_precision_timeval(&tv_sb_end);

#ifdef KMEANS_CHECKS
    for (long i = 0; i != res->rows; i++) {
        calculated_number *cno = &res->v[res->d * i];
        calculated_number *cnn = &cns[res->d * i];

        for (long j = 0; j != res->d; j++)
            if (cno[j] != cnn[j])
                fatal("cno[%ld][%ld] != cnn[%ld][%ld]: %Lf != %Lf", i, j, i, j, cno[j], cnn[j]);
    }
#endif

    now_monotonic_high_precision_timeval(&tv_km_begin);
    run_kmeans(cns, num_samples, num_dims_per_sample, diff_n, smooth_n, lag_n);
    now_monotonic_high_precision_timeval(&tv_km_end);

    freez(cns);
    rrdr_free(res);

    usec_t sb_dt = dt_usec(&tv_sb_end, &tv_sb_begin);
    info("samples buffer time: %Lu msec", sb_dt / USEC_PER_MS);

    usec_t km_dt = dt_usec(&tv_km_end, &tv_km_begin);
    info("k-means time: %Lu msec", km_dt / USEC_PER_MS);

    now_monotonic_high_precision_timeval(&tv_end);
    usec_t total_dt = dt_usec(&tv_end, &tv_begin);
    info("total time: %Lu msec", total_dt / USEC_PER_MS);

    dump_stats(sb_dt, km_dt, total_dt, num_samples);
}
