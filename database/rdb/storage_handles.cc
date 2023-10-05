#include "database/rrd.h"
#include "libnetdata/libnetdata.h"
#include "rdb-private.h"
#include <google/protobuf/arena.h>
#include <google/protobuf/repeated_field.h>
#include "rocksdb/db.h"
#include "si.h"

#include "protos/rdbv.pb.h"

namespace pb = google::protobuf;

using rocksdb::Slice;
using rocksdb::Status;
using rocksdb::Iterator;
using rocksdb::ReadOptions;

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

    char scratch[12];

    uint32_t gid = rmh->rmg->id;
    uint32_t mid = rmh->id;
    uint32_t pit = 0;

    const Slice StartK = SI->keySlice(scratch, gid, mid, pit);

    Iterator *It = SI->RDB->NewIterator(ReadOptions());
    for (It->Seek(StartK); It->Valid(); It->Next()) {
        const Slice &K = It->key();

        SI->parseKey(K, gid, mid, pit);
        return pit;
    }

    return 0;
}

time_t rdb_metric_latest_time(STORAGE_METRIC_HANDLE *smh)
{
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    char scratch[12];

    uint32_t gid = rmh->rmg->id;
    uint32_t mid = rmh->id + 1;
    uint32_t pit = 0;

    const Slice StartK = SI->keySlice(scratch, gid, mid, pit);

    Iterator *It = SI->RDB->NewIterator(ReadOptions());
    for (It->SeekForPrev(StartK); It->Valid(); It->Next()) {
        const Slice &K = It->key();

        SI->parseKey(K, gid, mid, pit);
        return pit;
    }

    return 0;
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
/* Collection handles                                                        */
/*===---------------------------------------------------------------------===*/

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

    spinlock_init(&rch->collection.lock);
    rch->collection.pit = 0;
    rch->collection.rdb_value = pb::Arena::Create<rdbv::RdbValue>(rmg->arena);
    rch->collection.rdb_value->mutable_storage_numbers_page()->mutable_storage_numbers()->Reserve(1024);
    memset(rch->collection.rdb_value->mutable_storage_numbers_page()->mutable_storage_numbers()->mutable_data(), 0xDEADBEEF, 4096);
    rch->collection.rdb_value->mutable_storage_numbers_page()->set_update_every(update_every);

    return reinterpret_cast<STORAGE_COLLECT_HANDLE *>(rch);
}

static void rdb_store_metric_flush_internal(STORAGE_COLLECT_HANDLE *sch, bool protect)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    if (protect) {
        spinlock_lock(&rch->collection.lock);
    }

    uint32_t gid = rch->rmh->rmg->id;
    uint32_t mid = rch->rmh->id;
    uint32_t pit = rch->collection.pit;

    char buf[12];
    rocksdb::Slice K = SI->keySlice(buf, gid, mid, pit);

    // TODO: the max size should be 4096 + 6 bytes. is there
    // any performance difference if the bytes buffer has exact size?
    // ie. are we hitting hot vs. cold memory on serialization?
    std::array<char, 64 * 1024> bytes;
    size_t n = rch->collection.rdb_value->ByteSizeLong();
    if (n > bytes.size())
        fatal("Could not serialize rdb value: (n=%zu > %zu bytes)", n, bytes.size());
    rch->collection.rdb_value->SerializeToArray(bytes.data(), bytes.size());

    if (protect) {
        spinlock_unlock(&rch->collection.lock);
    }

    rocksdb::Slice V(bytes.data(), n);

    rocksdb::WriteOptions WO;
    WO.disableWAL = true;
    WO.sync = false;
    SI->RDB->Put(WO, K, V);
    num_pages_written++;
}

void rdb_store_metric_flush(STORAGE_COLLECT_HANDLE *sch)
{
    rdb_store_metric_flush_internal(sch, true);
}

[[gnu::cold]]
static void rdb_store_metric_next_slow(STORAGE_COLLECT_HANDLE *sch, usec_t point_in_time,
                                       NETDATA_DOUBLE n, NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                                       uint16_t count, uint16_t anomaly_count, SN_FLAGS flags)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    
    rdbv::StorageNumbersPage *snp = rch->collection.rdb_value->mutable_storage_numbers_page();
    pb::RepeatedField<uint32_t> *sns = snp->mutable_storage_numbers();

    spinlock_lock(&rch->collection.lock);

    // this might be the first time we are saving something in the collection handle.
    if ((sns->size() == 0) && (rch->collection.pit == 0)) {
        rch->collection.pit = (point_in_time / USEC_PER_SEC) - snp->update_every();

        // try again
        spinlock_unlock(&rch->collection.lock);
        rdb_store_metric_next(sch, point_in_time, n, min_value, max_value, count, anomaly_count, flags);
        return;
    }

    usec_t page_end_time = rch->collection.pit * USEC_PER_SEC;

    if (page_end_time < point_in_time)
    {
        // point_in_time is in the future
        netdata_log_error("[1] point_in_time is in the future");

        usec_t delta_ut = point_in_time - (rch->collection.pit * USEC_PER_SEC);
        if (delta_ut < snp->update_every())
        {
            // step is too small
            rdb_store_metric_flush_internal(sch, false);
            sns->Clear();
        }
        else if (delta_ut < snp->update_every())
        {
            // step is unaligned
            rdb_store_metric_flush_internal(sch, false);
            sns->Clear();
        }
        else
        {
            // aligned but in the future
            size_t points_gap = delta_ut / (snp->update_every() * USEC_PER_SEC);
            size_t page_remaining_points = 1024 - sns->size();

            if (points_gap >= page_remaining_points)
            {
                // we can't store any points in the current page
                rdb_store_metric_flush_internal(sch, false);
                sns->Clear();
            }
            else
            {
                // fill gaps in the current page
                usec_t stop_ut = point_in_time - (snp->update_every() * USEC_PER_SEC);

                for (usec_t this_ut = (rch->collection.pit + snp->update_every()) * USEC_PER_SEC;
                     this_ut <= stop_ut;
                     this_ut = (rch->collection.pit + snp->update_every()) * USEC_PER_SEC)
                {
                    spinlock_unlock(&rch->collection.lock);
                    rdb_store_metric_next(sch, this_ut, NAN, NAN, NAN, 1, 0, SN_EMPTY_SLOT);
                    spinlock_lock(&rch->collection.lock);
                }
            }
        }

        spinlock_unlock(&rch->collection.lock);
        rdb_store_metric_next(sch, point_in_time, n, min_value, max_value, count, anomaly_count, flags);
        return;
    }
    else if (page_end_time > point_in_time)
    {
        netdata_log_error("[2] point_in_time is in the past");

        // point_in_time is in the past, nothing to do
        spinlock_unlock(&rch->collection.lock);
        return;
    }
    else if (page_end_time == point_in_time)
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

void rdb_store_metric_next(STORAGE_COLLECT_HANDLE *sch, usec_t point_in_time,
                           NETDATA_DOUBLE n, NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                           uint16_t count, uint16_t anomaly_count, SN_FLAGS flags)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    rdbv::StorageNumbersPage *snp = rch->collection.rdb_value->mutable_storage_numbers_page();
    pb::RepeatedField<uint32_t> *sns = snp->mutable_storage_numbers();

    spinlock_lock(&rch->collection.lock);

    if (sns->size() >= 1024) {
        rdb_store_metric_flush_internal(sch, false);
        sns->Clear();
    }

    usec_t delta_ut = point_in_time - (rch->collection.pit * USEC_PER_SEC);
    if (unlikely(delta_ut != (snp->update_every() * USEC_PER_SEC))) {
        spinlock_unlock(&rch->collection.lock);
        rdb_store_metric_next_slow(sch, point_in_time, n, min_value, max_value, count, anomaly_count, flags);
        return;
    }

    storage_number *sn = sns->AddAlreadyReserved();
    *sn = pack_storage_number(n, flags);
    rch->collection.pit += snp->update_every();

    spinlock_unlock(&rch->collection.lock);
}

int rdb_store_metric_finalize(STORAGE_COLLECT_HANDLE *sch)
{
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    delete rch;
    return 0;
}


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
