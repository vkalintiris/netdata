#ifndef ML_H
#define ML_H

#include <stddef.h>
#include <time.h>

extern size_t num_dims_per_sample;
extern size_t diff_n;
extern size_t smooth_n;
extern size_t lag_n;

extern void set_kmeans_conf_from_env(void);
extern void foobar(const char *, time_t, time_t);

#endif /* ML_H */
