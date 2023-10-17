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

namespace pb = google::protobuf;

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

enum class PageType : uint8_t
{
    StorageNumbersPage = rdbv::RdbValue::PageCase::kStorageNumbersPage,
};

class ImmutablePage
{
public:
    class ImmutablePageIterator
    {
    public:
        using iterator_category = std::forward_iterator_tag;
        using difference_type   = std::ptrdiff_t;
        using value_type        = STORAGE_POINT;
        using pointer           = value_type*;
        using reference         = value_type&;

        ImmutablePageIterator(const ImmutablePage *IP)
            : IP(IP), Position(0) { }

        ImmutablePageIterator(const ImmutablePage *IP, uint32_t Position)
            : IP(IP), Position(Position) { }

        bool operator==(const ImmutablePageIterator& Other) const
        {
            return (IP == Other.IP) && (Position == Other.Position);
        }

        bool operator!=(const ImmutablePageIterator& Other) const
        {
                return !(*this == Other);
        }

        inline value_type operator*() const
        {
            return IP->get(Position);
        }

        inline ImmutablePageIterator& operator++()
        {
            ++Position;
            return *this;
        }

        inline ImmutablePageIterator& operator--() {
            --Position;
            return *this;
        }

        inline ImmutablePageIterator operator++(int)
        {
            ImmutablePageIterator It = *this;
            ++(*this);
            return It;
        }

        inline ImmutablePageIterator operator--(int)
        {
            ImmutablePageIterator It = *this;
            --(*this);
            return It;
        }

    private:
        const ImmutablePage *IP;
        uint32_t Position;
    };

public:
    ImmutablePage(const rdbv::RdbValue *V) : V(V) { }

    inline PageType pageType() const
    {
        return static_cast<PageType>(V->Page_case());
    }

    template<uint32_t N>
    const Slice flush(std::array<char, N> &AR) const
    {
        assert(V->ByteSizeLong() <= AR.size());
        V->SerializeToArray(AR.data(), AR.size());
        return Slice(AR.data(), V->ByteSizeLong());
    }

    inline uint32_t size() const
    {
        switch (pageType())
        {
            case PageType::StorageNumbersPage:
                return V->storage_numbers_page().storage_numbers_size();
            default:
                __builtin_unreachable();
        }
    }

    inline const STORAGE_POINT get(uint32_t index) const
    {
        switch (pageType()) {
            case PageType::StorageNumbersPage:
            {
                auto &SNP = V->storage_numbers_page();
                assert(index < SNP.storage_numbers_size());
                storage_number SN = SNP.storage_numbers().Get(index);

                STORAGE_POINT SP;

                SP.min = SP.max = SP.sum = unpack_storage_number(SN);
                SP.flags = static_cast<SN_FLAGS>(SN & SN_USER_FLAGS);
                SP.count = 1;
                SP.anomaly_count = is_storage_number_anomalous(SN) ? 1 : 0;
                return SP;
            }
            default:
                __builtin_unreachable();
        }
    }

    inline ImmutablePageIterator begin()
    {
        return ImmutablePageIterator(this);
    }

    inline ImmutablePageIterator end()
    {
        return ImmutablePageIterator(this, size());
    }

private:
    const rdbv::RdbValue *V;
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
