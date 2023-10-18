#include "database/rdb/protos/rdbv.pb.h"
#include "rdb-private.h"

namespace pb = google::protobuf;

using rocksdb::Slice;
using rocksdb::Status;
using rocksdb::Iterator;
using rocksdb::ReadOptions;

static inline uint32_t rdb_store_metric_start_time(const rdb_collect_handle *rch);
static inline uint32_t rdb_store_metric_end_time(const rdb_collect_handle *rch);

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

    spinlock_lock(&rch->collection.lock);
    uint32_t start_time = rdb_store_metric_start_time(rch);
    spinlock_unlock(&rch->collection.lock);

    return start_time;
}

time_t rdb_metric_latest_time(STORAGE_METRIC_HANDLE *smh)
{
    uint32_t end_time = std::numeric_limits<uint32_t>::min();

    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    rdb_collect_handle *rch = rmh->rch;
    if (rch) {
        spinlock_lock(&rch->collection.lock);
        end_time = rdb_store_metric_end_time(rch);
        spinlock_unlock(&rch->collection.lock);
    }

    if (end_time == std::numeric_limits<uint32_t>::min())
    {
        const rdb::Key key{rmh->rmg->id, rmh->id + 1, 0};

        Iterator *it = SI->RDB->NewIterator(ReadOptions());
        for (it->SeekForPrev(key.slice());
             it->Valid();
             it->Next())
        {
            end_time = rdb::Key{it->key()}.pit();
            break;
        }
    }

    return end_time;
}

/*===---------------------------------------------------------------------===*/
/* Collection handles                                                        */
/*===---------------------------------------------------------------------===*/

static uint32_t rdb_store_metric_start_time(const rdb_collect_handle *rch) {
    if (!rch->collection.pit_ut)
        return std::numeric_limits<uint32_t>::max();

    uint32_t ue = rch->collection.update_every_ut / USEC_PER_SEC;
    uint32_t pit = rch->collection.pit_ut / USEC_PER_SEC;
    return pit - rch->collection.cp->duration() + ue;
}

static uint32_t rdb_store_metric_end_time(const rdb_collect_handle *rch) {
    if (!rch->collection.pit_ut)
        return std::numeric_limits<uint32_t>::min();

    return (rch->collection.pit_ut + rch->collection.update_every_ut) / USEC_PER_SEC;
}

static void rdb_store_metric_flush_internal(rdb_collect_handle *rch, bool protect)
{
    if (protect)
    {
        spinlock_lock(&rch->collection.lock);
    }

    uint32_t gid = rch->rmh->rmg->id;
    uint32_t mid = rch->rmh->id;
    uint32_t pit = rdb_store_metric_start_time(rch);

    internal_fatal(pit == 0 ||
                   pit == std::numeric_limits<uint32_t>::max(),
                   "Invalid start time: %u", pit);

    const rdb::Key key{gid, mid, pit};
    netdata_log_error("Adding key: %s (storage numbers: %zu)",
                      key.toString(true).c_str(),
                      rch->collection.cp->size());

    rdb::Page P = rch->collection.cp->page();

    // TODO: the max size should be 4096 + 6 bytes. is there
    // any performance difference if the bytes array has exact size?
    // ie. are we hitting hot vs. cold memory on serialization?
    std::array<char, 64 * 1024> bytes;

    std::optional<const Slice> OV = P.flush(bytes);
    if (!OV.has_value())
    {
        fatal("Failed to flush page...");
    }

    // TODO: make 1024 an SI constant
    rch->collection.cp->reset(1024);

    if (protect)
    {
        spinlock_unlock(&rch->collection.lock);
    }

    rocksdb::WriteOptions WO;
    WO.disableWAL = true;
    WO.sync = false;
    SI->RDB->Put(WO, key.slice(), OV.value());

    num_pages_written++;
}

static inline void rdb_store_metric_next_internal(STORAGE_COLLECT_HANDLE *sch,
                                                  STORAGE_POINT &SP,
                                                  usec_t point_in_time_ut);

[[gnu::cold]]
static void rdb_store_metric_next_internal_slow(STORAGE_COLLECT_HANDLE *sch,
                                                STORAGE_POINT &SP,
                                                usec_t point_in_time_ut,
                                                usec_t update_every_ut)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    
    spinlock_lock(&rch->collection.lock);

    // this might be the first time we are saving something in the collection handle.
    if (rch->collection.pit_ut == 0)
    {
        rch->collection.cp->appendPoint(SP);
        rch->collection.pit_ut = point_in_time_ut;
        spinlock_unlock(&rch->collection.lock);
        return;
    }

    if (rch->collection.pit_ut < point_in_time_ut)
    {
        // point_in_time is in the future
        netdata_log_error("[1] point_in_time is in the future");

        usec_t delta_ut = point_in_time_ut - rch->collection.pit_ut;

        if (delta_ut < update_every_ut)
        {
            // step is too small
            rdb_store_metric_flush_internal(rch, false);
        }
        else if (delta_ut < update_every_ut)
        {
            // step is unaligned
            rdb_store_metric_flush_internal(rch, false);
        }
        else
        {
            // aligned but in the future
            size_t points_gap = delta_ut / update_every_ut;

            if (points_gap >= rch->collection.cp->capacity())
            {
                // we can't store any points in the current page
                rdb_store_metric_flush_internal(rch, false);
            }
            else
            {
                // fill gaps in the current page
                usec_t stop_ut = point_in_time_ut - update_every_ut;

                for (usec_t this_ut = (rch->collection.pit_ut + update_every_ut);
                     this_ut <= stop_ut;
                     this_ut = (rch->collection.pit_ut + update_every_ut))
                {
                    spinlock_unlock(&rch->collection.lock);

                    STORAGE_POINT EmptySP = {
                        .min = NAN,
                        .max = NAN,
                        .sum = NAN,

                        .start_time_s = 0,
                        .end_time_s = 0,

                        .count = 1,
                        .anomaly_count = 0,

                        .flags = SN_EMPTY_SLOT,
                    };

                    rdb_store_metric_next_internal(sch, EmptySP, this_ut);
                    spinlock_lock(&rch->collection.lock);
                }
            }
        }

        spinlock_unlock(&rch->collection.lock);
        rdb_store_metric_next_internal(sch, SP, point_in_time_ut);
        return;
    }
    else if (rch->collection.pit_ut > point_in_time_ut)
    {
        netdata_log_error("[2] point_in_time is in the past");

        // point_in_time is in the past, nothing to do
        spinlock_unlock(&rch->collection.lock);
        return;
    }
    else if (rch->collection.pit_ut == point_in_time_ut)
    {
        netdata_log_error("[3] point_in_time has not progressed");

        // point_in_time has already been saved, nothing to do
        spinlock_unlock(&rch->collection.lock);
        return;
    }
    else
    {
        fatal("WTF?");
    }
}

static inline void rdb_store_metric_next_internal(STORAGE_COLLECT_HANDLE *sch,
                                                  STORAGE_POINT &SP,
                                                  usec_t point_in_time_ut)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    spinlock_lock(&rch->collection.lock);

    if (unlikely(rch->collection.cp->capacity() == 0))
    {
        rdb_store_metric_flush_internal(rch, false);
    }

    usec_t delta_ut = point_in_time_ut - rch->collection.pit_ut;
    usec_t update_every_ut = rch->collection.update_every_ut;

    if (unlikely(delta_ut != update_every_ut))
    {
        spinlock_unlock(&rch->collection.lock);
        rdb_store_metric_next_internal_slow(sch, SP, point_in_time_ut, update_every_ut);
        return;
    }

    rch->collection.cp->appendPoint(SP);

    rch->collection.pit_ut += update_every_ut;
    spinlock_unlock(&rch->collection.lock);
}

STORAGE_COLLECT_HANDLE *rdb_store_metric_init(STORAGE_METRIC_HANDLE *smh, uint32_t update_every, STORAGE_METRICS_GROUP *smg)
{
    rdb_metrics_group *rmg = reinterpret_cast<rdb_metrics_group *>(smg);
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    // link metric handle to its group
    rmh->rmg = rmg;

    // initialize a new collection handle
    rdb_collect_handle *rch = new rdb_collect_handle();

    rch->common.backend = STORAGE_ENGINE_BACKEND_RDB;
    rch->rmh = reinterpret_cast<rdb_metric_handle *>(rdb_metric_dup(smh));

    // TODO: make 1024 an SI constant
    uint32_t initial_slots = (rmg->id % 1024) + 1;

    spinlock_init(&rch->collection.lock);
    rch->collection.pit_ut = 0;
    rch->collection.update_every_ut = update_every * USEC_PER_SEC;

    std::optional<rdb::Page> OP = rdb::Page::create(*rmg->arena, rdb::PageOptions());
    if (!OP.has_value()) {
        fatal("Could not create new page for collection handle.");
    }

    rch->collection.cp = rdb::CollectionPage(OP.value(), initial_slots);
    rch->collection.cp->setUpdateEvery(update_every);

    // link collection handle to its metric
    rmh->rch = rch;

    return reinterpret_cast<STORAGE_COLLECT_HANDLE *>(rch);
}

void rdb_store_metric_next(STORAGE_COLLECT_HANDLE *sch, usec_t point_in_time_ut,
                           NETDATA_DOUBLE n, NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                           uint16_t count, uint16_t anomaly_count, SN_FLAGS flags)
{
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

    rdb_store_metric_next_internal(sch, SP, point_in_time_ut);
}

void rdb_store_metric_change_collection_frequency(STORAGE_COLLECT_HANDLE *sch, int update_every_s)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    spinlock_lock(&rch->collection.lock);

    rdb_store_metric_flush_internal(rch, false);

    rch->collection.update_every_ut = update_every_s * USEC_PER_SEC;
    rch->collection.cp->setUpdateEvery(update_every_s);

    spinlock_unlock(&rch->collection.lock);
}

void rdb_store_metric_flush(STORAGE_COLLECT_HANDLE *sch)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    rdb_store_metric_flush_internal(rch, true);
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

using RepKeyField = pb::RepeatedField<std::array<char, 12>>;

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
        const std::array<char, 12> &AR =
                *reinterpret_cast<const std::array<char, 12> *>(It->key().data());
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
    pb::RepeatedField<std::array<char, 12>> *keys;
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

    for (size_t i = 0; i != rqh->keys->size(); i++) {
        const std::array<char, 12> &ArrRef = rqh->keys->Get(i);
        uint32_t gid;
        uint32_t mid;
        uint32_t pit;
        SI->parseKey(Slice(ArrRef.data(), ArrRef.size()), gid, mid, pit);

        netdata_log_error("Retrieved key: gid=%u, mid=%u, pit=%u", gid, mid, pit);
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

    char scratch[12];

    uint32_t gid = 0;
    uint32_t mid = 0;
    uint32_t pit = 0;

    const Slice StartK = SI->keySlice(scratch, gid, mid, pit);

    uint32_t FirstPit = ~0u;

    Iterator *It = SI->RDB->NewIterator(ReadOptions());

    for (It->Seek(StartK); It->Valid(); It->Next())
    {
        const Slice &K = It->key();
        SI->parseKey(K, gid, mid, pit);
        netdata_log_error("gid=%u, mid=%u, pit=%u", gid, mid, pit);
        FirstPit = std::min(FirstPit, pit);
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

    char StartBuf[12];
    const Slice &StartK = SI->keySlice(StartBuf, 0, 0, 0);

    char LimitBuf[12];
    const Slice &LimitK = SI->keySlice(LimitBuf,
        std::numeric_limits<uint32_t>::max(),
        std::numeric_limits<uint32_t>::max(),
        std::numeric_limits<uint32_t>::max()
    );

    ranges[0].start = StartK;
    ranges[0].limit = LimitK;

    Status S = SI->RDB->GetApproximateSizes(Opts, SI->RDB->DefaultColumnFamily(), ranges.data(), ranges.size(), sizes.data());
    if (!S.ok()) {
        netdata_log_error("Could not get approximate size for default CF: %s", S.ToString().c_str());
        return 0;
    }

    return sizes[0];
}
