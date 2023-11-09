#ifndef RDB_FLUSHED_QUERY_HANDLE_H
#define RDB_FLUSHED_QUERY_HANDLE_H

#include "rdb-common.h"
#include "Key.h"
#include "Page.h"
#include "StorageInstance.h"
#include "CollectionHandle.h"

namespace rdb
{

class MetricHandleQuery
{
    friend class UniversalQuery;
    
public:
    MetricHandleQuery(const MetricHandle *MH, uint32_t After, uint32_t Before)
        : MH(MH), After(After), Before(Before), Now(After),
          Keys(MH->intervalManager().getKeys<16>(After, Before)),
          It(Keys.begin()), OP(), Finished(false)
    { }

    [[nodiscard]] inline bool isFinished(pb::Arena &Arena)
    {
        if (!Finished)
        {
            if (OP.has_value() && (OP->first != OP->second))
                return false;

            Finished = (Now >= Before) || !advance(Arena);
        }

        return Finished;
    }

    [[nodiscard]] inline STORAGE_POINT next()
    {
        if (OP->first == OP->second)
            fatal("PageIterator already consumed");

        Now++;
        return *OP->first++;
    }

    inline void finalize() const
    {
    }

private:
    [[nodiscard]] bool advance(pb::Arena &Arena)
    {
        while (It != Keys.end())
        {
            // Any old pages have been consumed. Reclaim space before
            // creating a new one to keep memory consumption low.
            Arena.Reset();

            uint32_t PIT = *It;
            Key K = Key(MH->gid(), MH->mid(), PIT);
            rocksdb::PinnableSlice PSV;

            // Use a MultiGet
            Status S = SI->getMD(K.slice(), &PSV);
            if (S.ok())
            {
                std::optional<Page> P = Page::deserialize(Arena, PSV);

                if (P.has_value())
                {
                    OP = P->query(K.pit(), Now);
                    if (OP.has_value())
                    {
                        It++;
                        return true;
                    }
                }
            }

            It++;
        }

        return false;
    }

private:
    const MetricHandle *MH;
    uint32_t After;
    uint32_t Before;
    uint32_t Now;

    absl::InlinedVector<uint32_t, 16> Keys;
    absl::InlinedVector<uint32_t, 16>::iterator It;

    std::optional<std::pair<Page::PageIterator, Page::PageIterator>> OP;

    bool Finished;
};

class CollectionQueryHandle
{
    friend class UniversalQuery;

public:
    CollectionQueryHandle(CollectionHandle *CH, usec_t After)
        : CH(CH),
          OP(CH ? CH->queryLock(After) : std::nullopt),
          Finished(!OP.has_value())
    { }

    [[nodiscard]] inline bool isFinished()
    {
        if (!Finished)
        {
            if (!OP.has_value())
                return true;

            Finished = OP->first == OP->second;
        }

        return Finished;
    }

    [[nodiscard]] inline STORAGE_POINT next()
    {
        if (OP->first == OP->second)
            fatal("PageIterator already consumed");

        return *OP->first++;
    }

    inline void finalize()
    {
        if (CH)
            CH->queryUnlock();
    }

private:
    CollectionHandle *CH;
    std::optional<std::pair<Page::PageIterator, Page::PageIterator>> OP;
    bool Finished;
};

class UniversalQuery
{
public:
    UniversalQuery(MetricHandle *MH, CollectionHandle *CH, uint32_t After, uint32_t Before)
        : MQH(MH, After, Before), CQH(CH, After)
    { }

    [[nodiscard]] inline bool isFinished(pb::Arena &Arena)
    {
        return MQH.isFinished(Arena) && CQH.isFinished();
    }

    [[nodiscard]] inline STORAGE_POINT next()
    {
        if (!MQH.Finished)
            return MQH.next();

        if (!CQH.Finished)
            return CQH.next();

        fatal("Tried to get storage point from finished query.");
    }

    void finalize()
    {
        MQH.finalize();
        CQH.finalize();
    }

private:
    MetricHandleQuery MQH;
    CollectionQueryHandle CQH;
};

} // namespace rdb

#endif /* RDB_FLUSHED_QUERY_HANDLE_H */
