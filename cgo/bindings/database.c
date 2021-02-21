#include "database.h"
#include "database/rrd.h"

const char *rrdhostp_hostname(RRDHOSTP host) {
    return host->hostname;
}
