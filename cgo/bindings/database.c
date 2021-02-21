#include "database.h"
#include "database/rrd.h"

void rrdhostp_rdlock(RRDHOSTP host) {
    rrdhost_rdlock(localhost);
}

void rrdhostp_unlock(RRDHOSTP host) {
    rrdhost_unlock(localhost);
}

const char *rrdhostp_hostname(RRDHOSTP host) {
    return host->hostname;
}

RRDSETP rrdhostp_root_set(RRDHOSTP host) {
    return host->rrdset_root;
}

RRDSETP rrdsetp_next_set(RRDSETP set) {
    return set->next;
}

const char *rrdsetp_name(RRDSETP set) {
    return set->name ? set->name : "";
}
