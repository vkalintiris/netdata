#ifndef NETDATA_ML_H
#define NETDATA_ML_H

void *ml_main(void *ptr);

#define NETDATA_PLUGIN_HOOK_ML \
{ \
    .name = "ML", \
    .config_section = NULL, \
    .config_name = NULL, \
    .enabled = 1, \
    .thread = NULL, \
    .init_routine = NULL, \
    .start_routine = ml_main \
},


#endif /* NETDATA_ML_H */
