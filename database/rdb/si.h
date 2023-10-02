#ifndef RDB_SI_H
#define RDB_SI_H

#include "rdb-private.h"
#include "uuid_shard.h"

class StorageInstance {
public:
    StorageInstance(size_t num_shards) :
        GroupsRegistry(num_shards),
        MetricsRegistry(num_shards)
    { }

public:
    UuidShard<rdb_metrics_group> GroupsRegistry;
    UuidShard<rdb_metric_handle> MetricsRegistry;
};

extern StorageInstance SI;

#endif /* RDB_SI_H */
