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

    rch->rdb_value = Arena::Create<rdbv::RdbValue>(rmg->arena);
    rch->rdb_value->mutable_storage_numbers_page()->mutable_storage_numbers()->Reserve(1024);

    // TODO: Improve this. Can we make this per-thread "global"?
    spinlock_init(&rch->lock);

    UNUSED(update_every);

    return reinterpret_cast<STORAGE_COLLECT_HANDLE *>(rch);
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

    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    RepeatedField<uint32_t> *sns = rch->rdb_value->mutable_storage_numbers_page()->mutable_storage_numbers();

    spinlock_lock(&rch->lock);

    if (sns->size() >= 1024) {
        rdb_store_metric_flush(sch);

        sns->Clear();
    }
    
    storage_number *sn = sns->AddAlreadyReserved();
    *sn = pack_storage_number(n, flags);

    spinlock_unlock(&rch->lock);
}

void rdb_store_metric_flush(STORAGE_COLLECT_HANDLE *sch) {
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    // TODO: check perf if we have a uint64_t field just for the id inside
    // rch, ie. (rmg->id << 32 | rmh->id).
    uint32_t gid = rch->rmh->gid;
    uint32_t mid = rch->rmh->id;
    uint32_t pit = rch->pit;

    char buf[12];
    rocksdb::Slice K = rdb_collection_key_serialize(buf, gid, mid, pit);

    std::array<char, 64 * 1024> bytes;
    size_t n = rch->rdb_value->ByteSizeLong();
    if (n > bytes.size()) {
        fatal("Could not serialize rdb value: (n=%zu > %zu bytes)", n, bytes.size());
    }
    rch->rdb_value->SerializeToArray(bytes.data(), bytes.size());
    rocksdb::Slice V(bytes.data(), n);

    rocksdb::WriteOptions WO;
    WO.disableWAL = true;
    WO.sync = false;
    RDB->Put(WO, K, V);
    num_pages_written++;
}

int rdb_store_metric_finalize(STORAGE_COLLECT_HANDLE *sch) {
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    delete rch;
    return 0;
}
