// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef NETDATA_ML_H
#define NETDATA_ML_H

#ifdef __cplusplus
extern "C" {
#endif

#include "daemon/common.h"

typedef struct ml_host_handle {
    void* HostPtr;
} ml_host_handle_t;

ml_host_handle_t *ml_host_new(RRDHOST *RH);
void ml_host_delete(ml_host_handle_t *host_handle);

void ml_host_new_unit(RRDDIM *RD);
void ml_host_delete_unit(RRDDIM *RD);

void ml_init(void);

#ifdef __cplusplus
};
#endif

#define CONFIG_SECTION_ML "ml"
#define CONFIG_NAME_ML "enabled"

#endif /* NETDATA_ML_H */
