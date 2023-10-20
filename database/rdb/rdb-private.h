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
    [[nodiscard]] inline uint32_t field(size_t i) const
    {
        assert(i < 3);

        uint32_t f;
        memcpy(&f, &Scratch[i * sizeof(uint32_t)], sizeof(uint32_t));
        return be32toh(f);
    }

public:
    static const Key min()
    {
        return Key(0, 0, 0);
    }
        
    static const Key max()
    {
        uint32_t m = std::numeric_limits<uint32_t>::max();
        return Key(m, m, m);
    }

    inline Key() = default;

    inline Key(uint32_t gid, uint32_t mid, uint32_t pit)
    {
        gid = htobe32(gid);
        mid = htobe32(mid);
        pit = htobe32(pit);

        memcpy(&Scratch[GroupIdField * sizeof(uint32_t)], &gid, sizeof(uint32_t));
        memcpy(&Scratch[MetricIdField * sizeof(uint32_t)], &mid, sizeof(uint32_t));
        memcpy(&Scratch[PointInTimeField * sizeof(uint32_t)], &pit, sizeof(uint32_t));
    }

    inline Key(const Slice &S)
    {
        memcpy(&Scratch[0], S.data(), rdb::Key::Bytes);
    }

    inline Key(const std::array<char, 12> &AR)
    {
        memcpy(&Scratch[0], AR.data(), AR.size());
    }

    [[nodiscard]] inline const Slice slice() const
    {
        return Slice(Scratch.data(), Scratch.size());
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
            snprintfz(buf.data(), buf.size() - 1, "gid=%u, mid=%u, pit=%u (0x%s)",
                      gid(), mid(), pit(), slice().ToString(true).c_str());
        }
        else
        {
            snprintfz(buf.data(), buf.size() - 1, "gid=%u, mid=%u, pit=%u",
                      gid(), mid(), pit());
        }

        return std::string(buf.data()); 
    }

private:
    std::array<char, Key::Bytes> Scratch;
};

enum class PageType : uint8_t
{
    StorageNumbersPage = Value::PageCase::kStorageNumbersPage,
};

struct PageOptions
{
    PageType page_type = PageType::StorageNumbersPage;
    uint32_t capacity = 1024;
    uint32_t initial_slots = 1024;
    uint32_t update_every = 1;

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

        inline PageIterator& operator--()
        {
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
        P.setUpdateEvery(PO.update_every);
        return P;
    }

private:
    Page(Value *V) : V(V) { }

public:
    [[nodiscard]] inline PageType pageType() const
    {
        return static_cast<PageType>(V->Page_case());
    }

    template<size_t N> [[nodiscard]] const std::optional<const Slice> flush(std::array<char, N> &AR) const
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
                fatal("Page: Tsimpa[1]");
        }
    }

    [[nodiscard]] inline const STORAGE_POINT get(uint32_t Pos, uint32_t PIT) const
    {
        switch (pageType())
        {
            case PageType::StorageNumbersPage:
            {
                auto &SNP = V->storage_numbers_page();
                assert(Pos < SNP.storage_numbers_size());
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
                fatal("Page: Tsimpa[2]");
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
                fatal("Page: Tsimpa[3]");
        }
    }

    [[nodiscard]] inline uint32_t updateEvery() const
    {
        switch (pageType())
        {
            case PageType::StorageNumbersPage:
            {
                const StorageNumbersPage &SNP = V->storage_numbers_page();
                return SNP.update_every();
            }
            default:
                fatal("Page: Tsimpa[4]");
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

    // The iterator will return all SPs with an QH->after() >= After
    [[nodiscard]] std::optional<std::pair<Page::PageIterator, Page::PageIterator>>
    query(uint32_t StartPIT, uint32_t After) const
    {
        if (After == 0)
            return {};

        if (After >= StartPIT + duration())
            return {};

        if (After % updateEvery())
            After -= After % updateEvery();

        Page::PageIterator It = begin(StartPIT);

        After = std::max(After, StartPIT);
        usec_t Skip = (After - StartPIT) / updateEvery();
        std::advance(It, Skip);

        return { { It, end() } };
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
                fatal("Page: Tsimpa[5]");
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
                fatal("Page: Tsimpa[6]");
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
                fatal("Page: Tsimpa[7]");
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
                fatal("Page: Tsimpa[8]");
        }
    }

private:
    Value *V;
};

class CollectionPage
{
public:
    CollectionPage(Page P, const PageOptions &PO)
        : Inner(P), Slots(PO.initial_slots) { }

    inline void appendPoint(STORAGE_POINT &SP)
    {
        Inner.appendPoint(SP);
        Slots--;
    }

    inline void setUpdateEvery(uint32_t UE)
    {
        Inner.setUpdateEvery(UE);
    }

    inline void reset(uint32_t Slots)
    {
        Inner.reset();
        this->Slots = Slots;
    }

    [[nodiscard]] inline uint32_t getUpdateEvery() const
    {
        return Inner.updateEvery();
    }

    [[nodiscard]] inline uint32_t duration() const
    {
        return Inner.duration();
    }

    [[nodiscard]] inline uint32_t size() const
    {
        return Inner.size();
    }

    [[nodiscard]] inline uint32_t capacity() const
    {
        return Slots;
    }

    [[nodiscard]] const Page *page() const
    {
        return &Inner;
    }

private:
    Page Inner;
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

namespace rdb {
    using StorageInstance = rdb::StorageInstanceHandler<rdb_metrics_group, rdb_metric_handle>;
}

extern rdb::StorageInstance *SI;
extern std::atomic<size_t> num_pages_written;

namespace rdb {
    
class CollectionHandle
{
public:
    static std::optional<CollectionHandle> create(pb::Arena &Arena, const PageOptions &PO,
                                                  uint32_t GID, uint32_t MID)
    {
        std::optional<rdb::Page> OP = rdb::Page::create(Arena, PO);
        if (!OP.has_value())
            return {};

        CollectionPage CP = CollectionPage(OP.value(), PO);
        return CollectionHandle(GID, MID, CP);
    }

private:
    CollectionHandle(uint32_t GID, uint32_t MID,
                     CollectionPage &CP)
        : GID(GID), MID(MID),
          CurrPIT(0), UE(CP.getUpdateEvery() * USEC_PER_SEC),
          CP(CP)
    {
        spinlock_init(&Lock);        
    }

    void store_next_internal(usec_t PIT, STORAGE_POINT &SP)
    {
        spinlock_lock(&Lock);

        // this might be the first time we are saving something in the collection handle.
        if (CurrPIT == 0)
        {
            CP.appendPoint(SP);
            CurrPIT = PIT;
            spinlock_unlock(&Lock);
            return;
        }

        if (CurrPIT < PIT)
        {
            // point_in_time is in the future
            usec_t Delta = PIT - CurrPIT;

            if (Delta < UE)
            {
                // step is too small
                flush_internal(false);
                fatal("Ask @ktsaou: should we ignore the point or change the collection frequency?");
                CurrPIT = PIT - Delta;
                UE = Delta;
            }
            else if (Delta % UE)
            {
                // step is unaligned
                flush_internal(false);
                fatal("Ask @ktsaou: should we ignore the point or change the collection frequency?");
                CurrPIT = PIT - Delta;
                UE = Delta;
            }
            else
            {
                // aligned but in the future
                size_t PointsInGap = Delta / UE;

                if (PointsInGap >= CP.capacity())
                {
                    // we can't store any points in the current page
                    flush_internal(false);
                    CurrPIT = PIT - UE;
                }
                else
                {
                    // fill gaps in the current page
                    usec_t StopPIT = PIT - UE;

                    for (usec_t ThisPIT = (CurrPIT + UE);
                         ThisPIT <= StopPIT;
                         ThisPIT = (CurrPIT + UE))
                    {
                        spinlock_unlock(&Lock);

                        STORAGE_POINT EmptySP = {
                            .min = NAN,
                            .max = NAN,
                            .sum = NAN,

                            .start_time_s = 0,
                            .end_time_s = 0,

                            .count = 1,
                            .anomaly_count = 0,

                            .flags = SN_EMPTY_SLOT,
                        };
                        store_next(ThisPIT, EmptySP);

                        spinlock_lock(&Lock);
                    }
                }
            }

            spinlock_unlock(&Lock);
            store_next(PIT, SP);
            return;
        }
        else if (CurrPIT > PIT)
        {
            // point_in_time is in the past, nothing to do
            spinlock_unlock(&Lock);
            return;
        }
        else if (CurrPIT == PIT)
        {
            // point_in_time has already been saved, nothing to do
            spinlock_unlock(&Lock);
            return;
        }
        else
        {
            fatal("WTF?");
        }
    }
        
    inline void flush_internal(bool Protect)
    {
        if (Protect)
        {
            spinlock_lock(&Lock);
        }

        if (!CP.duration())
        {
            if (Protect)
            {
                spinlock_unlock(&Lock);
            }

            return;
        }

        uint32_t StartPIT = after_internal(false) / USEC_PER_SEC;

        const Key K{GID, MID, StartPIT};

        // TODO: the max size should be 4096 + 6 bytes. is there
        // any performance difference if the bytes array has exact size?
        // ie. are we hitting hot vs. cold memory on serialization?
        std::array<char, 64 * 1024> bytes;

        std::optional<const Slice> OV = CP.page()->flush(bytes);
        if (!OV.has_value())
        {
            fatal("Failed to flush page...");
        }

        // TODO: make 1024 an SI constant
        CP.reset(1024);

        if (Protect)
        {
            spinlock_unlock(&Lock);
        }

        rocksdb::WriteOptions WO;
        WO.disableWAL = true;
        WO.sync = false;
        SI->RDB->Put(WO, K.slice(), OV.value());

        num_pages_written++;
    }

    [[nodiscard]] inline usec_t after_internal(bool Protect) const
    {
        usec_t After = 0;

        if (Protect)
        {
            spinlock_lock(&Lock);
        }

        if (CurrPIT)
            After = CurrPIT - (CP.duration() * USEC_PER_SEC) + UE;

        if (Protect)
        {
            spinlock_unlock(&Lock);
        }

        return After;
    }

    [[nodiscard]] inline usec_t before_internal(bool Protect) const
    {
        usec_t Before = 0;
            
        if (Protect)
        {
            spinlock_lock(&Lock);
        }

        if (CurrPIT)
            Before = CurrPIT + UE;

        if (Protect)
        {
            spinlock_unlock(&Lock);
        }

        return Before;
    }

public:
    inline void store_next(usec_t PIT, STORAGE_POINT &SP)
    {
        spinlock_lock(&Lock);

        if (unlikely(CP.capacity() == 0))
        {
            flush_internal(false);
        }

        usec_t Delta = PIT - this->CurrPIT;

        if (unlikely(Delta != UE))
        {
            spinlock_unlock(&Lock);
            store_next_internal(PIT, SP);
            return;
        }

        CP.appendPoint(SP);
        this->CurrPIT += UE;

        switch (CP.page()->pageType())
        {
            case PageType::StorageNumbersPage:
                break;
            default:
                fatal("Bad page type");
        }
        spinlock_unlock(&Lock);
    }

    inline void flush()
    {
        flush_internal(true);
    }

    inline void setUpdateEvery(usec_t UE)
    {
        spinlock_lock(&Lock);

        flush_internal(false);

        CP.setUpdateEvery(UE / USEC_PER_SEC);
        this->UE = UE;

        spinlock_unlock(&Lock);
    }

    [[nodiscard]] inline usec_t after() const
    {
        return after_internal(true);
    }

    [[nodiscard]] inline usec_t before() const
    {
        return before_internal(true);
    }

    [[nodiscard]] inline usec_t duration() const
    {
        spinlock_lock(&Lock);

        usec_t D = before_internal(false) - after_internal(false);

        spinlock_unlock(&Lock);

        return D;
    }

    // The iterator will return all SPs with an QH->after() >= After
    [[nodiscard]] std::optional<std::pair<Page::PageIterator, Page::PageIterator>>
    queryLock(usec_t After) const
    {
        spinlock_lock(&Lock);

        const Page *P = CP.page();
        return P->query(after_internal(false) / USEC_PER_SEC, After / USEC_PER_SEC);
    }

    inline void queryUnlock() const
    {
        spinlock_unlock(&Lock);
    }

private:
    uint32_t GID;
    uint32_t MID;
    usec_t CurrPIT;
    usec_t UE;
    CollectionPage CP;
    mutable SPINLOCK Lock;
};

} // namespace rdb

struct rdb_collect_handle
{
    // has to be first item
    struct storage_collect_handle common;

    // collection data
    rdb::CollectionHandle ch;

    rdb_collect_handle(rdb::CollectionHandle &CH)
        : common({ .backend = STORAGE_ENGINE_BACKEND_RDB }), ch(CH) { }
};

#endif /* RDB_PRIVATE_H */
