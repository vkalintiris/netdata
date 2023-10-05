#include "database/rrd.h"
#include "libnetdata/libnetdata.h"
#include "rdb-private.h"
#include <google/protobuf/arena.h>
#include <google/protobuf/repeated_field.h>
#include "rocksdb/db.h"
#include "si.h"

#include "protos/rdbv.pb.h"

using namespace google::protobuf;

struct rdb_collect_handle {
    struct storage_collect_handle common; // has to be first item
    rdb_metrics_group *rmg;
    rdb_metric_handle *rmh;

    uint32_t pit;
    rdbv::RdbValue *rdb_value;

    SPINLOCK lock;
};

const rocksdb::Slice rdb_collection_key_serialize(char scratch[12], uint32_t gid, uint32_t mid, uint32_t pit)
{
    memcpy(&scratch[0 * sizeof(uint32_t)], &gid, sizeof(uint32_t));
    memcpy(&scratch[1 * sizeof(uint32_t)], &mid, sizeof(uint32_t));
    memcpy(&scratch[2 * sizeof(uint32_t)], &pit, sizeof(uint32_t));

    return rocksdb::Slice(scratch, 3 * sizeof(uint32_t));
}

bool rdb_collection_key_deserialize(const rocksdb::Slice &S, uint32_t &gid, uint32_t &mid, uint32_t &pit)
{
    // TODO: skip this on release builds
    if (S.size() != 3 * sizeof(uint32_t))
        return false;
    
    const char *data = S.data();

    memcpy(&gid, &data[0 * sizeof(uint32_t)], sizeof(uint32_t));
    memcpy(&mid, &data[1 * sizeof(uint32_t)], sizeof(uint32_t));
    memcpy(&pit, &data[2 * sizeof(uint32_t)], sizeof(uint32_t));

    return true;
}

// TODO: rrd api should specify the page type
STORAGE_COLLECT_HANDLE *rdb_store_metric_init(STORAGE_METRIC_HANDLE *smh, uint32_t update_every, STORAGE_METRICS_GROUP *smg)
{
    rdb_metrics_group *rmg = reinterpret_cast<rdb_metrics_group *>(smg);

    rdb_collect_handle *rch = new rdb_collect_handle();
    rch->common.backend = STORAGE_ENGINE_BACKEND_RDB;

    // TODO: dup group like metric handle (rrd api should allow this)
    rch->rmh = reinterpret_cast<rdb_metric_handle *>(rdb_metric_dup(smh));
    rch->rmg = reinterpret_cast<rdb_metrics_group *>(smg);
    rch->rmh->gid = rch->rmg->id;
    rch->pit = 0;

    rch->rdb_value = Arena::Create<rdbv::RdbValue>(rmg->arena);
    rch->rdb_value->mutable_storage_numbers_page()->mutable_storage_numbers()->Reserve(1024);
    memset(rch->rdb_value->mutable_storage_numbers_page()->mutable_storage_numbers()->mutable_data(), 0xDEADBEEF, 4096);
    rch->rdb_value->mutable_storage_numbers_page()->set_update_every(update_every);

    // TODO: Improve this. Can we make this per-thread "global"?
    spinlock_init(&rch->lock);

    UNUSED(update_every);

    return reinterpret_cast<STORAGE_COLLECT_HANDLE *>(rch);
}

static void rdb_store_metric_flush_internal(STORAGE_COLLECT_HANDLE *sch, bool protect) {
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    if (protect) {
        spinlock_lock(&rch->lock);
    }

    // TODO: check perf if we have a uint64_t field just for the id inside
    // rch, ie. (rmg->id << 32 | rmh->id).
    uint32_t gid = rch->rmh->gid;
    uint32_t mid = rch->rmh->id;
    uint32_t pit = rch->pit;

    char buf[12];
    rocksdb::Slice K = rdb_collection_key_serialize(buf, gid, mid, pit);

    // TODO: the max size should be 4096 + 6 bytes. is there
    // any performance difference if the bytes buffer has exact size?
    // ie. are we hitting hot vs. cold memory on serialization?
    std::array<char, 64 * 1024> bytes;
    size_t n = rch->rdb_value->ByteSizeLong();
    if (n > bytes.size())
        fatal("Could not serialize rdb value: (n=%zu > %zu bytes)", n, bytes.size());
    rch->rdb_value->SerializeToArray(bytes.data(), bytes.size());

    if (protect) {
        spinlock_unlock(&rch->lock);
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
    
    rdbv::StorageNumbersPage *snp = rch->rdb_value->mutable_storage_numbers_page();
    RepeatedField<uint32_t> *sns = snp->mutable_storage_numbers();

    spinlock_lock(&rch->lock);

    // this might be the first time we are saving something in the collection handle.
    if ((sns->size() == 0) && (rch->pit == 0)) {
        rch->pit = (point_in_time / USEC_PER_SEC) - snp->update_every();

        // try again
        spinlock_unlock(&rch->lock);
        rdb_store_metric_next(sch, point_in_time, n, min_value, max_value, count, anomaly_count, flags);
        return;
    }

    usec_t page_end_time = rch->pit * USEC_PER_SEC;

    if (page_end_time < point_in_time)
    {
        // point_in_time is in the future
        netdata_log_error("[1] point_in_time is in the future");

        usec_t delta_ut = point_in_time - (rch->pit * USEC_PER_SEC);
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

                for (usec_t this_ut = (rch->pit + snp->update_every()) * USEC_PER_SEC;
                     this_ut <= stop_ut;
                     this_ut = (rch->pit + snp->update_every()) * USEC_PER_SEC)
                {
                    spinlock_unlock(&rch->lock);
                    rdb_store_metric_next(sch, this_ut, NAN, NAN, NAN, 1, 0, SN_EMPTY_SLOT);
                    spinlock_lock(&rch->lock);
                }
            }
        }

        spinlock_unlock(&rch->lock);
        rdb_store_metric_next(sch, point_in_time, n, min_value, max_value, count, anomaly_count, flags);
        return;
    }
    else if (page_end_time > point_in_time)
    {
        netdata_log_error("[2] point_in_time is in the past");

        // point_in_time is in the past, nothing to do
        spinlock_unlock(&rch->lock);
        return;
    }
    else if (page_end_time == point_in_time)
    {
        netdata_log_error("[3] point_in_time has not progressed");

        // point_in_time has already been saved, nothing to do
        spinlock_unlock(&rch->lock);
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

    rdbv::StorageNumbersPage *snp = rch->rdb_value->mutable_storage_numbers_page();
    RepeatedField<uint32_t> *sns = snp->mutable_storage_numbers();

    spinlock_lock(&rch->lock);

    if (sns->size() >= 1024) {
        rdb_store_metric_flush_internal(sch, false);
        sns->Clear();
    }

    usec_t delta_ut = point_in_time - (rch->pit * USEC_PER_SEC);
    if (unlikely(delta_ut != (snp->update_every() * USEC_PER_SEC))) {
        spinlock_unlock(&rch->lock);
        rdb_store_metric_next_slow(sch, point_in_time, n, min_value, max_value, count, anomaly_count, flags);
        return;
    }

    storage_number *sn = sns->AddAlreadyReserved();
    *sn = pack_storage_number(n, flags);
    rch->pit += snp->update_every();

    spinlock_unlock(&rch->lock);
}

int rdb_store_metric_finalize(STORAGE_COLLECT_HANDLE *sch) {
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    delete rch;
    return 0;
}
