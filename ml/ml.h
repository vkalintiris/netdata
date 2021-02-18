#ifndef NETDATA_ML_H
#define NETDATA_ML_H

void *ml_loop(void *ptr);

#define NETDATA_PLUGIN_HOOK_ML_TRAIN \
{ \
    .name = "MLTRAIN", \
    .config_section = NULL, \
    .config_name = NULL, \
    .enabled = 1, \
    .thread = NULL, \
    .init_routine = NULL, \
    .start_routine = ml_loop \
},

#define NETDATA_PLUGIN_HOOK_ML_PREDICT \
{ \
    .name = "MLPREDICT", \
    .config_section = NULL, \
    .config_name = NULL, \
    .enabled = 1, \
    .thread = NULL, \
    .init_routine = NULL, \
    .start_routine = ml_loop \
},

#endif /* NETDATA_ML_H */
