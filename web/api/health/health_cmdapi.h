// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef NETDATA_WEB_HEALTH_SVG_H
#define NETDATA_WEB_HEALTH_SVG_H 1

#include "libnetdata/libnetdata.h"
#include "web/server/web_client.h"
#include "health/health.h"

int web_client_api_request_v1_mgmt_health(RRDHOST *host, struct web_client *w, char *url);

#include "web/api/web_api_v1.h"

#endif /* NETDATA_WEB_HEALTH_SVG_H */
