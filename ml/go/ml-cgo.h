#ifndef ML_CGO_H
#define ML_CGO_H

typedef struct rrdset* RRDSETP;

int rrdset_num_dims(RRDSETP st);
int rrdset_update_every(RRDSETP st);

extern RRDSETP curr_set;

#endif /* ML_CFGO_H */
