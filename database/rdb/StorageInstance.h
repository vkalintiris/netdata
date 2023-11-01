#ifndef RDB_STORAGE_INSTANCE_H
#define RDB_STORAGE_INSTANCE_H

#include "rdb-common.h"
#include <google/protobuf/arena.h>

struct rdb_collect_handle;

struct rdb_metrics_group
{
    uuid_t uuid;
    uint32_t id;
    uint32_t rc;
    google::protobuf::Arena *arena;

    rdb_metrics_group() : uuid{}, id{0}, rc{0}, arena{nullptr} { }
};

struct rdb_metric_handle
{
    uuid_t uuid;
    uint32_t id;
    uint32_t rc;

    rdb_metrics_group *rmg;
    rdb_collect_handle *rch;

    std::atomic<uint32_t> oldest_time;

    rdb_metric_handle() :
        uuid{}, id{0}, rc{0}, rmg{nullptr}, rch{nullptr}, oldest_time{0}
    { }
};

namespace rdb {

class StorageInstance
{
public:
    StorageInstance(size_t registry_shards) :
        RDB(nullptr),
        GroupsRegistry(registry_shards),
        MetricsRegistry(registry_shards)
    { }

    rocksdb::Status open(rocksdb::Options Opts, const char *Path)
    {
        using namespace rocksdb;
            
        Opts.error_if_exists = false;
        Opts.create_if_missing = true;
        Opts.create_missing_column_families = true;
            
        std::vector<std::string> CFs;
        Status S = DB::ListColumnFamilies(DBOptions(), Path, &CFs);

        if (!S.ok())
        {
            CFs = { kDefaultColumnFamilyName };
            CFs.push_back("md");
            CFs.push_back("mh");
        }
            
        std::vector<ColumnFamilyDescriptor> CFDs;
        for (const auto& CF : CFs)
        {
            CFDs.emplace_back(CF, rocksdb::ColumnFamilyOptions());
        }

        return rocksdb::DB::Open(Opts, Path, CFDs, &CFHs, &RDB);
    }

    rocksdb::Status putMD(const rocksdb::Slice &K, const rocksdb::Slice &V)
    {
        rocksdb::WriteOptions WO;
        WO.disableWAL = true;
        WO.sync = false;
        return RDB->Put(WO, CFHs[1], K, V);
    }

    rocksdb::Iterator *getIteratorMD(const rocksdb::ReadOptions &RO)
    {
        return RDB->NewIterator(RO, CFHs[1]);
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
            if (It == Arenas.cend())
            {
                pb::ArenaOptions AO;
                AO.start_block_size = 1024 * 1024;
                AO.max_block_size = AO.start_block_size;

                pb::Arena *A = new pb::Arena(AO);

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

    std::vector<rocksdb::ColumnFamilyHandle *> CFHs;

    std::mutex ArenasMutex;
    std::unordered_map<pid_t, pb::Arena *> Arenas;
};

} // namespace rdb

extern rdb::StorageInstance *SI;
extern std::atomic<size_t> NumFlushedPages;

#endif /* RDB_STORAGE_INSTANCE_H */
