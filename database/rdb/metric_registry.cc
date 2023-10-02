#include "si.h"

STORAGE_METRIC_HANDLE *rdb_metric_get(STORAGE_INSTANCE *si, uuid_t *uuid)
{
    UNUSED(si);

    rdb_metric_handle *rmh = SI.MetricsRegistry.acquire(*uuid);
    return reinterpret_cast<STORAGE_METRIC_HANDLE *>(rmh);
}

STORAGE_METRIC_HANDLE *rdb_metric_get_or_create(RRDDIM *rd, STORAGE_INSTANCE *si)
{
    UNUSED(si);

    rdb_metric_handle *rmh = SI.MetricsRegistry.add_or_create(rd->metric_uuid);
    return reinterpret_cast<STORAGE_METRIC_HANDLE *>(rmh);
}

STORAGE_METRIC_HANDLE *rdb_metric_dup(STORAGE_METRIC_HANDLE *smh)
{
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);
    SI.MetricsRegistry.acquire(rmh);
    return smh;
}

void rdb_metric_release(STORAGE_METRIC_HANDLE *smh)
{
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);
    SI.MetricsRegistry.release(rmh);
}

bool rdb_metric_retention_by_uuid(STORAGE_INSTANCE *si, uuid_t *uuid, time_t *first_entry_s, time_t *last_entry_s) {
    UNUSED(si);
    UNUSED(uuid);
    UNUSED(first_entry_s);
    UNUSED(last_entry_s);

    fatal("Not implemented yet.");

    return false;
}
