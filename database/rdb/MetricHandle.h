#ifndef RDB_METRIC_HANDLE_H
#define RDB_METRIC_HANDLE_H

#include "rdb-common.h"
#include "Intervals.h"

namespace rdb {
    
class MetricHandle
{
public:
    [[nodiscard]] static inline MetricHandle fromIDs(uint32_t gid, uint32_t mid)
    {
        MetricHandle MH;

        MH.group_id = gid;
        MH.metric_id = mid;

        return MH;
    }

    [[nodiscard]] static inline std::optional<MetricHandle> deserialize(const Slice &S)
    {
        rdbv::MetricHandle V;

        if (!V.ParseFromArray(S.data(), S.size()))
            return std::nullopt;

        return fromIDs(V.group_id(), V.metric_id());
    }

    [[nodiscard]] inline uint32_t gid() const
    {
        return group_id;
    }
    
    [[nodiscard]] inline uint32_t mid() const
    {
        return metric_id;
    }
    
    template<size_t N> [[nodiscard]] const std::optional<const rocksdb::Slice> serialize(std::array<char, N> &AR) const
    {
        rdbv::MetricHandle V;

        V.set_group_id(group_id);
        V.set_metric_id(metric_id);

        assert(V.ByteSizeLong() <= AR.size());

        if (!V.SerializeToArray(AR.data(), AR.size()))
            return std::nullopt;

        return rocksdb::Slice(AR.data(), V.ByteSizeLong());
    }

private:
    uint32_t group_id;
    uint32_t metric_id;
};

} // namespace rdb

#endif /* RDB_METRIC_HANDLE_H */
