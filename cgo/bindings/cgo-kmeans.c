#include "cgo-kmeans.h"
#include "ml/kmeans/kmeans-c.h"
#include "database/rrd.h"

KMREF kmref_new(int num_centers) {
    return kmeans_new(num_centers);
}

void kmref_train(KMREF kmref, RRDRP res, int diff_n, int smooth_n, int lag_n) {
    size_t ns = res->rows;
    size_t ndps = res->d;

    size_t bytes_per_feature = sizeof(calculated_number) * ndps * (lag_n + 1);

    calculated_number *cns = callocz(ns, bytes_per_feature);
    memcpy(cns, res->v, sizeof(calculated_number) * ndps * ns);
    rrdr_free(res);

    kmeans_train(kmref, cns, ns, ndps, diff_n, smooth_n, lag_n);
    free(cns);
}

double kmref_predict(KMREF kmref, RRDRP res, int diff_n, int smooth_n, int lag_n) {
    size_t ns = res->rows;
    size_t ndps = res->d;

    size_t bytes_per_feature = sizeof(calculated_number) * ndps * (lag_n + 1);

    calculated_number *cns = callocz(ns, bytes_per_feature);
    memcpy(cns, res->v, sizeof(calculated_number) * ndps * ns);
    rrdr_free(res);

    double d = kmeans_anomaly_score(kmref, cns, ns, ndps, diff_n, smooth_n, lag_n);

    free(cns);

    return d;
}
