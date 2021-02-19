#ifndef ML_CGO_H
#define ML_CGO_H

extern struct rrdset;
typedef struct rrdset RRDSET;

int rrdset_num_dims(const RRDSET *st);
int rrdset_update_every(const RRDSET *st);

#endif /* ML_CFGO_H */
