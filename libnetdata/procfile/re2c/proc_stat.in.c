#include "proc_stat.h"

void proc_stat(char *buf, proc_stat_t *pstat) {
    char *YYCURSOR = buf;
    char *YYMARKER;

    int col = 0;

    for (;;) {
        char *p = YYCURSOR;

    /*!re2c
        re2c:define:YYCTYPE = char;
        re2c:yyfill:enable = 0;

        end = [\x00];
        num = [0-9]+;
        sep = [ \t]+;

        end         { return; }
        sep         { continue; }
        "cpu" sep   { col = 1; continue; }
        num {
            uint32_t v = str2uint32_t(p);
            
            switch (col) {
            case 1:
                pstat->user = v;
                break;
            case 2:
                pstat->nice = v;
                break;               
            case 3:
                pstat->system = v;
                break;               
            case 4:
                pstat->idle = v;
                break;               
            case 5:
                pstat->iowait = v;
                break;               
            case 6:
                pstat->irq = v;
                break;               
            case 7:
                pstat->softirq = v;
                break;               
            case 8:
                pstat->steal = v;
                break;               
            case 9:
                pstat->guest = v;
                break;               
            case 10:
                pstat->guest_nice = v;
                return;
            default:
                continue;
            }

            col += 1;
            continue;
        }
        * { col = 0; continue; }
    */
    }
}
