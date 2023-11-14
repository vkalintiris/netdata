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
        : MH(MH), After(After),
          Keys(MH->intervalManager().getKeys<16>(After, Before)),
          It(Keys.begin()), OP(), Finished(false)
    { }

    [[nodiscard]] inline bool isFinished(pb::Arena &Arena)
    {
        if (!Finished)
        {
            if (OP.has_value() && (OP->first != OP->second))
                return false;

            Finished = !advance(Arena);
        }

        return Finished;
    }

    [[nodiscard]] inline STORAGE_POINT next()
    {
        if (OP->first == OP->second)
            fatal("PageIterator already consumed");

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
                    OP = P->query(K.pit(), std::max(After, K.pit()));
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

    absl::InlinedVector<uint32_t, 16> Keys;
    absl::InlinedVector<uint32_t, 16>::iterator It;

    std::optional<std::pair<Page::PageIterator, Page::PageIterator>> OP;

    bool Finished;
};

class CollectionQueryHandle
{
    friend class UniversalQuery;

public:
    CollectionQueryHandle(CollectionHandle *CH, uint32_t After, uint32_t Before)
        : CH(CH),
          OP(std::nullopt),
          Finished(!OP.has_value())
    {
        if (CH)
        {
            usec_t AfterCH = CH->after();
            if (AfterCH >= (After * USEC_PER_SEC))
            {
                if (AfterCH < (Before * USEC_PER_SEC))
                {
                    fatal("BBBBBBBBBBBBBBBBBBBB");
                    OP = CH->queryLock(After * USEC_PER_SEC);
                }
            }
        }
        
    }

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
        if (OP.has_value())
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
        : After(After), Before(Before), Now(After),
          // MQH(MH, After, Before), CQH(CH, After, Before)
          CQH(CH, After, Before)
    { }

    [[nodiscard]] inline bool isFinished(pb::Arena &Arena)
    {
        if (Now >= Before) {
            netdata_log_error("UQ: is finished!");
            return true;
        }

        // return MQH.isFinished(Arena) && CQH.isFinished();
        bool rc = CQH.isFinished();
        if (rc) {
            netdata_log_error("CQH: is finished!");
        } else {
            netdata_log_error("CQH: is not finished!");
        }

        return rc;
    }

    [[nodiscard]] inline STORAGE_POINT next()
    {
        STORAGE_POINT SP;

        // if (!MQH.Finished) {
        //     SP = MQH.next();
        //     Now = SP.end_time_s;
        //     return SP;
        // }

        if (!CQH.Finished) {
            SP = CQH.next();
            Now = SP.end_time_s;

            netdata_log_error("CQH returned next point!");
            return SP;
        }

        fatal("Tried to get storage point from finished query.");
    }

    void finalize()
    {
        // MQH.finalize();
        netdata_log_error("CQH is being finalized!");
        CQH.finalize();
    }

private:
    uint32_t After;
    uint32_t Before;
    uint32_t Now;

    // MetricHandleQuery MQH;
    CollectionQueryHandle CQH;
};

} // namespace rdb

#endif /* RDB_FLUSHED_QUERY_HANDLE_H */
