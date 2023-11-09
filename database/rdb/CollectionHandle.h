#ifndef RDB_COLLECTION_HANDLE_H
#define RDB_COLLECTION_HANDLE_H

#include "rdb-common.h"
#include "Key.h"
#include "Page.h"
#include "CollectionPage.h"
#include "StorageInstance.h"

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
    CollectionHandle(uint32_t GID, uint32_t MID, CollectionPage &CP)
        : GID(GID), MID(MID),
          CurrPIT(0), UE(CP.updateEvery() * USEC_PER_SEC),
          CP(CP), OldestKey(std::nullopt)
    {
        spinlock_init(&Lock);
    }

    void store_next_internal(MetricHandle &MH, usec_t PIT, const STORAGE_POINT &SP)
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
                flush_internal(MH, false);
                fatal("Ask @ktsaou: should we ignore the point or change the collection frequency?");
                CurrPIT = PIT - Delta;
                UE = Delta;
            }
            else if (Delta % UE)
            {
                // step is unaligned
                flush_internal(MH, false);
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
                    flush_internal(MH, false);
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
                        store_next(MH, ThisPIT, EmptySP);

                        spinlock_lock(&Lock);
                    }
                }
            }

            spinlock_unlock(&Lock);
            store_next(MH, PIT, SP);
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

    inline void flush_internal(MetricHandle &MH, bool Protect)
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

        std::optional<const Slice> OV = CP.serialize(bytes);
        if (!OV.has_value())
        {
            fatal("Failed to serialize page...");
        }

        Status S = SI->putMD(K.slice(), OV.value());
        if (!S.ok())
        {
            fatal("Failed to put key %s (%s)", K.toString(true).c_str(), S.ToString().c_str());
        }

        MH.addInterval(StartPIT, CP.size(), CP.updateEvery());
        S = SI->setIntervalManager(MH.gid(), MH.mid(), MH.intervalManager());
        if (!S.ok())
        {
            fatal("Failed to set IM for MH(%u, %u) - %s", MH.gid(), MH.mid(), S.ToString().c_str());
        }

        // TODO: make 1024 an SI constant
        CP.reset(1024);

        if (Protect)
        {
            spinlock_unlock(&Lock);
        }

        global_statistics_rdb_flushed_pages_incr();
        NumFlushedPages++;
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
    inline void store_next(MetricHandle &MH, usec_t PIT, const STORAGE_POINT &SP)
    {
        spinlock_lock(&Lock);

        if (unlikely(CP.capacity() == 0))
        {
            flush_internal(MH, false);
        }

        usec_t Delta = PIT - this->CurrPIT;

        if (unlikely(Delta != UE))
        {
            spinlock_unlock(&Lock);
            store_next_internal(MH, PIT, SP);
            return;
        }

        CP.appendPoint(SP);
        this->CurrPIT += UE;

        spinlock_unlock(&Lock);
    }

    inline void flush(MetricHandle &MH)
    {
        flush_internal(MH, true);
    }

    inline void setUpdateEvery(MetricHandle &MH, usec_t UE)
    {
        spinlock_lock(&Lock);

        flush_internal(MH, false);

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

    [[nodiscard]] inline std::optional<std::pair<Page::PageIterator, Page::PageIterator>>
    queryLock(usec_t After) const
    {
        spinlock_lock(&Lock);
        return CP.query(after_internal(false) / USEC_PER_SEC, After / USEC_PER_SEC);
    }

    [[nodiscard]] inline std::optional<std::pair<Page::PageIterator, Page::PageIterator>>
    queryLock(const Key &StartK) const
    {
        return queryLock(StartK.pit() * USEC_PER_SEC);
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
    std::optional<Key> OldestKey;
};

} // namespace rdb

struct rdb_collect_handle
{
    // has to be first item
    struct storage_collect_handle common;

    // collection data
    rdb::CollectionHandle ch;

    rdb_collect_handle(rdb::CollectionHandle &CH)
        : common({ .backend = STORAGE_ENGINE_BACKEND_RDB }), ch(CH)
    { }
};

#endif /* RDB_COLLECTION_HANDLE_H */
