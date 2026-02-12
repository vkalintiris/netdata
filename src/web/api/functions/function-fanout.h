// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef NETDATA_FUNCTION_FANOUT_H
#define NETDATA_FUNCTION_FANOUT_H

#include "database/rrd.h"

#define RRDFUNCTIONS_FANOUT_HELP "Fan out a function call to all nodes that support it and collect their results."

int function_fanout(struct rrd_function_execute *rfe, void *data);

#endif //NETDATA_FUNCTION_FANOUT_H
