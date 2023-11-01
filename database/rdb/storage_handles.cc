#include "database/rdb/Key.h"
#include "libnetdata/clocks/clocks.h"
#include "rdb-private.h"

namespace pb = google::protobuf;

using rocksdb::Status;
using rocksdb::Iterator;
using rocksdb::ReadOptions;

/*===---------------------------------------------------------------------===*/
/* Groups                                                                    */
/*===---------------------------------------------------------------------===*/

STORAGE_METRICS_GROUP *rdb_metrics_group_get(STORAGE_INSTANCE *si, uuid_t *uuid)
{
    UNUSED(si);

    global_statistics_metrics_group_get();

    rdb_metrics_group *rmg = SI->GroupsRegistry.create(*uuid);
    rmg->arena = SI->getThreadArena();

    return reinterpret_cast<STORAGE_METRICS_GROUP *>(rmg);
}

void rdb_metrics_group_release(STORAGE_INSTANCE *si, STORAGE_METRICS_GROUP *smg)
{
    UNUSED(si);

    global_statistics_metrics_group_release();

    rdb_metrics_group *rmg = reinterpret_cast<rdb_metrics_group *>(smg);
    SI->GroupsRegistry.release(rmg);
}

/*===---------------------------------------------------------------------===*/
/* Metrics                                                                   */
/*===---------------------------------------------------------------------===*/

STORAGE_METRIC_HANDLE *rdb_metric_get(STORAGE_INSTANCE *si, uuid_t *uuid)
{
    UNUSED(si);

    global_statistics_metric_get();

    rdb_metric_handle *rmh = SI->MetricsRegistry.acquire(*uuid);
    return reinterpret_cast<STORAGE_METRIC_HANDLE *>(rmh);
}

STORAGE_METRIC_HANDLE *rdb_metric_get_or_create(RRDDIM *rd, STORAGE_INSTANCE *si)
{
    UNUSED(si);

    global_statistics_metric_get_or_create();

    rdb_metric_handle *rmh = SI->MetricsRegistry.add_or_create(rd->metric_uuid);
    return reinterpret_cast<STORAGE_METRIC_HANDLE *>(rmh);
}

STORAGE_METRIC_HANDLE *rdb_metric_dup(STORAGE_METRIC_HANDLE *smh)
{
    global_statistics_metric_dup();

    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);
    SI->MetricsRegistry.acquire(rmh);
    return smh;
}

void rdb_metric_release(STORAGE_METRIC_HANDLE *smh)
{
    global_statistics_metric_release();

    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);
    SI->MetricsRegistry.release(rmh);
}

bool rdb_metric_retention_by_uuid(STORAGE_INSTANCE *si, uuid_t *uuid, time_t *first_entry_s, time_t *last_entry_s)
{
    global_statistics_metric_retention_by_uuid();

    UNUSED(si);
    UNUSED(uuid);
    UNUSED(first_entry_s);
    UNUSED(last_entry_s);

    fatal("Not implemented yet.");

    return false;
}

#if 0
time_t rdb_metric_oldest_time(STORAGE_METRIC_HANDLE *smh)
{
    global_statistics_metric_oldest_time();

    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    uint32_t PIT = 0;
    const rdb::Key key{rmh->rmg->id, rmh->id, 0};

    Iterator *It = SI->getIteratorMD(ReadOptions());
    It->Seek(key.slice());
    if (It->Valid())
    {
        rdb::Key K = It->key();
        if (K.mid() == rmh->id)
            PIT = K.pit();
    }
    delete It;

    if (PIT)
        return PIT;
    
    // FIXME: maybe it's rmh that needs the spinlock for rch
    rdb_collect_handle *rch = rmh->rch;
    if (!rch)
        return PIT;

    return rch->ch.after() / USEC_PER_SEC;
}
#else
time_t rdb_metric_oldest_time(STORAGE_METRIC_HANDLE *smh)
{
    global_statistics_metric_oldest_time();

    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);
    rdb_collect_handle *rch = rmh->rch;

    if (!rch)
        return 0;


    return rch->ch.oldestTime();
}
#endif

time_t rdb_metric_latest_time(STORAGE_METRIC_HANDLE *smh)
{
    global_statistics_metric_latest_time();

    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    rdb_collect_handle *rch = rmh->rch;
    if (rch)
        return rch->ch.before() / USEC_PER_SEC;

    const rdb::Key key{rmh->rmg->id, rmh->id + 1, 0};

    Iterator *it = SI->getIteratorMD(ReadOptions());
    for (it->SeekForPrev(key.slice());
         it->Valid();
         it->Next())
    {
        // FIXME: Need to add page duration
        uint32_t PIT = rdb::Key{it->key()}.pit();
        delete it;
        return PIT;
    }

    delete it;
    return 0;
}

/*===---------------------------------------------------------------------===*/
/* Collection handles                                                        */
/*===---------------------------------------------------------------------===*/

STORAGE_COLLECT_HANDLE *rdb_store_metric_init(STORAGE_METRIC_HANDLE *smh,
                                              uint32_t update_every,
                                              STORAGE_METRICS_GROUP *smg)
{
    global_statistics_store_metric_init();

    rdb_metrics_group *rmg = reinterpret_cast<rdb_metrics_group *>(smg);
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    rmh->rmg = rmg;

    rdb::PageOptions PO = rdb::PageOptions();
    PO.initial_slots = (rmh->id % PO.capacity) + 1;
    std::optional<rdb::CollectionHandle> CH =
        rdb::CollectionHandle::create(*rmg->arena, PO, rmg->id, rmh->id);
    if (!CH.has_value())
        fatal("Could not create collection handle");

    CH->setUpdateEvery(update_every * USEC_PER_SEC);

    global_statistics_rdb_collection_handles_incr();

    rmh->rch = new rdb_collect_handle(CH.value());
    return reinterpret_cast<STORAGE_COLLECT_HANDLE *>(rmh->rch);
}

void rdb_store_metric_next(STORAGE_COLLECT_HANDLE *sch, usec_t point_in_time_ut,
                           NETDATA_DOUBLE n, NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                           uint16_t count, uint16_t anomaly_count, SN_FLAGS flags)
{
    global_statistics_store_metric_next();

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
    global_statistics_store_metric_flush();

    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    rch->ch.flush();
}

void rdb_store_metric_change_collection_frequency(STORAGE_COLLECT_HANDLE *sch, int update_every_s)
{
    global_statistics_store_metric_change_collection_frequency();

    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    rch->ch.setUpdateEvery(update_every_s * USEC_PER_SEC);
}

int rdb_store_metric_finalize(STORAGE_COLLECT_HANDLE *sch)
{
    global_statistics_store_metric_finalize();

    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    rch->ch.flush();
    delete rch;
    global_statistics_rdb_collection_handles_decr();
    return 0;
}

/*===---------------------------------------------------------------------===*/
/* Query ops                                                                 */
/*===---------------------------------------------------------------------===*/

struct rdb_query_handle
{
    rdb_metric_handle *rmh;

    pb::Arena Arena;
    Iterator *It;

    rdb::Key AfterK;
    uint32_t Before;
    uint32_t Now;
    rdb::UniversalQuery UQ;

    rdb_query_handle(rdb_metric_handle *rmh,
                     rdb::CollectionHandle *CH,
                     const rdb::Key &AfterK, uint32_t Before) :
        rmh(rmh), Arena(), It(nullptr), AfterK(AfterK),
        Before(Before), Now(AfterK.pit()),
        UQ(CH, AfterK)
    {
        seek();
    }

    void seek()
    {
        if (!rmh->rch) {
            if (rmh->rch->ch.after() >= (AfterK.pit() / USEC_PER_SEC))
                return;
        }
        
        It = SI->getIteratorMD(rocksdb::ReadOptions());
        if (!It)
            fatal("Could not get new allocator from RocksDB");

        It->SeekForPrev(AfterK.slice());
        if (!It->Valid())
            It->Seek(AfterK.slice());

        if (!It->Valid())
            Now = Before;
    }

    inline STORAGE_POINT next()
    {
        STORAGE_POINT SP = UQ.next();
        Now = SP.start_time_s;
        return SP;
    }

    inline bool isFinished()
    {
        return (Now > Before) ? true : UQ.isFinished(Arena, *It);
    }

    ~rdb_query_handle()
    {
        UQ.finalize();

        if (It)
            delete It;

        rdb_metric_release(reinterpret_cast<STORAGE_METRIC_HANDLE *>(rmh));
    }
};

void rdb_load_metric_init(STORAGE_METRIC_HANDLE *smh,
                          struct storage_engine_query_handle *seqh,
                          time_t After,
                          time_t Before,
                          STORAGE_PRIORITY priority)
{
    global_statistics_load_metric_init();

    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(rdb_metric_dup(smh));

    After = std::max(rdb_metric_oldest_time(smh), After);
    Before = std::min(rdb_metric_latest_time(smh), Before);

    rdb::Key StartK(rmh->rmg->id, rmh->id, After);

    rdb_query_handle *rqh = new rdb_query_handle(rmh, &rmh->rch->ch, StartK, Before);

    seqh->start_time_s = After;
    seqh->end_time_s = Before;
    seqh->backend = STORAGE_ENGINE_BACKEND_RDB;
    seqh->priority = priority;
    seqh->handle = reinterpret_cast<STORAGE_QUERY_HANDLE *>(rqh);
}

STORAGE_POINT rdb_load_metric_next(struct storage_engine_query_handle *seqh)
{
    global_statistics_load_metric_next();

    rdb_query_handle *rqh = reinterpret_cast<rdb_query_handle *>(seqh->handle);
    return rqh->UQ.next();
}

int rdb_load_metric_is_finished(struct storage_engine_query_handle *seqh)
{
    global_statistics_load_metric_is_finished();

    rdb_query_handle *rqh = reinterpret_cast<rdb_query_handle *>(seqh->handle);
    return rqh->isFinished();
}

void rdb_load_metric_finalize(struct storage_engine_query_handle *seqh) {
    global_statistics_load_metric_finalize();

    rdb_query_handle *rqh = reinterpret_cast<rdb_query_handle *>(seqh->handle);
    delete rqh;
}

/*===---------------------------------------------------------------------===*/
/* Storage instance                                                          */
/*===---------------------------------------------------------------------===*/

time_t rdb_global_first_time_s(STORAGE_INSTANCE *si)
{
    global_statistics_global_first_time();

    UNUSED(si);

    Iterator *It = SI->getIteratorMD(ReadOptions());

    It->SeekToFirst();
    if (!It->Valid()) {
        // We probably haven't written anything yet. Should we consult the
        // collection handles?
        return 0;
    }

    rdb::Key K(It->key());

    delete It;
    return static_cast<time_t>(K.pit());
}

uint64_t rdb_disk_space_used(STORAGE_INSTANCE *si)
{
    global_statistics_disk_space_used();

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
