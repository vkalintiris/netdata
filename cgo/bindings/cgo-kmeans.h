#ifndef CGO_KMEANS_H
#define CGO_KMEANS_H

#include "cgo-rrd.h"
#include <stdlib.h>

typedef struct KMeans* KMREF;

KMREF kmref_new(int num_centers);
void kmref_train(KMREF kmref, RRDRP res, int diff_n, int smooth_n, int lag_n);
double kmref_predict(KMREF kmref, RRDRP res, int diff_n, int smooth_n, int lag_n);

#endif /* CGO_KMEANS_H */
