#ifndef RDB_SI_H
#define RDB_SI_H

#include "rdb-private.h"
#include "uuid_shard.h"

namespace rocksdb
{
    class DB;
};

using rocksdb::Slice;

namespace rdb {

class Key
{
public:
    constexpr static size_t Fields = 3;
    constexpr static size_t Bytes = Fields * sizeof(uint32_t);

private:
    constexpr static size_t GroupIdField = 0;
    constexpr static size_t MetricIdField = 1;
    constexpr static size_t PointInTimeField = 2;

private:
    Key() = delete;

    inline uint32_t field(size_t i) const
    {
        assert(i < 3);

        uint32_t f;
        memcpy(&f, &scratch[i * sizeof(uint32_t)], sizeof(uint32_t));
        return be32toh(f);
    }

public:
    inline Key(uint32_t gid, uint32_t mid, uint32_t pit)
    {
        gid = htobe32(gid);
        mid = htobe32(mid);
        pit = htobe32(pit);

        memcpy(&scratch[GroupIdField * sizeof(uint32_t)], &gid, sizeof(uint32_t));
        memcpy(&scratch[MetricIdField * sizeof(uint32_t)], &mid, sizeof(uint32_t));
        memcpy(&scratch[PointInTimeField * sizeof(uint32_t)], &pit, sizeof(uint32_t));
    }

    inline Key(const Slice &S)
    {
        memcpy(&scratch[0], S.data(), 12);
    }

    inline const Slice slice() const
    {
        return Slice(scratch, Key::Bytes);
    }

    inline uint32_t gid() const
    {
        return field(GroupIdField);
    }

    inline uint32_t mid() const
    {
        return field(MetricIdField);
    }
    
    inline uint32_t pit() const
    {
        return field(PointInTimeField);
    }

    std::string toString(bool hex = false) const
    {
        std::array<char, 1024> buf;

        if (hex)
        {
            snprintfz(buf.data(), buf.size() - 1,
                      "gid=%u, mid=%u, pit=%u (0x%s)",
                      gid(), mid(), pit(), slice().ToString(true).c_str());
        }
        else
        {
            snprintfz(buf.data(), buf.size() - 1,
                      "gid=%u, mid=%u, pit=%u",
                      gid(), mid(), pit());
        }

        return std::string(buf.data()); 
    }

private:
    char scratch[Key::Bytes];
};

} // namespace rdb

class StorageInstance {
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

    inline const Slice keySlice(char scratch[12], uint32_t gid, uint32_t mid, uint32_t pit) const
    {
        gid = htobe32(gid);
        mid = htobe32(mid);
        pit = htobe32(pit);

        memcpy(&scratch[0 * sizeof(uint32_t)], &gid, sizeof(uint32_t));
        memcpy(&scratch[1 * sizeof(uint32_t)], &mid, sizeof(uint32_t));
        memcpy(&scratch[2 * sizeof(uint32_t)], &pit, sizeof(uint32_t));

        return Slice(scratch, 3 * sizeof(uint32_t));
    }

    inline bool parseKey(const Slice &S, uint32_t &gid, uint32_t &mid, uint32_t &pit)
    {
        const char *data = S.data();

        memcpy(&gid, &data[0 * sizeof(uint32_t)], sizeof(uint32_t));
        memcpy(&mid, &data[1 * sizeof(uint32_t)], sizeof(uint32_t));
        memcpy(&pit, &data[2 * sizeof(uint32_t)], sizeof(uint32_t));

        gid = be32toh(gid);
        mid = be32toh(mid);
        pit = be32toh(pit);

        return true;
    }

    google::protobuf::Arena *getThreadArena()
    {
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
