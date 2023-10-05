#ifndef RDB_PRIVATE_H
#define RDB_PRIVATE_H

#include "rdb.h"
#include "barrier.h"
#include "protos/rdbv.pb.h"

#include <rocksdb/db.h>

struct rdb_collect_handle;

struct rdb_metrics_group
{
    uuid_t uuid;
    uint32_t id;
    uint32_t rc;
    google::protobuf::Arena *arena;
};

struct rdb_metric_handle
{
    uuid_t uuid;
    uint32_t id;
    uint32_t rc;

    rdb_metrics_group *rmg;
    rdb_collect_handle *rch;
};

class ValueWrapper
{
public:
    static ValueWrapper create(rdbv::RdbValue::PageCase PC, google::protobuf::Arena *Arena, uint32_t Slots, uint32_t UpdateEvery);

    bool appendPoint(usec_t point_in_time_ut, NETDATA_DOUBLE n,
                     NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                     uint16_t count, uint16_t anomaly_count, SN_FLAGS flags);

    const rocksdb::Slice flush(char *buffer, size_t n) const;

    inline uint32_t capacity() const {
        return Slots;
    }

    inline uint32_t updateEvery() const {
        switch (Value->Page_case()) {
            case rdbv::RdbValue::PageCase::kStorageNumbersPage:
                return Value->storage_numbers_page().update_every();
            default:
                return 0;
        }
    }

    void reset(uint32_t Slots);

private:
    rdbv::RdbValue *Value;
    uint32_t Slots;
};

struct rdb_collect_handle
{
    // has to be first item
    struct storage_collect_handle common;

    // back-links to group/metric handles
    rdb_metrics_group *rmg;
    rdb_metric_handle *rmh;

    // collection data
    struct {
        // Can we make the lock per-thread?
        SPINLOCK lock;
        ValueWrapper value;
        usec_t pit_ut;
    } collection;
};

#endif /* RDB_PRIVATE_H */