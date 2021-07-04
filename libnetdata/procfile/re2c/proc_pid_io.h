#ifndef PROC_PID_IO_H
#define PROC_PID_IO_H

#include "common.h"

typedef struct {
    uint64_t rchar;
    uint64_t wchar;
    uint64_t syscr;
    uint64_t syscw;
    uint64_t read_bytes;
    uint64_t write_bytes;
    uint64_t cancelled_write_bytes;
} proc_pid_io_t;

void proc_pid_io(char *buf, proc_pid_io_t *pid_io);

#endif /* PROC_PID_IO_H */
