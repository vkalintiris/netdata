#ifndef PROC_PID_STATUS_H
#define PROC_PID_STATUS_H

#include "common.h"

typedef struct {
    uint64_t euid;
    uint64_t egid;
    uint64_t vm_size;
    uint64_t vm_rss;
    uint64_t rss_file;
    uint64_t rss_shmem;
    uint64_t vm_swap;
} proc_pid_status_t;

void proc_pid_status(char *buf, proc_pid_status_t *pid_status);

#endif /* PROC_PID_STATUS_H */
