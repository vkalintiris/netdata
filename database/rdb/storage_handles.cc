#include "rdb-private.h"

namespace pb = google::protobuf;

using rocksdb::Slice;
using rocksdb::Status;
using rocksdb::Iterator;
using rocksdb::ReadOptions;

/*===---------------------------------------------------------------------===*/
/* Groups                                                                    */
/*===---------------------------------------------------------------------===*/

STORAGE_METRICS_GROUP *rdb_metrics_group_get(STORAGE_INSTANCE *si, uuid_t *uuid)
{
    UNUSED(si);

    rdb_metrics_group *rmg = SI->GroupsRegistry.create(*uuid);
    rmg->arena = SI->getThreadArena();

    return reinterpret_cast<STORAGE_METRICS_GROUP *>(rmg);
}

void rdb_metrics_group_release(STORAGE_INSTANCE *si, STORAGE_METRICS_GROUP *smg)
{
    UNUSED(si);

    rdb_metrics_group *rmg = reinterpret_cast<rdb_metrics_group *>(smg);
    SI->GroupsRegistry.release(rmg);
}

/*===---------------------------------------------------------------------===*/
/* Metrics                                                                   */
/*===---------------------------------------------------------------------===*/

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

bool rdb_metric_retention_by_uuid(STORAGE_INSTANCE *si, uuid_t *uuid, time_t *first_entry_s, time_t *last_entry_s)
{
    UNUSED(si);
    UNUSED(uuid);
    UNUSED(first_entry_s);
    UNUSED(last_entry_s);

    fatal("Not implemented yet.");

    return false;
}

time_t rdb_metric_oldest_time(STORAGE_METRIC_HANDLE *smh)
{
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    const rdb::Key key{rmh->rmg->id, rmh->id, 0};

    Iterator *it = SI->RDB->NewIterator(ReadOptions());
    for (it->Seek(key.slice()); it->Valid(); it->Next())
    {
        return rdb::Key{it->key()}.pit();
    }

    // FIXME: maybe it's rmh that needs the spinlock for rch
    rdb_collect_handle *rch = rmh->rch;
    if (!rch)
        return std::numeric_limits<uint32_t>::max();

    return rch->ch.after() / USEC_PER_SEC;
}

time_t rdb_metric_latest_time(STORAGE_METRIC_HANDLE *smh)
{
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    rdb_collect_handle *rch = rmh->rch;
    if (rch)
        return rch->ch.before() / USEC_PER_SEC;

    const rdb::Key key{rmh->rmg->id, rmh->id + 1, 0};

    Iterator *it = SI->RDB->NewIterator(ReadOptions());
    for (it->SeekForPrev(key.slice());
         it->Valid();
         it->Next())
    {
        // FIXME: Need to add page duration
        return rdb::Key{it->key()}.pit();
    }

    return 0;
}

/*===---------------------------------------------------------------------===*/
/* Collection handles                                                        */
/*===---------------------------------------------------------------------===*/

STORAGE_COLLECT_HANDLE *rdb_store_metric_init(STORAGE_METRIC_HANDLE *smh,
                                              uint32_t update_every,
                                              STORAGE_METRICS_GROUP *smg)
{
    rdb_metrics_group *rmg = reinterpret_cast<rdb_metrics_group *>(smg);
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    rmh->rmg = rmg;

    std::optional<rdb::CollectionHandle> CH =
        rdb::CollectionHandle::create(*rmg->arena, rdb::PageOptions(), rmg->id, rmh->id);
    if (!CH.has_value())
        fatal("Could not create collection handle");

    CH->setUpdateEvery(update_every * USEC_PER_SEC);

    rmh->rch = new rdb_collect_handle(CH.value());
    return reinterpret_cast<STORAGE_COLLECT_HANDLE *>(rmh->rch);
}

void rdb_store_metric_next(STORAGE_COLLECT_HANDLE *sch, usec_t point_in_time_ut,
                           NETDATA_DOUBLE n, NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                           uint16_t count, uint16_t anomaly_count, SN_FLAGS flags)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    STORAGE_POINT SP = {
        .min = min_value,
        .max = max_value,
        .sum = n,

        .start_time_s = 0,
        .end_time_s = 0,

        .count = count,
        .anomaly_count = anomaly_count,

        .flags = flags,
    };

    rch->ch.store_next(point_in_time_ut, SP);
}

void rdb_store_metric_flush(STORAGE_COLLECT_HANDLE *sch)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    rch->ch.flush();
}

void rdb_store_metric_change_collection_frequency(STORAGE_COLLECT_HANDLE *sch, int update_every_s)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    rch->ch.setUpdateEvery(update_every_s * USEC_PER_SEC);
}

int rdb_store_metric_finalize(STORAGE_COLLECT_HANDLE *sch)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    delete rch;
    return 0;
}

/*===---------------------------------------------------------------------===*/
/* Query ops                                                                 */
/*===---------------------------------------------------------------------===*/

using RepKeyField = pb::RepeatedField<rdb::Key>;

static RepKeyField *rdb_util_collect_keys(pb::Arena &Arena,
                                          const rdb::Key &key_start, const rdb::Key &key_end)
{
    RepKeyField *keys = pb::Arena::CreateMessage<RepKeyField>(&Arena);
    size_t size_hint = 2 * (key_end.pit() - key_start.pit()) / 1024;
    keys->Reserve(size_hint);

    Iterator *It = SI->RDB->NewIterator(ReadOptions());
    for (It->SeekForPrev(key_start.slice());
         It->Valid() && ((It->key().compare(key_end.slice()) <= 0));
         It->Next())
    {
        const std::array<char, rdb::Key::Bytes> &AR =
                *reinterpret_cast<const std::array<char, rdb::Key::Bytes> *>(It->key().data());
        keys->Add(AR);
    }

    return keys;
}

struct rdb_query_handle
{
    rdb_metric_handle *rmh;

    uint32_t after_s;
    uint32_t before_s;
    uint32_t now_s;

    pb::Arena Arena;
    pb::RepeatedField<rdb::Key> *keys;
};

void rdb_load_metric_init(STORAGE_METRIC_HANDLE *smh,
                          struct storage_engine_query_handle *seqh,
                          time_t start_time_s,
                          time_t end_time_s,
                          STORAGE_PRIORITY priority)
{
    rdb_query_handle *rqh = new rdb_query_handle();

    rqh->rmh = reinterpret_cast<rdb_metric_handle *>(rdb_metric_dup(smh));
    rqh->after_s = std::max(rdb_metric_oldest_time(smh), start_time_s);
    rqh->before_s = std::min(rdb_metric_latest_time(smh), end_time_s);
    rqh->now_s = rqh->after_s;

    rdb::Key key_start{rqh->rmh->rmg->id, rqh->rmh->id, rqh->after_s};
    rdb::Key key_end{rqh->rmh->rmg->id, rqh->rmh->id, rqh->before_s};
    rqh->keys = rdb_util_collect_keys(rqh->Arena, key_start, key_end);

    for (auto It = rqh->keys->begin(); It != rqh->keys->end(); It++) {
        const rdb::Key &K = *It;
        netdata_log_error("Found key: %s", K.toString(true).c_str());
    }

    seqh->start_time_s = rqh->after_s;
    seqh->end_time_s = rqh->before_s;
    seqh->backend = STORAGE_ENGINE_BACKEND_RDB;
    seqh->priority = priority;
    seqh->handle = reinterpret_cast<STORAGE_QUERY_HANDLE *>(rqh);
}

static bool rdb_load_metric_next_page(rdb_query_handle *rqh, uint32_t after, uint32_t before)
{
    UNUSED(rqh);
    UNUSED(after);
    UNUSED(before);

    // // check the collection handle first
    // rdb_collect_handle *rch = rqh->rmh->rch;
    // if (rch)
    // {
    //     spinlock_lock(&rch->collection.lock);

    //     uint32_t start_time_s = rdb_store_metric_start_time(rch);
    //     if (rqh->now_s >= start_time_s)
    //     {
    //         rqh->P = rch->collection.value.getPage(&rqh->Arena, start_time_s);
    //     } else {
    //         rqh->P.reset();
    //     }

    //     spinlock_unlock(&rch->collection.lock);
    // }

    // if (rqh->P.has_value())
    //     return false;


    return false;
}

STORAGE_POINT rdb_load_metric_next(struct storage_engine_query_handle *seqh)
{
    rdb_query_handle *rqh = reinterpret_cast<rdb_query_handle *>(seqh->handle);

    rdb_load_metric_next_page(rqh, seqh->start_time_s, seqh->end_time_s);

    for (size_t i = 0; i != rqh->keys->size(); i++)
    {
        const rdb::Key &K = rqh->keys->Get(i);
        netdata_log_error("Retrieved key: gid=%u, mid=%u, pit=%u",
                          K.gid(), K.mid(), K.pit());
    }

    STORAGE_POINT sp;
    storage_point_empty(sp, 10, 10);
    return sp;
}

int rdb_load_metric_is_finished(struct storage_engine_query_handle *seqh)
{
    rdb_query_handle *rqh = reinterpret_cast<rdb_query_handle *>(seqh->handle);
    return rqh->now_s > seqh->end_time_s;
}

/*===---------------------------------------------------------------------===*/
/* Storage instance                                                          */
/*===---------------------------------------------------------------------===*/

time_t rdb_global_first_time_s(STORAGE_INSTANCE *si)
{
    UNUSED(si);

    // FIXME: this will iterate _ALL_ keys.
    netdata_log_error("Expensive operation: %s()", __func__);

    const rdb::Key StartK(0, 0, 0);

    uint32_t FirstPit = ~0u;

    Iterator *It = SI->RDB->NewIterator(ReadOptions());

    for (It->Seek(StartK.slice()); It->Valid(); It->Next())
    {
        const rdb::Key K(It->key());
        netdata_log_error("gid=%u, mid=%u, pit=%u", K.gid(), K.mid(), K.pit());
        FirstPit = std::min(FirstPit, K.pit());
    }

    return FirstPit;
}

uint64_t rdb_disk_space_used(STORAGE_INSTANCE *si)
{
    UNUSED(si);

    std::array<rocksdb::Range, 1> ranges;
    std::array<uint64_t, 1> sizes;
    rocksdb::SizeApproximationOptions Opts;

    Opts.include_memtables = false;
    Opts.files_size_error_margin = 0.1;

    ranges[0].start = rdb::Key::min().slice();
    ranges[0].limit = rdb::Key::max().slice();

    Status S = SI->RDB->GetApproximateSizes(Opts, SI->RDB->DefaultColumnFamily(), ranges.data(), ranges.size(), sizes.data());
    if (!S.ok()) {
        netdata_log_error("Could not get approximate size for default CF: %s", S.ToString().c_str());
        return 0;
    }

    return sizes[0];
}
