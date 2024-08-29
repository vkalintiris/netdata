#ifndef NETDATA_OTEL_CHART_HPP
#define NETDATA_OTEL_CHART_HPP

#include "otel_flatten.hpp"
#include "otel_utils.hpp"
#include "otel_config.hpp"
#include "otel_hash.hpp"
#include "otel_iterator.hpp"

#include "database/rrd.h"

#include <fstream>

namespace otel
{
class Chart {
public:
    Chart() : RS(nullptr), RDs(), LastCollectionTime(0)
    {
    }

    void update(
        const ScopeConfig *ScopeCfg,
        const pb::Metric &M,
        const std::string &BlakeId,
        const pb::RepeatedPtrField<pb::KeyValue> &Labels)
    {
        if (!LastCollectionTime) {
            LastCollectionTime = pb::findOldestCollectionTime(M) / NSEC_PER_SEC;
            return;
        }

        if (!RS) {
            createRS(ScopeCfg, M, BlakeId);
            setLabels(Labels);
        }

        updateRDs(M);
    }

    void setLabels(const pb::RepeatedPtrField<pb::KeyValue> &RPF)
    {
        if (RPF.empty())
            return;

        RRDLABELS *Labels = rrdlabels_create();

        for (const auto &KV : RPF) {
            const auto &K = KV.key();
            const auto &V = KV.value().string_value();

            rrdlabels_add(Labels, K.c_str(), V.c_str(), RRDLABEL_SRC_AUTO);
        }

        rrdset_update_rrdlabels(RS, Labels);
    }

private:
    std::string findDimensionName(const MetricConfig *MetricCfg, const pb::NumberDataPoint &DP);

    template <typename T> void createRDs(const MetricConfig *MetricCfg, bool Monotonic, const T &DPs);
    void createRDs(const MetricConfig *MetricCfg, const pb::Metric &M);

    void createRS(const ScopeConfig *ScopeCfg, const pb::Metric &M, const std::string &BlakeId);

    void updateRDs(const pb::Metric &M);
    template <typename T> void updateRDs(const pb::RepeatedPtrField<T> &DPs);

private:
    RRDSET *RS;
    std::vector<RRDDIM *> RDs;
    uint64_t LastCollectionTime;
};

} // namespace otel

#endif /* NETDATA_OTEL_CHART_HPP */
