#ifndef RDB_STORAGE_INSTANCE_H
#define RDB_STORAGE_INSTANCE_H

#include "rdb-common.h"

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

namespace rdb {

class StorageInstance
{
public:
    StorageInstance(size_t registry_shards) :
        RDB(nullptr),
        GroupsRegistry(registry_shards),
        MetricsRegistry(registry_shards)
    {}

    rocksdb::Status open(rocksdb::Options Opts, const char *path)
    {
        rocksdb::Status S = rocksdb::DB::Open(Opts, path, &RDB);
        if (!S.ok())
            RDB = nullptr;

        return S;
    }

    void close()
    {
        rocksdb::FlushOptions FO;
        FO.allow_write_stall = true;
        FO.wait = true;

        RDB->Flush(FO);
        RDB->SyncWAL();

        RDB->Close();
        delete RDB;
        RDB = nullptr;
    }

    pb::Arena *getThreadArena()
    {
        pid_t tid = gettid();

        {
            std::lock_guard<std::mutex> L(ArenasMutex);

            auto It = Arenas.find(tid);
            if (It == Arenas.cend()) {
                pb::Arena *A = new pb::Arena();
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
    std::unordered_map<pid_t, pb::Arena *> Arenas;
};

} // namespace rdb

extern rdb::StorageInstance *SI;
extern std::atomic<size_t> num_pages_written;

#endif /* RDB_STORAGE_INSTANCE_H */
