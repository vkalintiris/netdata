#ifndef PROC_PID_STAT_H
#define PROC_PID_STAT_H

#include "common.h"

#define MAX_COMM_LEN    128

typedef struct {
    // (1) pid  %d

    // (2) comm  %s
    char comm[MAX_COMM_LEN];

    // (3) state  %c

    // (4) ppid  %d
    uint32_t ppid;

    // (5) pgrp  %d
    // (6) session  %d
    // (7) tty_nr  %d
    // (8) tpgid  %d
    // (9) flags  %u

    // (10) minflt  %lu
    uint64_t minflt;

    // (11) cminflt  %lu
    uint64_t cminflt;

    // (12) majflt  %lu
    uint64_t majflt;

    // (13) cmajflt  %lu
    uint64_t cmajflt;

    // (14) utime  %lu
    uint64_t utime;

    // (15) stime  %lu
    uint64_t stime;

    // (16) cutime  %ld
    uint64_t cutime;

    // (17) cstime  %ld
    uint64_t cstime;

    // (18) priority  %ld
    // (19) nice  %ld

    // (20) num_threads  %ld
    uint32_t num_threads;

    // (21) itrealvalue  %ld

    // (22) starttime  %llu
    uint64_t starttime;

    // (23) vsize  %lu
    // (24) rss  %ld
    // (25) rsslim  %lu
    // (26) startcode  %lu
    // (27) endcode  %lu
    // (28) startstack  %lu
    // (29) kstkesp  %lu
    // (30) kstkeip  %lu
    // (31) signal  %lu
    // (32) blocked  %lu
    // (33) sigignore  %lu
    // (34) sigcatch  %lu
    // (35) wchan  %lu
    // (36) nswap  %lu
    // (37) cnswap  %lu
    // (38) exit_signal  %d
    // (39) processor  %d
    // (40) rt_priority  %u
    // (41) policy  %u
    // (42) delayacct_blkio_ticks  %llu

    // (43) guest_time  %lu
    uint64_t guest_time;

    // (44) cguest_time  %ld
    uint64_t cguest_time;

    // (45) start_data  %lu
    // (46) end_data  %lu
    // (47) start_brk  %lu
    // (48) arg_start  %lu
    // (49) arg_end  %lu
    // (50) env_start  %lu
    // (51) env_end  %lu
    // (52) exit_code  %d
} proc_pid_stat_t;

void proc_pid_stat(char *buf, proc_pid_stat_t *pid_stat);

#endif /* PROC_PID_STAT_H */
