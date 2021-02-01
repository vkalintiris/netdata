#ifndef NETDATA_ML_H
#define NETDATA_ML_H

#define NETDATA_PLUGIN_HOOK_ML_TRAIN \
{ \
    .name = "MLTRAIN", \
    .config_section = NULL, \
    .config_name = NULL, \
    .enabled = 1, \
    .thread = NULL, \
    .init_routine = NULL, \
    .start_routine = ml_train \
},

#define NETDATA_PLUGIN_HOOK_ML_PREDICT \
{ \
    .name = "MLPREDICT", \
    .config_section = NULL, \
    .config_name = NULL, \
    .enabled = 1, \
    .thread = NULL, \
    .init_routine = NULL, \
    .start_routine = ml_predict \
},

void *ml_train(void *arg);

void *ml_predict(void *arg);

#endif /* NETDATA_ML_H */
