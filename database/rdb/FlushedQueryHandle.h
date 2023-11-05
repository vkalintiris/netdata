#ifndef RDB_FLUSHED_QUERY_HANDLE_H
#define RDB_FLUSHED_QUERY_HANDLE_H

#include "rdb-common.h"
#include "Key.h"
#include "Page.h"
#include "StorageInstance.h"
#include "CollectionHandle.h"

namespace rdb
{

class FlushedQueryHandle
{
    friend class UniversalQuery;

public:
    FlushedQueryHandle(const Key &AfterK)
        : AfterK(AfterK), OP(), Finished(false) { }

    [[nodiscard]] inline bool isFinished(pb::Arena &Arena, rocksdb::Iterator &It)
    {
        if (!Finished)
        {
            if (OP.has_value() && (OP->first != OP->second))
                return false;

            Finished = !advance(Arena, It);
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
    }

private:
    [[nodiscard]] bool advance(pb::Arena &Arena, rocksdb::Iterator &It)
    {
        // We can not advance an invalid iterator
        if (!It.Valid())
            return false;

        while (It.Valid())
        {
            // Any old pages have been consumed. Reclaim space before
            // creating a new one to keep memory consumption low.
            Arena.Reset();

            Key K = Key(It.key());
            if (K.mid() != AfterK.mid())
                return false;

            std::optional<Page> P = Page::deserialize(Arena, It.value());

            if (P.has_value())
            {
                OP = P->query(K.pit(), AfterK.pit());
                if (OP.has_value())
                {
                    It.Next();
                    return true;
                }
            }

            It.Next();
        }

        return false;
    }

private:
    const Key &AfterK;
    std::optional<std::pair<Page::PageIterator, Page::PageIterator>> OP;
    bool Finished;
};

class CollectionQueryHandle
{
    friend class UniversalQuery;

public:
    CollectionQueryHandle(CollectionHandle *CH, const Key &AfterK) :
        CH(CH),
        OP(CH ? CH->queryLock(AfterK.pit() * USEC_PER_SEC) : std::nullopt),
        Finished(!OP.has_value())
    { }

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
    UniversalQuery(CollectionHandle *CH, const Key &AfterK)
        : AfterK(AfterK), FQH(this->AfterK), CQH(CH, this->AfterK) { }

    [[nodiscard]] inline bool isFinished(pb::Arena &Arena, rocksdb::Iterator &It)
    {
        return FQH.isFinished(Arena, It) && CQH.isFinished();
    }

    [[nodiscard]] inline STORAGE_POINT next()
    {
        if (!FQH.Finished)
            return FQH.next();

        if (!CQH.Finished)
            return CQH.next();

        fatal("Tried to get storage point from finished query.");
    }

    void finalize()
    {
        FQH.finalize();
        CQH.finalize();
    }

private:
    const Key AfterK;
    FlushedQueryHandle FQH;
    CollectionQueryHandle CQH;
};

} // namespace rdb

#endif /* RDB_FLUSHED_QUERY_HANDLE_H */
