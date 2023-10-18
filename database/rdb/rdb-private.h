#ifndef RDB_PRIVATE_H
#define RDB_PRIVATE_H

#include "barrier.h"
#include "protos/rdbv.pb.h"
#include "rdb.h"
#include "uuid_shard.h"

#include <rocksdb/advanced_options.h>
#include <rocksdb/db.h>
#include <rocksdb/statistics.h>
#include <rocksdb/table.h>

#ifdef ENABLE_TESTS
#include <gtest/gtest.h>
#include <random>
#endif

namespace rdb {

namespace pb = google::protobuf;

using Options = rocksdb::Options;
using Slice = rocksdb::Slice;
using Status = rocksdb::Status;

using Value = rdbv::RdbValue;
using StorageNumbersPage = rdbv::StorageNumbersPage;

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

    [[nodiscard]] inline uint32_t field(size_t i) const
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

    [[nodiscard]] inline const Slice slice() const
    {
        return Slice(scratch, Key::Bytes);
    }

    [[nodiscard]] inline uint32_t gid() const
    {
        return field(GroupIdField);
    }

    [[nodiscard]] inline uint32_t mid() const
    {
        return field(MetricIdField);
    }
    
    [[nodiscard]] inline uint32_t pit() const
    {
        return field(PointInTimeField);
    }

    [[nodiscard]] std::string toString(bool hex = false) const
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
    StorageNumbersPage = Value::PageCase::kStorageNumbersPage,
};

struct PageOptions
{
    PageType page_type = PageType::StorageNumbersPage;
    uint32_t capacity = 1024;

    PageOptions() {}
};

class Page
{
public:
    // A full blown random-access iterator, we most probably need
    // just a simple forward iterator
    class PageIterator
    {
        friend class Page;

    public:
        using iterator_category = std::random_access_iterator_tag;
        using difference_type   = std::ptrdiff_t;
        using value_type        = STORAGE_POINT;
        using pointer           = value_type*;
        using reference         = value_type&;

    private:
        PageIterator(const Page *IP, const uint32_t PIT, const uint32_t Pos)
            : IP(IP), PIT(PIT), Pos(Pos) { }

    public:
        [[nodiscard]] static PageIterator create(const Page *IP,
                                                 uint32_t Pos,
                                                 uint32_t PIT)
        {
            return PageIterator(IP, Pos, PIT);
        }

        bool operator==(const PageIterator& Other) const
        {
            // We intentionaly ignore PIT to simplify the begin()/end() API.
            return (IP == Other.IP) && (Pos == Other.Pos);
        }

        bool operator!=(const PageIterator& Other) const
        {
                return !(*this == Other);
        }

        inline value_type operator*() const
        {
            return IP->get(Pos, PIT);
        }

        inline PageIterator& operator++()
        {
            ++Pos;
            return *this;
        }

        inline PageIterator& operator--() {
            --Pos;
            return *this;
        }

        inline PageIterator operator++(int)
        {
            PageIterator It = *this;
            ++(*this);
            return It;
        }

        inline PageIterator operator--(int)
        {
            PageIterator It = *this;
            --(*this);
            return It;
        }

        inline PageIterator operator+(int N) const
        {
            PageIterator It = *this;
            It.Pos += N;
            return It;
        }

        inline PageIterator operator-(int N) const
        {
            PageIterator It = *this;
            It.Pos -= N;
            return It;
        }

        inline PageIterator& operator+=(int N)
        {
            Pos += N;
            return *this;
        }

        inline PageIterator& operator-=(int N)
        {
            Pos -= N;
            return *this;
        }

        inline value_type operator[](int N) const
        {
            return IP->get(Pos + N, PIT);
        }

        inline bool operator<(const PageIterator& Other) const
        {
            return Pos < Other.Pos;
        }

        inline bool operator>(const PageIterator& Other) const
        {
            return Pos > Other.Pos;
        }

        inline bool operator<=(const PageIterator& Other) const
        {
            return Pos <= Other.Pos;
        }

        inline bool operator>=(const PageIterator& Other) const
        {
            return Pos >= Other.Pos;
        }

        inline int operator-(const PageIterator& Other) const
        {
            return Pos - Other.Pos;
        }

    private:
        const Page *IP;
        const uint32_t PIT;
        uint32_t Pos;
    };

public:
    [[nodiscard]] static std::optional<const Page> fromSlice(pb::Arena &Arena, const Slice &S)
    {
        Value *V = pb::Arena::CreateMessage<Value>(&Arena);
        if (!V)
            return {};

        if (!V->ParseFromArray(S.data(), S.size()))
            return {};

        return Page(V);
    }

    [[nodiscard]] static std::optional<Page> create(pb::Arena &Arena, const PageOptions &PO)
    {
        Value *V = pb::Arena::CreateMessage<Value>(&Arena);
        if (!V)
            return {};

        Page P(V);
        P.reserve(PO.page_type, PO.capacity);
        return P;
    }

private:
    Page(Value *V) : V(V) { }

public:
    [[nodiscard]] inline PageType pageType() const
    {
        return static_cast<PageType>(V->Page_case());
    }

    template<size_t N>
    [[nodiscard]] const std::optional<const Slice> flush(std::array<char, N> &AR) const
    {
        assert(V->ByteSizeLong() <= AR.size());
        if (!V->SerializeToArray(AR.data(), AR.size()))
            return {};
        return Slice(AR.data(), V->ByteSizeLong());
    }

    [[nodiscard]] inline uint32_t size() const
    {
        switch (pageType())
        {
            case PageType::StorageNumbersPage:
                return V->storage_numbers_page().storage_numbers_size();
            default:
                __builtin_unreachable();
        }
    }

    [[nodiscard]] inline const STORAGE_POINT get(uint32_t Pos, uint32_t PIT) const
    {
        switch (pageType())
        {
            case PageType::StorageNumbersPage:
            {
                auto &SNP = V->storage_numbers_page();
                assert(index < SNP.storage_numbers_size());
                storage_number SN = SNP.storage_numbers().Get(Pos);

                STORAGE_POINT SP;

                SP.min = SP.max = SP.sum = unpack_storage_number(SN);

                SP.start_time_s = PIT + (Pos * SNP.update_every());
                SP.end_time_s = SP.start_time_s + SNP.update_every();

                SP.count = 1;
                SP.anomaly_count = is_storage_number_anomalous(SN) ? 1 : 0;

                SP.flags = static_cast<SN_FLAGS>(SN & SN_USER_FLAGS);

                return SP;
            }
            default:
                __builtin_unreachable();
        }
    }

    [[nodiscard]] inline uint32_t duration() const
    {
        switch (pageType())
        {
            case PageType::StorageNumbersPage:
            {
                const StorageNumbersPage &SNP = V->storage_numbers_page();
                return SNP.storage_numbers_size() * SNP.update_every();
            }
            default:
                __builtin_unreachable();
        }
    }

    [[nodiscard]] inline PageIterator begin(uint32_t PIT = 0) const
    {
        return PageIterator(this, PIT, 0);
    }

    [[nodiscard]] inline PageIterator end() const
    {
        return PageIterator(this, 0, size());
    }

    inline void appendPoint(STORAGE_POINT &SP)
    {
        switch (pageType())
        {
            case PageType::StorageNumbersPage:
            {
                StorageNumbersPage *SNP = V->mutable_storage_numbers_page();
                pb::RepeatedField<uint32_t> *SNs = SNP->mutable_storage_numbers();

                storage_number SN = pack_storage_number(SP.sum, SP.flags);
                SNs->AddAlreadyReserved(SN);
                break;
            }
            default:
                __builtin_unreachable();
        }
    }

    inline void setUpdateEvery(uint32_t updateEvery)
    {
        switch (pageType())
        {
            case PageType::StorageNumbersPage:
                V->mutable_storage_numbers_page()->set_update_every(updateEvery);
                break;
            default:
                __builtin_unreachable();
        }
    }

    inline void reset()
    {
        switch (pageType())
        {
            case PageType::StorageNumbersPage:
            {
                StorageNumbersPage *SNP = V->mutable_storage_numbers_page();
                pb::RepeatedField<uint32_t> *SNs = SNP->mutable_storage_numbers();

                SNs->Clear();
                break;
            }
            default:
                __builtin_unreachable();
        }
    }

private:
    inline void reserve(PageType PT, uint32_t N)
    {
        switch (PT)
        {
            case PageType::StorageNumbersPage:
            {
                StorageNumbersPage *SNP = V->mutable_storage_numbers_page();
                SNP->mutable_storage_numbers()->Reserve(N);
                break;
            }
            default:
                __builtin_unreachable();
        }
    }

private:
    Value *V;
};

class CollectionPage
{
public:
    CollectionPage(Page MutablePage, uint32_t Slots)
        : MutablePage(MutablePage), Slots(Slots) { }

    inline void appendPoint(STORAGE_POINT &SP)
    {
        MutablePage.appendPoint(SP);
        Slots--;
    }

    inline void setUpdateEvery(uint32_t UE)
    {
        MutablePage.setUpdateEvery(UE);
    }

    inline void reset(uint32_t Slots)
    {
        MutablePage.reset();
        this->Slots = Slots;
    }

    [[nodiscard]] inline uint32_t duration() const
    {
        return MutablePage.duration();
    }

    [[nodiscard]] inline uint32_t size() const
    {
        return MutablePage.size();
    }

    [[nodiscard]] inline uint32_t capacity() const
    {
            return Slots;
    }

    [[nodiscard]] Page page() const
    {
        return MutablePage;
    }

private:
    Page MutablePage;
    uint32_t Slots;
};

template<typename GroupsRegistryT, typename MetricsRegistryT>
class StorageInstanceHandler
{
public:
    StorageInstanceHandler(size_t registry_shards) :
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
    UuidShard<GroupsRegistryT> GroupsRegistry;
    UuidShard<MetricsRegistryT> MetricsRegistry;

    std::mutex ArenasMutex;
    std::unordered_map<pid_t, pb::Arena *> Arenas;
};

} // namespace rdb

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

struct rdb_collect_handle
{
    // has to be first item
    struct storage_collect_handle common;

    // back-links to group/metric handles
    rdb_metrics_group *rmg;
    rdb_metric_handle *rmh;

    // collection data
    struct collection_data {
        // Can we make the lock per-thread?
        rdb::CollectionPage cp;
        usec_t pit_ut;
        usec_t update_every_ut;
        SPINLOCK lock;

        collection_data(rdb::CollectionPage &cp, uint32_t update_every)
            : cp(cp), pit_ut(0), update_every_ut(update_every * USEC_PER_SEC)
        {
            spinlock_init(&lock);
        }
    } cd;

    rdb_collect_handle(rdb_metrics_group *rmg,
                       rdb_metric_handle *rmh,
                       collection_data &cd)
        : common({ .backend = STORAGE_ENGINE_BACKEND_RDB }),
          rmg(rmg), rmh(reinterpret_cast<rdb_metric_handle *>(rdb_metric_dup(reinterpret_cast<STORAGE_METRIC_HANDLE *>(rmh)))), cd(cd)
    { }
};

namespace rdb {
    using StorageInstance = rdb::StorageInstanceHandler<rdb_metrics_group, rdb_metric_handle>;
}

extern rdb::StorageInstance *SI;

extern std::atomic<size_t> num_pages_written;

#endif /* RDB_PRIVATE_H */
