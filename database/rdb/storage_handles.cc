#include "rdb-private.h"

struct rdb_collect_handle {
    struct storage_collect_handle common; // has to be first item
    rdb_metric_handle *rmh;
    storage_number *sns;
};

STORAGE_COLLECT_HANDLE *rdb_store_metric_init(STORAGE_METRIC_HANDLE *smh, uint32_t update_every, STORAGE_METRICS_GROUP *smg)
{
    rdb_collect_handle *rch = new rdb_collect_handle();

    rch->common.backend = STORAGE_ENGINE_BACKEND_RDB;
    rch->rmh = reinterpret_cast<rdb_metric_handle *>(rdb_metric_dup(smh));

    UNUSED(update_every);
    UNUSED(smg);

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
}

void rdb_store_metric_flush(STORAGE_COLLECT_HANDLE *sch) {
    UNUSED(sch);
}

int rdb_store_metric_finalize(STORAGE_COLLECT_HANDLE *sch) {
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);
    delete rch;
    return 0;
}
