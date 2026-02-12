// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef NETDATA_FUNCTION_FANOUT_H
#define NETDATA_FUNCTION_FANOUT_H

#include "database/rrd.h"

#define RRDFUNCTIONS_FANOUT_HELP "Fan out a function call to all nodes that support it and collect their results."

int function_fanout(BUFFER *wb, const char *function, BUFFER *payload, const char *source);

#endif //NETDATA_FUNCTION_FANOUT_H
