#ifndef REPLICATION_COLLECTOPS_H
#define REPLICATION_COLLECTOPS_H

#ifdef __cplusplus
extern "C" {
#endif

#include "daemon/common.h"

RRDDIM_PAST_DATA *
replication_collect_past_metric_init(RRDHOST *RH, const char *Set, const char *Chart);

void
replication_collect_past_metric(RRDDIM_PAST_DATA *DPD, time_t Timestamp, storage_number SN);

void replication_collect_past_metric_done(RRDDIM_PAST_DATA *DPD);

#ifdef __cplusplus
};
#endif

#endif /* REPLICATION_COLLECTOPS_H */
