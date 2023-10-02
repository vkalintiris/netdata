#include "rdb-private.h"
#include "uuid_shard.h"

static class UuidShard<rdb_metric_handle> MetricsRegistry(24);

STORAGE_METRIC_HANDLE *rdb_metric_get(STORAGE_INSTANCE *si, uuid_t *uuid)
{
    UNUSED(si);

    rdb_metric_handle *rmh = MetricsRegistry.acquire(*uuid);
    return reinterpret_cast<STORAGE_METRIC_HANDLE *>(rmh);
}

STORAGE_METRIC_HANDLE *rdb_metric_get_or_create(RRDDIM *rd, STORAGE_INSTANCE *si)
{
    UNUSED(si);

    rdb_metric_handle *rmh = MetricsRegistry.add_or_create(rd->metric_uuid);
    return reinterpret_cast<STORAGE_METRIC_HANDLE *>(rmh);
}

STORAGE_METRIC_HANDLE *rdb_metric_dup(STORAGE_METRIC_HANDLE *smh)
{
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);
    MetricsRegistry.acquire(rmh);
    return smh;
}

void rdb_metric_release(STORAGE_METRIC_HANDLE *smh)
{
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);
    MetricsRegistry.release(rmh);
}

bool rdb_metric_retention_by_uuid(STORAGE_INSTANCE *si, uuid_t *uuid, time_t *first_entry_s, time_t *last_entry_s) {
    UNUSED(si);
    UNUSED(uuid);
    UNUSED(first_entry_s);
    UNUSED(last_entry_s);

    fatal("Not implemented yet.");

    return false;
}
