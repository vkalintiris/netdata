#ifndef RDB_PRIVATE_H
#define RDB_PRIVATE_H

#include "rdb.h"
#include "barrier.h"
#include "protos/rdbv.pb.h"

#include <rocksdb/db.h>

struct rdb_collect_handle;

struct rdb_metrics_group {
    uuid_t uuid;
    uint32_t id;
    uint32_t rc;
    google::protobuf::Arena *arena;
};

struct rdb_metric_handle {
    uuid_t uuid;
    uint32_t id;
    uint32_t rc;

    rdb_metrics_group *rmg;
    rdb_collect_handle *rch;
};

struct rdb_collect_handle {
    // has to be first item
    struct storage_collect_handle common;

    // back-links to group/metric handles
    rdb_metrics_group *rmg;
    rdb_metric_handle *rmh;

    // collection data
    struct {
        // Can we make the lock per-thread?
        SPINLOCK lock;
        rdbv::RdbValue *rdb_value;
        uint32_t pit;
    } collection;
};

#endif /* RDB_PRIVATE_H */