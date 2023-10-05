#include "si.h"
#include <rocksdb/db.h>

using rocksdb::Slice;
using rocksdb::Iterator;
using rocksdb::ReadOptions;

STORAGE_METRIC_HANDLE *rdb_metric_get(STORAGE_INSTANCE *si, uuid_t *uuid)
{
    UNUSED(si);

    rdb_metric_handle *rmh = SI->MetricsRegistry.acquire(*uuid);
    return reinterpret_cast<STORAGE_METRIC_HANDLE *>(rmh);
}

STORAGE_METRIC_HANDLE *rdb_metric_get_or_create(RRDDIM *rd, STORAGE_INSTANCE *si)
{
    UNUSED(si);

    rdb_metric_handle *rmh = SI->MetricsRegistry.add_or_create(rd->metric_uuid);
    return reinterpret_cast<STORAGE_METRIC_HANDLE *>(rmh);
}

STORAGE_METRIC_HANDLE *rdb_metric_dup(STORAGE_METRIC_HANDLE *smh)
{
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);
    SI->MetricsRegistry.acquire(rmh);
    return smh;
}

void rdb_metric_release(STORAGE_METRIC_HANDLE *smh)
{
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);
    SI->MetricsRegistry.release(rmh);
}

bool rdb_metric_retention_by_uuid(STORAGE_INSTANCE *si, uuid_t *uuid, time_t *first_entry_s, time_t *last_entry_s) {
    UNUSED(si);
    UNUSED(uuid);
    UNUSED(first_entry_s);
    UNUSED(last_entry_s);

    fatal("Not implemented yet.");

    return false;
}

time_t rdb_metric_oldest_time(STORAGE_METRIC_HANDLE *smh) {
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    char scratch[12];

    uint32_t gid = rmh->rmg->id;
    uint32_t mid = rmh->id;
    uint32_t pit = 0;
    
    const Slice StartK = rdb_collection_key_serialize(scratch, gid, mid, pit);

    Iterator *it = SI->RDB->NewIterator(ReadOptions());
    for (it->Seek(StartK); it->Valid(); it->Next()) {
        const Slice &K = it->key();

        rdb_collection_key_deserialize(K, gid, mid, pit);
        return pit;
    }

    return 0;
}

time_t rdb_metric_latest_time(STORAGE_METRIC_HANDLE *smh) {
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    char scratch[12];

    uint32_t gid = rmh->rmg->id;
    uint32_t mid = rmh->id + 1;
    uint32_t pit = 0;
    
    const Slice StartK = rdb_collection_key_serialize(scratch, gid, mid, pit);

    Iterator *it = SI->RDB->NewIterator(ReadOptions());
    for (it->SeekForPrev(StartK); it->Valid(); it->Next()) {
        const Slice &K = it->key();

        rdb_collection_key_deserialize(K, gid, mid, pit);
        return pit;
    }

    return 0;
}
#include "si.h"
#include <google/protobuf/arena.h>

static class UuidShard<rdb_metrics_group> GroupsRegistry(24);

STORAGE_METRICS_GROUP *rdb_metrics_group_get(STORAGE_INSTANCE *si, uuid_t *uuid) {
    UNUSED(si);

    rdb_metrics_group *rmg = SI->GroupsRegistry.create(*uuid);
    rmg->arena = SI->getThreadArena();

    return reinterpret_cast<STORAGE_METRICS_GROUP *>(rmg);
}

void rdb_metrics_group_release(STORAGE_INSTANCE *si, STORAGE_METRICS_GROUP *smg) {
    UNUSED(si);

    rdb_metrics_group *rmg = reinterpret_cast<rdb_metrics_group *>(smg);
    SI->GroupsRegistry.release(rmg);
}
