#ifndef NETDATA_OTEL_PROCESS_HPP
#define NETDATA_OTEL_PROCESS_HPP

#include "otel_utils.hpp"
#include "otel_config.hpp"
#include "otel_hash.hpp"

#include "database/rrd.h"

namespace otel {

class Chart {
public:
    Chart() : RS(nullptr), RDs(), LastCollectionTime(0)
    {
    }

    void update(const ScopeConfig *ScopeCfg, const pb::Metric &M, const std::string &BlakeId)
    {
        if (!LastCollectionTime) {
            LastCollectionTime = pb::findOldestCollectionTime(M) / NSEC_PER_SEC;
            return;
        }

        if (!RS) {
            createRS(ScopeCfg, M, BlakeId);
        }

        return;

        updateRDs(M);
    }

private:
    std::string findDimensionName(const MetricConfig *MetricCfg, const pb::NumberDataPoint &DP);

    template <typename T> void createRDs(const MetricConfig *MetricCfg, const T &DPs);
    void createRDs(const MetricConfig *MetricCfg, const pb::Metric &M);

    void createRS(const ScopeConfig *ScopeCfg, const pb::Metric &M, const std::string &BlakeId);

    void updateRDs(const pb::Metric &M);
    template <typename T> void updateRDs(const pb::RepeatedPtrField<T> &DPs);

private:
    RRDSET *RS;
    std::vector<RRDDIM *> RDs;
    uint64_t LastCollectionTime;
};

class MetricProcessor {
public:
    void processMetricsData(const Config *Cfg, const pb::MetricsData *MD);

private:
    std::unordered_map<std::string, Chart> Charts;
};

} // namespace otel

#endif /* NETDATA_OTEL_PROCESS_HPP */
