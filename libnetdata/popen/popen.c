// SPDX-License-Identifier: GPL-3.0-or-later

#include "../libnetdata.h"

#define PIPE_READ 0
#define PIPE_WRITE 1

/* custom_popene flag definitions */
#define FLAG_CREATE_PIPE    1 // Create a pipe like popen() when set, otherwise set stdout to /dev/null
#define FLAG_CLOSE_FD       2 // Close all file descriptors other than STDIN_FILENO, STDOUT_FILENO, STDERR_FILENO

/*
 * Returns -1 on failure, 0 on success. When FLAG_CREATE_PIPE is set, on success set the FILE *fp pointer.
 */
static inline int custom_popene(const char *command, volatile pid_t *pidptr, char **env, uint8_t flags, FILE **fpp) {
    FILE *fp = NULL;
    int ret = 0; // success by default
    int pipefd[2], error;
    pid_t pid;
    char *const spawn_argv[] = {
            "sh",
            "-c",
            (char *)command,
            NULL
    };
    posix_spawnattr_t attr;
    posix_spawn_file_actions_t fa;

    if (flags & FLAG_CREATE_PIPE) {
        if (pipe(pipefd) == -1)
            return -1;
        if ((fp = fdopen(pipefd[PIPE_READ], "r")) == NULL) {
            goto error_after_pipe;
        }
    }

    if (flags & FLAG_CLOSE_FD) {
        // Mark all files to be closed by the exec() stage of posix_spawn()
        int i;
        for (i = (int) (sysconf(_SC_OPEN_MAX) - 1); i >= 0; i--) {
            if (i != STDIN_FILENO && i != STDERR_FILENO)
                (void) fcntl(i, F_SETFD, FD_CLOEXEC);
        }
    }

    if (!posix_spawn_file_actions_init(&fa)) {
        if (flags & FLAG_CREATE_PIPE) {
            // move the pipe to stdout in the child
            if (posix_spawn_file_actions_adddup2(&fa, pipefd[PIPE_WRITE], STDOUT_FILENO)) {
                error("posix_spawn_file_actions_adddup2() failed");
                goto error_after_posix_spawn_file_actions_init;
            }
        } else {
            // set stdout to /dev/null
            if (posix_spawn_file_actions_addopen(&fa, STDOUT_FILENO, "/dev/null", O_WRONLY, 0)) {
                error("posix_spawn_file_actions_addopen() failed");
                // this is not a fatal error
            }
        }
    } else {
        error("posix_spawn_file_actions_init() failed.");
        goto error_after_pipe;
    }
    if (!(error = posix_spawnattr_init(&attr))) {
        // reset all signals in the child
        sigset_t mask;

        if (posix_spawnattr_setflags(&attr, POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_SETSIGDEF))
            error("posix_spawnattr_setflags() failed.");
        sigemptyset(&mask);
        if (posix_spawnattr_setsigmask(&attr, &mask))
            error("posix_spawnattr_setsigmask() failed.");
    } else {
        error("posix_spawnattr_init() failed.");
    }

    if (!posix_spawn(&pid, "/bin/sh", &fa, &attr, spawn_argv, env)) {
        *pidptr = pid;
        debug(D_CHILDS, "Spawned command: '%s' on pid %d from parent pid %d.", command, pid, getpid());
    } else {
        error("Failed to spawn command: '%s' from parent pid %d.", command, getpid());
        if (flags & FLAG_CREATE_PIPE) {
            fclose(fp);
        }
        ret = -1;
    }
    if (flags & FLAG_CREATE_PIPE) {
        close(pipefd[PIPE_WRITE]);
        if (0 == ret) // on success set FILE * pointer
            *fpp = fp;
    }

    if (!error) {
        // posix_spawnattr_init() succeeded
        if (posix_spawnattr_destroy(&attr))
            error("posix_spawnattr_destroy");
    }
    if (posix_spawn_file_actions_destroy(&fa))
        error("posix_spawn_file_actions_destroy");

    return ret;

error_after_posix_spawn_file_actions_init:
    if (posix_spawn_file_actions_destroy(&fa))
        error("posix_spawn_file_actions_destroy");
error_after_pipe:
    if (flags & FLAG_CREATE_PIPE) {
        if (fp)
            fclose(fp);
        else
            close(pipefd[PIPE_READ]);

        close(pipefd[PIPE_WRITE]);
    }
    return -1;
}

// See man environ
extern char **environ;

FILE *mypopen(const char *command, volatile pid_t *pidptr) {
    FILE *fp = NULL;
    (void)custom_popene(command, pidptr, environ, FLAG_CREATE_PIPE | FLAG_CLOSE_FD, &fp);
    return fp;
}

FILE *mypopene(const char *command, volatile pid_t *pidptr, char **env) {
    FILE *fp = NULL;
    (void)custom_popene(command, pidptr, env, FLAG_CREATE_PIPE | FLAG_CLOSE_FD, &fp);
    return fp;
}

// returns 0 on success, -1 on failure
int netdata_spawn(const char *command, volatile pid_t *pidptr) {
    return custom_popene(command, pidptr, environ, 0, NULL);
}

int custom_pclose(FILE *fp, pid_t pid) {
    int ret;
    siginfo_t info;

    debug(D_EXIT, "Request to mypclose() on pid %d", pid);

    if (fp) {
        // close the pipe fd
        // this is required in musl
        // without it the childs do not exit
        close(fileno(fp));

        // close the pipe file pointer
        fclose(fp);
    }

    errno = 0;

    ret = waitid(P_PID, (id_t) pid, &info, WEXITED);

    if (ret != -1) {
        switch (info.si_code) {
            case CLD_EXITED:
                if(info.si_status)
                    error("child pid %d exited with code %d.", info.si_pid, info.si_status);
                return(info.si_status);

            case CLD_KILLED:
                if(info.si_status == 15) {
                    info("child pid %d killed by signal %d.", info.si_pid, info.si_status);
                    return(0);
                }
                else {
                    error("child pid %d killed by signal %d.", info.si_pid, info.si_status);
                    return(-1);
                }

            case CLD_DUMPED:
                error("child pid %d core dumped by signal %d.", info.si_pid, info.si_status);
                return(-2);

            case CLD_STOPPED:
                error("child pid %d stopped by signal %d.", info.si_pid, info.si_status);
                return(0);

            case CLD_TRAPPED:
                error("child pid %d trapped by signal %d.", info.si_pid, info.si_status);
                return(-4);

            case CLD_CONTINUED:
                error("child pid %d continued by signal %d.", info.si_pid, info.si_status);
                return(0);

            default:
                error("child pid %d gave us a SIGCHLD with code %d and status %d.", info.si_pid, info.si_code, info.si_status);
                return(-5);
        }
    }
    else
        error("Cannot waitid() for pid %d", pid);
    
    return 0;
}

int mypclose(FILE *fp, pid_t pid)
{
    return custom_pclose(fp, pid);
}

int netdata_spawn_waitpid(pid_t pid)
{
    return custom_pclose(NULL, pid);
}
