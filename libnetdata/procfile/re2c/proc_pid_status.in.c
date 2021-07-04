#include "proc_pid_status.h"

char *parse_effective_id(char *buf, uint64_t *id) {
    char *YYCURSOR = buf;
    char *YYMARKER;

    char *tag;
    /*!stags:re2c format = 'const char *@@;'; */

/*!re2c
    re2c:define:YYCTYPE = char;
    re2c:yyfill:enable = 0;
    re2c:flags:tags = 1;

    end = [\x00];
    num = [0-9]+;
    id = [1-9][0-9]*;
    sep = [ \t]+;

    sep id sep @tag id sep id sep id {
        *id = str2uint64_t(tag);
        return YYCURSOR;
    }
    * { return YYCURSOR; }
*/
}

char *parse_size(char *buf, uint64_t *size) {
    char *YYCURSOR = buf;
    char *YYMARKER;

    char *tag;
    /*!stags:re2c format = 'const char *@@;'; */

/*!re2c
    sep @tag num sep "kB" {
        *size = str2uint64_t(tag);
        return YYCURSOR;
    }
    * { return YYCURSOR; }
*/
}

void proc_pid_status(char *buf, proc_pid_status_t *pid_status) {
    char *YYCURSOR = buf;
    char *YYMARKER;

    for (int needed = 0; needed != 7;) {
    /*!re2c
        * { continue; }
        end { return; }
        sep { continue; }
        "Uid:" {
            YYCURSOR = parse_effective_id(YYCURSOR, &pid_status->euid);
            needed++;
            continue;
        }
        "Gid:" {
            YYCURSOR = parse_effective_id(YYCURSOR, &pid_status->egid);
            needed++;
            continue;
        }
        "VmSize:" {
            YYCURSOR = parse_size(YYCURSOR, &pid_status->vm_size);
            needed++;
            continue;
        }
        "VmRSS:" {
            YYCURSOR = parse_size(YYCURSOR, &pid_status->vm_rss);
            needed++;
            continue;
        }
        "RssFile:" {
            YYCURSOR = parse_size(YYCURSOR, &pid_status->rss_file);
            needed++;
            continue;
        }
        "RssShmem:" {
            YYCURSOR = parse_size(YYCURSOR, &pid_status->rss_shmem);
            needed++;
            continue;
        }
        "VmSwap:" {
            YYCURSOR = parse_size(YYCURSOR, &pid_status->vm_swap);
            needed++;
            continue;
        }
    */
    }
}
