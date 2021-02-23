#include "cgo-rrd.h"
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

long long cfg_get_number(const char *section, const char *name, long long value) {
    return config_get_number(section, name, value);
}

RRDRP rrdrp_get(RRDSETP set, int num_samples) {
    time_t time_before = now_realtime_sec() - 1;
    time_t time_after = time_before - num_samples;

    RRDRP res = rrd2rrdr(
        set,
        0, /* points requested */
        time_after, /* after */
        time_before, /* before */
        RRDR_GROUPING_AVERAGE, /* grouping method */
        0, /* resampling time */
        0, /* grouping options */
        NULL, /* dimensions */
        NULL /* context params */
    );

    if (!res)
        return NULL;

    for (long i = 0; i != res->rows; i++) {
        calculated_number *cn = &res->v[res->d * i];
        RRDR_VALUE_FLAGS *vf = &res->o[res->d * i];

        for (long j = 0; j != res->d; j++) {
            if (vf[j] && RRDR_VALUE_EMPTY) {
                rrdr_free(res);
                return NULL;
            }
        }
    }

    return res;
}

long rrdrp_num_rows(RRDRP res) {
    return res->rows;
}

void rrdrp_free(RRDRP res) {
    rrdr_free(res);
}

RRDSETP rrdsetp_create(
        const char *type, const char *id, const char *name, const char *family,
        const char *context, const char *title, const char *units,
        const char *plugin, const char *module,
        long priority, int update_every) {
    return rrdset_create_localhost(
        type, id, name, family,
        context, title, units, plugin,
        module, priority, update_every, RRDSET_TYPE_AREA);
}

RRDDIMP rrdsetp_add_dim(RRDSETP st, const char *id, const char *name) {
    rrddim_add(st, id, NULL,  1, 1, RRD_ALGORITHM_ABSOLUTE);
}
