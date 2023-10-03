#include "libnetdata/libnetdata.h"
#include "rdb-private.h"
#include <google/protobuf/arena.h>
#include <google/protobuf/repeated_field.h>
#include "rocksdb/db.h"
#include "si.h"

using namespace google::protobuf;

struct rdb_collect_handle {
    struct storage_collect_handle common; // has to be first item
    rdb_metric_handle *rmh;
    RepeatedField<storage_number> sns;
    SPINLOCK lock;
};

STORAGE_COLLECT_HANDLE *rdb_store_metric_init(STORAGE_METRIC_HANDLE *smh, uint32_t update_every, STORAGE_METRICS_GROUP *smg)
{
    rdb_metrics_group *rmg = reinterpret_cast<rdb_metrics_group *>(smg);

    rdb_collect_handle *rch = new rdb_collect_handle();
    rch->common.backend = STORAGE_ENGINE_BACKEND_RDB;
    rch->rmh = reinterpret_cast<rdb_metric_handle *>(rdb_metric_dup(smh));
    rch->sns = RepeatedField<storage_number>(rmg->arena);

    // FIXME: improve this
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

    spinlock_lock(&rch->lock);

    storage_number *sn = rch->sns.Add();
    *sn = pack_storage_number(n, flags);

    if (rch->sns.size() >= 1024) {
        uint32_t pit = point_in_time / USEC_PER_SEC;

        char buf[8] = { 0 };
        memcpy(buf, &rch->rmh->id, sizeof(uint32_t));
        memcpy(&buf[sizeof(uint32_t)], &pit, sizeof(uint32_t));

        rocksdb::Slice K(buf, 8);
        rocksdb::Slice V((const char *) rch->sns.data(), rch->sns.size() * sizeof(storage_number));

        rocksdb::WriteOptions WO;
        WO.disableWAL = true;
        WO.sync = false;
        RDB->Put(WO, K, V);

        rch->sns.Clear();
    }
    
    spinlock_unlock(&rch->lock);
}

void rdb_store_metric_flush(STORAGE_COLLECT_HANDLE *sch) {
    UNUSED(sch);
}

int rdb_store_metric_finalize(STORAGE_COLLECT_HANDLE *sch) {
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    delete rch;
    return 0;
}
