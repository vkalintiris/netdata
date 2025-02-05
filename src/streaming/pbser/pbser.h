#ifndef NETDATA_PBSER_H
#define NETDATA_PBSER_H


#ifdef __cplusplus
extern "C" {
#endif

#include "database/rrd.h"

void pbser_rrdhost_init(RRDHOST *rh);
void pbser_rrdhost_new_chart_id(RRDHOST *rh, RRDSET *rs);
void pbser_rrdhost_fini(RRDHOST *rh);

void pbser_chart_update_start(RRDSET *rs);
void pbser_chart_update_metric(RRDDIM *rd, usec_t point_end_time_ut, NETDATA_DOUBLE n);
void pbser_chart_update_end(RRDSET *rs);

#ifdef __cplusplus
}
#endif

#endif /*  NETDATA_PBSER_H */
