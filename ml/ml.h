// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef NETDATA_ML_H
#define NETDATA_ML_H

#ifdef __cplusplus
extern "C" {
#endif

#include "daemon/common.h"

void ml_init(void);

typedef void* ml_host_t;
typedef void* ml_unit_t;

void ml_new_host(RRDHOST *RH);
void ml_delete_host(RRDHOST *RH);

void ml_new_unit(RRDDIM *RD);
void ml_delete_unit(RRDDIM *RD);

bool ml_is_anomalous(RRDDIM *RD);

char *ml_find_anomaly_events(RRDHOST *RH, time_t after, time_t before);

char *ml_get_anomaly_event_info(RRDHOST *RH, time_t after, time_t before);

int ml_test(int argc, char *argv[]);

#ifdef __cplusplus
};
#endif

#define CONFIG_SECTION_ML "ml"
#define CONFIG_NAME_ML "enabled"

#endif /* NETDATA_ML_H */
