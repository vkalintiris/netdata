#ifndef REPLICATION_H
#define REPLICATION_H

#ifdef __cplusplus
extern "C" {
#endif

#include "daemon/common.h"

void replication_init(void);
void replication_fini(void);

#ifdef __cplusplus
};
#endif

#endif /* REPLICATION_H */
