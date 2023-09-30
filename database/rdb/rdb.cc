#include "database/rrd.h"
#include "rdb-private.h"
#include "libnetdata/xxhash.h"

static inline size_t hash_uuid(const uuid_t *uuid) {
    return XXH32(*uuid, UUID_SZ, 0);
}

/*
 * STORAGE_METRIC_HANDLE
*/

static struct rdb_metrics metrics;

static STORAGE_METRIC_HANDLE *rdb_metric_create(STORAGE_INSTANCE *si, uuid_t *uuid)
{
    UNUSED(si);

    struct rdb_metric_handle *rmh = new rdb_metric_handle();
    uuid_copy(rmh->uuid, *uuid);
    rmh->rc = 0;

    {
        std::lock_guard<std::mutex> L(metrics.mutex);

        rmh->id = metrics.max_id++;
        metrics.map[hash_uuid(uuid)] = rmh;
    }

    return (STORAGE_METRIC_HANDLE *) rmh;
}

STORAGE_METRIC_HANDLE *rdb_metric_get(STORAGE_INSTANCE *si, uuid_t *uuid)
{
    UNUSED(si);
    
    std::lock_guard<std::mutex> L(metrics.mutex);

    auto it = metrics.map.find(hash_uuid(uuid));
    if (it == metrics.map.end())
        return nullptr;

    it->second->rc++;
    return (STORAGE_METRIC_HANDLE *) it->second;
}

STORAGE_METRIC_HANDLE *rdb_metric_get_or_create(RRDDIM *rd, STORAGE_INSTANCE *si)
{
    STORAGE_METRIC_HANDLE *smh = rdb_metric_get(si, &rd->metric_uuid);
    if (!smh)
        smh = rdb_metric_create(si, &rd->metric_uuid);

    return smh;
}

STORAGE_METRIC_HANDLE *rdb_metric_dup(STORAGE_METRIC_HANDLE *smh)
{
    METRIC *metric = (METRIC *) smh;
    return (STORAGE_METRIC_HANDLE *) mrg_metric_dup(main_mrg, metric);
}

void rdb_metric_release(STORAGE_METRIC_HANDLE *smh)
{
    METRIC *metric = (METRIC *) smh;
    mrg_metric_release(main_mrg, metric);
}

bool rdb_metric_retention_by_uuid(STORAGE_INSTANCE *si, uuid_t *uuid, time_t *first_entry_s, time_t *last_entry_s) {
    UNUSED(si);
    UNUSED(uuid);
    UNUSED(first_entry_s);
    UNUSED(last_entry_s);

    fatal("Not implemented yet.");

    return false;
}

/*
 * STORAGE_METRICS_GROUP
*/

STORAGE_METRICS_GROUP *rdb_metrics_group_get(STORAGE_INSTANCE *si, uuid_t *uuid) {
    UNUSED(si);
    UNUSED(uuid);

    rdb_metrics_group *rmg = new rdb_metrics_group();
    rmg->rc = 0;
    return (STORAGE_METRICS_GROUP *) rmg;
}

void rdb_metrics_group_release(STORAGE_INSTANCE *si, STORAGE_METRICS_GROUP *smg) {
    UNUSED(si);

    rdb_metrics_group *rmg = (rdb_metrics_group *) smg;
    if(__atomic_sub_fetch(&rmg->rc, 1, __ATOMIC_SEQ_CST) == 0)
        delete rmg;
}

/*
 * STORAGE_COLLECT_HANDLE
*/

STORAGE_COLLECT_HANDLE *rdb_store_metric_init(STORAGE_METRIC_HANDLE *smh, uint32_t update_every, STORAGE_METRICS_GROUP *smg)
{
    UNUSED(smh);
    UNUSED(update_every);
    UNUSED(smg);

    return nullptr;
}

void rdb_store_metric_next(STORAGE_COLLECT_HANDLE *sch, usec_t point_in_time,
                           NETDATA_DOUBLE n, NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                           uint16_t count, uint16_t anomaly_count, SN_FLAGS flags)
{
    UNUSED(sch);
    UNUSED(point_in_time);
    UNUSED(n);
    UNUSED(min_value);
    UNUSED(max_value);
    UNUSED(count);
    UNUSED(anomaly_count);
    UNUSED(flags);
}

void rdb_store_metric_flush(STORAGE_COLLECT_HANDLE *sch) {
    UNUSED(sch);
}

int rdb_store_metric_finalize(STORAGE_COLLECT_HANDLE *sch) {
    UNUSED(sch);

    return 0;
}
