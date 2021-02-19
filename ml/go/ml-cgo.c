#include "ml-cgo.h"
#include "ml/ml-private.h"

RRDSETP curr_set;

const char *rrdset_name(RRDSETP st) {
    return st->name;
}

int rrdset_num_dims(RRDSETP st) {
    int num_dims = 0;

    for (RRDDIM *dim = st->dimensions; dim; dim = dim->next)
        num_dims++;

    return num_dims;
}

int rrdset_update_every(RRDSETP st) {
    return st->update_every;
}
