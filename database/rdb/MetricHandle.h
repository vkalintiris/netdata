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
    MetricHandle(uint32_t GID, uint32_t MID) : GID(GID), MID(MID), IM() { }

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
};

} // namespace rdb

#endif /* RDB_METRIC_HANDLE_H */
