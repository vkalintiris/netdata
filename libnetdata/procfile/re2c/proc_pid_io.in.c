#include "proc_pid_io.h"

enum token_t {
    TOKEN_RCHAR,
    TOKEN_WCHAR,
    TOKEN_SYSCR,
    TOKEN_SYSCW,
    TOKEN_READ_BYTES,
    TOKEN_WRITE_BYTES,
    TOKEN_CANCELLED_WRITE_BYTES,
    TOKEN_INVALID
};

void proc_pid_io(char *buf, proc_pid_io_t *pid_io) {
    char *YYCURSOR = buf;
    char *YYMARKER;

    enum token_t tok = TOKEN_INVALID;

    for (;;) {
        char *p = YYCURSOR;

    /*!re2c
        re2c:define:YYCTYPE = char;
        re2c:yyfill:enable = 0;

        end = [\x00];
        num = [0-9]+;
        sep = [: \n]+;

        end                     { return; }
        sep                     { continue; }
        "rchar"                 { tok = TOKEN_RCHAR; continue; }
        "wchar"                 { tok = TOKEN_WCHAR; continue; }
        "syscr"                 { tok = TOKEN_SYSCR; continue; }
        "syscw"                 { tok = TOKEN_SYSCW; continue; }
        "read_bytes"            { tok = TOKEN_READ_BYTES; continue; }
        "write_bytes"           { tok = TOKEN_WRITE_BYTES; continue; }
        "cancelled_write_bytes" { tok = TOKEN_CANCELLED_WRITE_BYTES; continue; }
        num {
            switch (tok) {
            case TOKEN_RCHAR:
                pid_io->rchar  = str2uint32_t(p);
                break;
            case TOKEN_WCHAR:
                pid_io->wchar  = str2uint32_t(p);
                break;
            case TOKEN_SYSCR:
                pid_io->syscr  = str2uint32_t(p);
                break;
            case TOKEN_SYSCW:
                pid_io->syscw  = str2uint32_t(p);
                break;
            case TOKEN_READ_BYTES:
                pid_io->read_bytes  = str2uint32_t(p);
                break;
            case TOKEN_WRITE_BYTES:
                pid_io->write_bytes  = str2uint32_t(p);
                break;
            case TOKEN_CANCELLED_WRITE_BYTES:
                pid_io->cancelled_write_bytes  = str2uint32_t(p);
                break;
            case TOKEN_INVALID:
                break;
            }

            tok = TOKEN_INVALID;
            continue;
        }
        * {
            tok = TOKEN_INVALID;
            continue;
        }
    */
    }
}
