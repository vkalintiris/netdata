#include <string.h>
#include "proc_pid_stat.h"

void cp_comm(char *dest, char *lparen, char *rparen) {
    if (!lparen && !rparen && (lparen < rparen))
        return;

    size_t n = (rparen - lparen);
    if (n > MAX_COMM_LEN)
        n = MAX_COMM_LEN;

    strncpy(dest, lparen + 1, n - 1);
    dest[n - 1] = '\0';
}

void parse_number(proc_pid_stat_t *pid_stat, char *s, int col) {
    switch (col) {
        default:
            break;
        case 4:
            pid_stat->ppid = str2uint32_t(s);
            break;
        case 10:
            pid_stat->minflt = str2uint64_t(s);
            break;
        case 11:
            pid_stat->cminflt = str2uint64_t(s);
            break;
        case 12:
            pid_stat->majflt = str2uint64_t(s);
            break;
        case 13:
            pid_stat->cmajflt = str2uint64_t(s);
            break;
        case 14:
            pid_stat->utime = str2uint64_t(s);
            break;
        case 15:
            pid_stat->stime = str2uint64_t(s);
            break;
        case 16:
            pid_stat->cutime = str2uint64_t(s);
            break;
        case 17:
            pid_stat->cstime = str2uint64_t(s);
            break;
        case 20:
            pid_stat->num_threads = str2uint32_t(s);
            break;
        case 22:
            pid_stat->starttime = str2uint64_t(s);
            break;
        case 43:
            pid_stat->guest_time = str2uint64_t(s);
            break;
        case 44:
            pid_stat->cguest_time = str2uint64_t(s);
            break;
    }
}

void proc_pid_stat(char *buf, proc_pid_stat_t *pid_stat) {
    char *YYCURSOR = buf;

    int col = 0;

    for (;;) {
        char *p = YYCURSOR;

    /*!re2c
        re2c:define:YYCTYPE = char;
        re2c:yyfill:enable = 0;

        end = [\x00];
        sep = [ \t]+;
        num = '-'?[0-9]+;
        state = [A-Za-z];

        end { return; }
        sep { continue; }
        state { col++; continue; }
        '(' {
            col++;

            char *lparen = p;
            char *rparen = strrchr(p, ')');

            cp_comm(pid_stat->comm, lparen, rparen);

            // continue lexing after comm's rightmost paren.
            YYCURSOR = rparen + 1;
            continue;
        }
        num {
            col++; 
            parse_number(pid_stat, p, col);
            continue;
        }
        * { return; }
    */
    }
}
