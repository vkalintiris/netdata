#ifndef RDB_PRIVATE_H
#define RDB_PRIVATE_H

#include "rdb.h"
#include <google/protobuf/arena.h>
#include <rocksdb/db.h>

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

    uint32_t gid;
};

const rocksdb::Slice rdb_collection_key_serialize(char scratch[12], uint32_t gid, uint32_t mid, uint32_t pit);

bool rdb_collection_key_deserialize(const rocksdb::Slice &S, uint32_t &gid, uint32_t &mid, uint32_t &pit);

#endif /* RDB_PRIVATE_H */