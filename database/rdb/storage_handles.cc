#include "database/rdb/protos/rdbv.pb.h"
#include "database/rdb/rdb-private.h"
#include "libnetdata/locks/locks.h"
#include "rdb.h"
#include "si.h"
#include <google/protobuf/arena.h>
#include <limits>

namespace pb = google::protobuf;

using rocksdb::Slice;
using rocksdb::Status;
using rocksdb::Iterator;
using rocksdb::ReadOptions;

using rdbv::RdbValue;
using rdbv::StorageNumbersPage;

static uint32_t rdb_store_metric_start_time(rdb_collect_handle *rch);
static uint32_t rdb_store_metric_end_time(rdb_collect_handle *rch);

/*===---------------------------------------------------------------------===*/
/* ValueWrapper                                                                    */
/*===---------------------------------------------------------------------===*/

static const char *page_case_string(const RdbValue::PageCase &PC)
{
    switch (PC) {
        case RdbValue::PageCase::kStorageNumbersPage:
            return "StorageNumbersPage";
        default:
            return "UknownPage";
    }
}

ValueWrapper ValueWrapper::create(RdbValue::PageCase PC, pb::Arena *Arena, uint32_t Slots, uint32_t UpdateEvery)
{
    RdbValue *Value = pb::Arena::CreateMessage<rdbv::RdbValue>(Arena);

    switch (PC)
    {
        case RdbValue::PageCase::kStorageNumbersPage:
        {
            StorageNumbersPage *SNP = Value->mutable_storage_numbers_page();

            // Make 1024 an SI constant;
            SNP->mutable_storage_numbers()->Reserve(1024);
            SNP->set_update_every(UpdateEvery);
            break;
        }
        default:
            fatal("Unknown page case: %s", page_case_string(PC));
    }

    ValueWrapper VW;
    VW.Value = Value;
    VW.Slots = Slots;
    return VW;
}

inline bool ValueWrapper::appendPoint(usec_t point_in_time_ut, NETDATA_DOUBLE n,
                                      NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                                      uint16_t count, uint16_t anomaly_count, SN_FLAGS flags)
{
    UNUSED(point_in_time_ut);
    UNUSED(min_value);
    UNUSED(max_value);
    UNUSED(count);
    UNUSED(anomaly_count);

    switch (Value->Page_case())
    {
        case RdbValue::PageCase::kStorageNumbersPage:
        {
            StorageNumbersPage *SNP = Value->mutable_storage_numbers_page();
            pb::RepeatedField<uint32_t> *SNs = SNP->mutable_storage_numbers();

            storage_number *SN = SNs->AddAlreadyReserved();
            *SN = pack_storage_number(n, flags);
            Slots--;
            break;
        }
        default:
            fatal("Unknown page case: %s", page_case_string(Value->Page_case()));
    }

    return true;
}

const Slice ValueWrapper::flush(char *buffer, size_t n) const
{
    size_t nbytes = Value->ByteSizeLong();
    Value->SerializeToArray(buffer, n);
    return rocksdb::Slice(buffer, nbytes);
}

void ValueWrapper::reset(uint32_t Slots)
{
    switch (Value->Page_case())
    {
        case RdbValue::PageCase::kStorageNumbersPage:
        {
            StorageNumbersPage *SNP = Value->mutable_storage_numbers_page();
            pb::RepeatedField<uint32_t> *SNs = SNP->mutable_storage_numbers();

            SNs->Clear();
            break;
        }
        default:
            fatal("Unknown page case: %s", page_case_string(Value->Page_case()));
    }

    this->Slots = Slots;
}

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

    uint32_t gid = rmh->rmg->id;
    uint32_t mid = rmh->id;
    uint32_t pit = 0;

    char scratch[12];
    const Slice StartK = SI->keySlice(scratch, gid, mid, pit);

    Iterator *It = SI->RDB->NewIterator(ReadOptions());
    for (It->Seek(StartK); It->Valid(); It->Next()) {
        const Slice &K = It->key();

        SI->parseKey(K, gid, mid, pit);
        return pit;
    }

    // FIXME: maybe it's rmh that needs the spinlock for rch
    rdb_collect_handle *rch = rmh->rch;
    if (!rch)
        return 0;

    spinlock_lock(&rch->collection.lock);
    uint32_t start_time = rdb_store_metric_start_time(rch);
    spinlock_unlock(&rch->collection.lock);

    return start_time;
}

time_t rdb_metric_latest_time(STORAGE_METRIC_HANDLE *smh)
{
    uint32_t end_time = 0;

    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    rdb_collect_handle *rch = rmh->rch;
    if (rch) {
        spinlock_lock(&rch->collection.lock);
        end_time = rdb_store_metric_end_time(rch);
        spinlock_unlock(&rch->collection.lock);
    }

    if (!end_time)
    {
        char scratch[12];

        uint32_t gid = rmh->rmg->id;
        uint32_t mid = rmh->id + 1;
        uint32_t pit = 0;

        const Slice StartK = SI->keySlice(scratch, gid, mid, pit);

        Iterator *It = SI->RDB->NewIterator(ReadOptions());
        for (It->SeekForPrev(StartK); It->Valid(); It->Next()) {
            const Slice &K = It->key();

            SI->parseKey(K, gid, mid, pit);
            end_time = pit;
            break;
        }
    }

    return end_time;
}

/*===---------------------------------------------------------------------===*/
/* Collection handles                                                        */
/*===---------------------------------------------------------------------===*/

static uint32_t rdb_store_metric_start_time(rdb_collect_handle *rch) {
    if (!rch->collection.value.size())
        return 0;

    const ValueWrapper &VW = rch->collection.value;
    uint32_t shift = VW.duration() - VW.updateEvery();
    return (rch->collection.pit_ut / USEC_PER_SEC) - shift;
}

static uint32_t rdb_store_metric_end_time(rdb_collect_handle *rch) {
    if (!rch->collection.value.size())
        return 0;

    return rch->collection.pit_ut / USEC_PER_SEC;
}

static void rdb_store_metric_flush_internal(rdb_collect_handle *rch, bool protect)
{
    if (protect) {
        spinlock_lock(&rch->collection.lock);
    }

    uint32_t gid = rch->rmh->rmg->id;
    uint32_t mid = rch->rmh->id;
    uint32_t pit = rdb_store_metric_start_time(rch);

    char buf[12];
    rocksdb::Slice K = SI->keySlice(buf, gid, mid, pit);

    // TODO: the max size should be 4096 + 6 bytes. is there
    // any performance difference if the bytes array has exact size?
    // ie. are we hitting hot vs. cold memory on serialization?
    std::array<char, 64 * 1024> bytes;
    const Slice V = rch->collection.value.flush(bytes.data(), bytes.size());

    // TODO: make 1024 an SI constant
    rch->collection.value.reset(1024);

    if (protect) {
        spinlock_unlock(&rch->collection.lock);
    }

    rocksdb::WriteOptions WO;
    WO.disableWAL = true;
    WO.sync = false;
    SI->RDB->Put(WO, K, V);

    num_pages_written++;
}

[[gnu::cold]]
static void rdb_store_metric_next_slow(STORAGE_COLLECT_HANDLE *sch, usec_t point_in_time_ut, usec_t update_every_ut,
                                       NETDATA_DOUBLE n, NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                                       uint16_t count, uint16_t anomaly_count, SN_FLAGS flags)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    
    spinlock_lock(&rch->collection.lock);

    // this might be the first time we are saving something in the collection handle.
    if (rch->collection.pit_ut == 0)
    {
        rch->collection.pit_ut = point_in_time_ut - update_every_ut;

        // try again
        spinlock_unlock(&rch->collection.lock);
        rdb_store_metric_next(sch, point_in_time_ut, n, min_value, max_value, count, anomaly_count, flags);
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

            if (points_gap >= rch->collection.value.capacity())
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
                    rdb_store_metric_next(sch, this_ut, NAN, NAN, NAN, 1, 0, SN_EMPTY_SLOT);
                    spinlock_lock(&rch->collection.lock);
                }
            }
        }

        spinlock_unlock(&rch->collection.lock);
        rdb_store_metric_next(sch, point_in_time_ut, n, min_value, max_value, count, anomaly_count, flags);
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
    rch->collection.value = ValueWrapper::create(RdbValue::PageCase::kStorageNumbersPage, rmg->arena, initial_slots, update_every);

    // link collection handle to its metric
    rmh->rch = rch;

    return reinterpret_cast<STORAGE_COLLECT_HANDLE *>(rch);
}

void rdb_store_metric_next(STORAGE_COLLECT_HANDLE *sch, usec_t point_in_time_ut,
                           NETDATA_DOUBLE n, NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                           uint16_t count, uint16_t anomaly_count, SN_FLAGS flags)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    spinlock_lock(&rch->collection.lock);

    if (unlikely(rch->collection.value.capacity() == 0))
    {
        rdb_store_metric_flush_internal(rch, false);
    }

    usec_t delta_ut = point_in_time_ut - rch->collection.pit_ut;
    usec_t update_every_ut = rch->collection.update_every_ut;

    if (unlikely(delta_ut != update_every_ut))
    {
        spinlock_unlock(&rch->collection.lock);
        rdb_store_metric_next_slow(sch, point_in_time_ut, update_every_ut, n, min_value, max_value, count, anomaly_count, flags);
        return;
    }

    rch->collection.value.appendPoint(point_in_time_ut, n, min_value, max_value, count, anomaly_count, flags);
    rch->collection.pit_ut += update_every_ut;
    spinlock_unlock(&rch->collection.lock);
}

void rdb_store_metric_change_collection_frequency(STORAGE_COLLECT_HANDLE *sch, int update_every_s)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    spinlock_lock(&rch->collection.lock);

    rdb_store_metric_flush_internal(rch, false);

    rch->collection.update_every_ut = update_every_s * USEC_PER_SEC;
    rch->collection.value.changeCollectionFrequency(update_every_s);

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

struct rdb_query_handle
{
    rdb_metric_handle *rmh;
    uint32_t now_s;

    pb::Arena Arena;
    std::optional<Page> P;
};

void rdb_load_metric_init(STORAGE_METRIC_HANDLE *smh, struct storage_engine_query_handle *seqh,
                          time_t start_time_s, time_t end_time_s, STORAGE_PRIORITY priority)
{
    time_t db_start_time_s = rdb_metric_oldest_time(smh);
    time_t db_end_time_s = rdb_metric_latest_time(smh);

    seqh->start_time_s = std::max(db_start_time_s, start_time_s);
    seqh->end_time_s = std::min(db_end_time_s, end_time_s);
    seqh->backend = STORAGE_ENGINE_BACKEND_RDB;
    seqh->priority = priority;

    rdb_query_handle *rqh = new rdb_query_handle();
    rqh->rmh = reinterpret_cast<rdb_metric_handle *>(rdb_metric_dup(smh));
    rqh->now_s = seqh->start_time_s;

    seqh->handle = reinterpret_cast<STORAGE_QUERY_HANDLE *>(rqh);
}

static void rdb_load_metric_next_page(rdb_query_handle *rqh)
{
    // check the collection handle first
    rdb_collect_handle *rch = rqh->rmh->rch;
    if (rch)
    {
        spinlock_lock(&rch->collection.lock);

        // find the start time of the current collection handle
        uint32_t pit = rch->collection.pit_ut / USEC_PER_SEC;
        uint32_t duration = rch->collection.value.duration();
        uint32_t start_time_s = pit - duration;

        if (rqh->now_s >= start_time_s)
        {
            rqh->P = rch->collection.value.getPage(&rqh->Arena, pit);
        }
        spinlock_unlock(&rch->collection.lock);
    }
}

static void rdb_load_metric_next_value(rdb_query_handle *rqh)
{
    /* Find the proper value wrapper */
    if (!rqh->P.has_value()) {
        rdb_load_metric_next_page(rqh);
    }
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
