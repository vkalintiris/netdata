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
        0, /* options */
        NULL, /* dimensions */
        NULL /* context params */
    );

    if (!res)
        return NULL;

    long num_empty_values = 0;

    for (long dim = 0; dim != res->d; dim++) {
        bool is_hidden = res->od[dim] & RRDR_DIMENSION_HIDDEN;

        for (long row = 0; row != res->rows; row++) {
            long idx = (row * res->d) + dim;

            if (is_hidden) {
                res->v[idx] = 0.0L;
            } else if (res->o[idx] && RRDR_VALUE_EMPTY) {
                num_empty_values++;
            }
        }
    }

    info("%ld empty values in %s", num_empty_values, set->name ? set->name : "unknown");
    if (num_empty_values) {
        rrdr_free(res);
        return NULL;
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
