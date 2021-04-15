// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef NETDATA_ML_H
#define NETDATA_ML_H

#ifdef __cplusplus
extern "C" {
#endif

#include "daemon/common.h"

void ml_host_create(RRDHOST *host);

#ifdef __cplusplus
};
#endif

#define CONFIG_SECTION_ML "ml"
#define CONFIG_NAME_ML "enabled"

#endif /* NETDATA_ML_H */
