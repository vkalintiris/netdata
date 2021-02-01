// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef NETDATA_ML_H
#define NETDATA_ML_H

#ifdef __cplusplus
extern "C" {
#endif

void ml_init(void);
void *ml_main(void *ptr);

#ifdef __cplusplus
};
#endif

#define CONFIG_SECTION_ML "ml"
#define CONFIG_NAME_ML "enabled"

#define NETDATA_PLUGIN_HOOK_ML_TRAIN \
{ \
    .name           = "MLTRAIN", \
    .config_section = CONFIG_SECTION_ML, \
    .config_name    = CONFIG_NAME_ML, \
    .enabled        = 1, \
    .thread         = NULL, \
    .init_routine   = ml_init, \
    .start_routine  = ml_main \
},

#define NETDATA_PLUGIN_HOOK_ML_PREDICT \
{ \
    .name           = "MLPREDICT", \
    .config_section = CONFIG_SECTION_ML, \
    .config_name    = CONFIG_NAME_ML, \
    .enabled        = 1, \
    .thread         = NULL, \
    .init_routine   = ml_init, \
    .start_routine  = ml_main \
},

#endif /* NETDATA_ML_H */
