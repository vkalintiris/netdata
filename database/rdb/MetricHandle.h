#ifndef RDB_METRIC_HANDLE_H
#define RDB_METRIC_HANDLE_H

#include "database/rdb/Key.h"
#include "rdb-common.h"
#include "Intervals.h"

namespace rdb {

class MetricHandle
{
private:
    MetricHandle() = delete;

public:
    MetricHandle(uint32_t GID, uint32_t MID) : GID(GID), MID(MID), IM()
    {
        spinlock_init(&Lock);
    }

    MetricHandle(uint32_t GID, uint32_t MID, IntervalManager<1024> &&IM)
        : GID(GID), MID(MID), IM(IM)
    {
        spinlock_init(&Lock);
    }

    [[nodiscard]] static inline MetricHandle fromKey(const MetricKey &MK)
    {
        return MetricHandle(MK.gid(), MK.mid());
    }

    [[nodiscard]] inline uint32_t gid() const
    {
        return GID;
    }

    [[nodiscard]] inline uint32_t mid() const
    {
        return MID;
    }

    [[nodiscard]] inline const IntervalManager<1024>& intervalManager()
    {
        return IM;
    }

    inline void addInterval(uint32_t PIT, uint32_t Slots, uint32_t UpdateEvery)
    {
        spinlock_lock(&Lock);
        IM.addInterval(PIT, Slots, UpdateEvery);
        spinlock_unlock(&Lock);
    }

    [[nodiscard]] std::optional<uint32_t> after() const
    {
        spinlock_lock(&Lock);
        std::optional<uint32_t> After = IM.after();
        spinlock_unlock(&Lock);
        return After;
    }

    [[nodiscard]] std::optional<uint32_t> before() const
    {
        spinlock_lock(&Lock);
        std::optional<uint32_t> Before = IM.before();
        spinlock_unlock(&Lock);
        return Before;
    }

    template<size_t N> [[nodiscard]] const std::optional<const rocksdb::Slice> serialize(std::array<char, N> &AR) const
    {
        rdbv::MetricHandle V;

        V.set_group_id(GID);
        V.set_metric_id(MID);

        assert(V.ByteSizeLong() <= AR.size());

        if (!V.SerializeToArray(AR.data(), AR.size()))
            return std::nullopt;

        return rocksdb::Slice(AR.data(), V.ByteSizeLong());
    }

private:
    uint32_t GID;
    uint32_t MID;
    IntervalManager<1024> IM;
    mutable SPINLOCK Lock;
};

} // namespace rdb

#endif /* RDB_METRIC_HANDLE_H */
