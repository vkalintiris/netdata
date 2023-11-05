#ifndef RDB_PAGE_H
#define RDB_PAGE_H

#include "rdb-common.h"

namespace rdb {

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
        PageIterator(Value *V, const uint32_t PIT, const uint32_t Pos)
            : V(V), PIT(PIT), Pos(Pos) { }

    public:
        [[nodiscard]] static PageIterator create(Value *V,
                                                 uint32_t Pos,
                                                 uint32_t PIT)
        {
            return PageIterator(V, Pos, PIT);
        }

        bool operator==(const PageIterator& Other) const
        {
            // We intentionaly ignore PIT to simplify the begin()/end() API.
            return (V == Other.V) && (Pos == Other.Pos);
        }

        bool operator!=(const PageIterator& Other) const
        {
            return !(*this == Other);
        }

        inline value_type operator*() const
        {
            return Page(V).get(Pos, PIT);
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
            return Page(V).get(Pos + N, PIT);
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
        Value *V;
        uint32_t PIT;
        uint32_t Pos;
    };

public:
    [[nodiscard]] static std::optional<const Page> deserialize(pb::Arena &Arena, const Slice &S)
    {
        Value *V = pb::Arena::CreateMessage<Value>(&Arena);
        if (!V)
            return std::nullopt;

        if (!V->ParseFromArray(S.data(), S.size()))
            return std::nullopt;

        return Page(V);
    }

    [[nodiscard]] static std::optional<Page> create(pb::Arena &Arena, const PageOptions &PO)
    {
        Value *V = pb::Arena::CreateMessage<Value>(&Arena);
        if (!V)
            return std::nullopt;

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

    template<size_t N> [[nodiscard]] const std::optional<const Slice> serialize(std::array<char, N> &AR) const
    {
        assert(V->ByteSizeLong() <= AR.size());

        if (!V->SerializeToArray(AR.data(), AR.size()))
            return std::nullopt;

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
        return PageIterator::create(V, PIT, 0);
    }

    [[nodiscard]] inline PageIterator end() const
    {
        return PageIterator(V, 0, size());
    }

    // The iterator will return all SPs with an QH->after() >= After
    [[nodiscard]] std::optional<std::pair<Page::PageIterator, Page::PageIterator>>
    query(uint32_t StartPIT, uint32_t After) const
    {
        if (After == 0)
            return std::nullopt;

        if (After >= StartPIT + duration())
            return std::nullopt;

        if (After % updateEvery())
            After -= After % updateEvery();

        Page::PageIterator It = begin(StartPIT);

        After = std::max(After, StartPIT);
        usec_t Skip = (After - StartPIT) / updateEvery();
        std::advance(It, Skip);

        if (It == end())
            return std::nullopt;

        return { { It, end() } };
    }

    inline void appendPoint(const STORAGE_POINT &SP)
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

} // namespace rdb

#endif /* RDB_PAGE_H */
