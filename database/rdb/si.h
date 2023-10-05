#ifndef RDB_SI_H
#define RDB_SI_H

#include "rdb-private.h"
#include "uuid_shard.h"

namespace rocksdb {
    class DB;
};

class StorageInstance {
public:
    StorageInstance(size_t registry_shards) :
        RDB(nullptr),
        GroupsRegistry(registry_shards),
        MetricsRegistry(registry_shards)
    {}

    rocksdb::Status open(rocksdb::Options Opts, const char *path) {
        rocksdb::Status S = rocksdb::DB::Open(Opts, path, &RDB);
        if (!S.ok())
            RDB = nullptr;

        return S;
    }

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
    rocksdb::DB *RDB;
    UuidShard<rdb_metrics_group> GroupsRegistry;
    UuidShard<rdb_metric_handle> MetricsRegistry;

    std::mutex ArenasMutex;
    std::unordered_map<pid_t, google::protobuf::Arena *> Arenas;
};

extern StorageInstance *SI;
extern std::atomic<size_t> num_pages_written;

#endif /* RDB_SI_H */
