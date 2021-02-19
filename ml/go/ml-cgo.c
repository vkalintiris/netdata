#include "ml/ml-private.h"

int rrdset_num_dims(const RRDSET *st) {
    int num_dims = 0;

    for (RRDDIM *dim = st->dimensions; dim; dim = dim->next)
        num_dims++;

    return num_dims;
}

int rrdset_update_every(const RRDSET *st) {
    return st->update_every;
}
