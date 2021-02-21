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

int rrdsetp_update_every(RRDSETP set) {
    return set->update_every;
}

int rrdsetp_num_dims(RRDSETP set) {
    int cntr = 0;

    for (RRDDIM *dim = set->dimensions; dim; dim = dim->next)
        cntr++;

    return cntr;
}

void rrdsetp_rdlock(RRDSETP set) {
    rrdset_rdlock(set);
}

void rrdsetp_unlock(RRDSETP set) {
    rrdset_unlock(set);
}
