#ifndef RDB_PRIVATE_H
#define RDB_PRIVATE_H

#include "rdb.h"
#include <google/protobuf/arena.h>

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
};

#endif /* RDB_PRIVATE_H */