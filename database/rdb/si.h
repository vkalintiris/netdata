#ifndef RDB_SI_H
#define RDB_SI_H

#include "rdb-private.h"
#include "uuid_shard.h"

#include "google/protobuf/arena.h"
#include <mutex>

class StorageInstance {
public:
    StorageInstance(size_t registry_shards) :
        GroupsRegistry(registry_shards),
        MetricsRegistry(registry_shards)
    { }

    google::protobuf::Arena *getThreadArena() {
        pid_t tid = gettid();

        {
            std::lock_guard<std::mutex> L(ArenasMutex);

            auto It = Arenas.find(tid);
            if (It == Arenas.cend()) {
                google::protobuf::Arena *A = new google::protobuf::Arena();
                Arenas[tid] = A;
                return A;
            } else {
                return It->second;
            }
        }
    }

public:
    UuidShard<rdb_metrics_group> GroupsRegistry;
    UuidShard<rdb_metric_handle> MetricsRegistry;

    std::mutex ArenasMutex;
    std::unordered_map<pid_t, google::protobuf::Arena *> Arenas;
};

namespace rocksdb {
    class DB;
};

extern StorageInstance *SI;
extern rocksdb::DB *RDB;

#endif /* RDB_SI_H */
