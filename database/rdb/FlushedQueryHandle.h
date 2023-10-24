#ifndef RDB_FLUSHED_QUERY_HANDLE_H
#define RDB_FLUSHED_QUERY_HANDLE_H

#include "rdb-common.h"
#include "Key.h"
#include "Page.h"
#include "StorageInstance.h"

namespace rdb
{

class FlushedQueryHandle
{
public:
    FlushedQueryHandle(const Key &StartK)
        : StartK(StartK) { }

    bool isFinished(pb::Arena &Arena, rocksdb::Iterator &It)
    {
        if (OP.has_value() && (OP->first != OP->second))
            return false;

        return !advance(Arena, It);
    }

    STORAGE_POINT next()
    {
        if (OP->first == OP->second)
            fatal("PageIterator already consumed");

        return *OP->first++;
    }

private:
    bool advance(pb::Arena &Arena, rocksdb::Iterator &It)
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
            std::optional<Page> P = Page::fromSlice(Arena, It.value());

            if (P.has_value())
            {
                OP = P->query(K.pit(), StartK.pit());
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
    Key StartK;
    std::optional<std::pair<Page::PageIterator, Page::PageIterator>> OP;
};

} // namespace rdb

#endif /* RDB_FLUSHED_QUERY_HANDLE_H */
