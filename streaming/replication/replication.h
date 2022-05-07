#ifndef REPLICATION_H
#define REPLICATION_H

#ifdef __cplusplus
extern "C" {
#endif

#include "daemon/common.h"

typedef void * replication_handle_t;

void replication_new(RRDHOST *RH);
void replication_delete(RRDHOST *RH);

#ifdef __cplusplus
};
#endif

#endif /* REPLICATION_H */
