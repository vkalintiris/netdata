#ifndef PROC_STAT_H
#define PROC_STAT_H

#include "common.h"

typedef struct {
    uint64_t user;
    uint64_t nice;
    uint64_t system;
    uint64_t idle;
    uint64_t iowait;
    uint64_t irq;
    uint64_t softirq;
    uint64_t steal;
    uint64_t guest;
    uint64_t guest_nice;
} proc_stat_t;

void proc_stat(char *buf, proc_stat_t *pstat);

#endif /* PROC_STAT_H */
