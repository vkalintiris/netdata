#ifndef RDB_COLLECTION_PAGE_H
#define RDB_COLLECTION_PAGE_H

#include "rdb-common.h"
#include "Page.h"

namespace rdb {

class CollectionPage
{
public:
    CollectionPage(const Page &P, const PageOptions &PO)
        : Inner(P), Slots(PO.initial_slots) { }

    inline void appendPoint(const STORAGE_POINT &SP)
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

    [[nodiscard]] inline PageType pageType() const {
        return Inner.pageType();
    }

    [[nodiscard]] inline uint32_t updateEvery() const
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

    template<size_t N>
    [[nodiscard]] const std::optional<const Slice> flush(std::array<char, N> &AR) const
    {
        return Inner.flush(AR);
    }

    [[nodiscard]] inline
    std::optional<std::pair<Page::PageIterator, Page::PageIterator>>
    query(uint32_t StartPIT, uint32_t After) const
    {
        return Inner.query(StartPIT, After);
    }

private:
    Page Inner;
    uint32_t Slots;
};

} // namespace rdb

#endif /* RDB_COLLECTION_PAGE_H */
