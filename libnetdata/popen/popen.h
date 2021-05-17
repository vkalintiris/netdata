// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef NETDATA_POPEN_H
#define NETDATA_POPEN_H 1

#include "../libnetdata.h"

#define PIPE_READ 0
#define PIPE_WRITE 1

extern FILE *mypopen(const char *command, volatile pid_t *pidptr);
extern FILE *mypopene(const char *command, volatile pid_t *pidptr, char **env);
extern int mypclose(FILE *fp, pid_t pid);
extern int netdata_spawn(const char *command, volatile pid_t *pidptr);
extern int netdata_spawn_waitpid(pid_t pid);

#endif /* NETDATA_POPEN_H */
