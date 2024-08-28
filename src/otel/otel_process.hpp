#ifndef NETDATA_OTEL_PROCESS_HPP
#define NETDATA_OTEL_PROCESS_HPP

#include "otel_utils.hpp"
#include "otel_config.hpp"
#include "otel_hash.hpp"

#include "database/rrd.h"

#include <fstream>

namespace otel
{
class Chart {
public:
    Chart() : RS(nullptr), RDs(), LastCollectionTime(0)
    {
    }

    void debug() const
    {
        std::ofstream OS("/tmp/debug.txt", std::ios_base::app);
        if (!OS) {
            fatal("Failed to open debug file");
            return;
        }

        if (ActiveResource)
            OS << "R: " << ActiveResource->Utf8DebugString() << "\n";

        if (ActiveMetric)
            OS << "M: " << ActiveMetric->Utf8DebugString() << "\n";

        if (RS) {
            OS << "LastCollectionTime: " << LastCollectionTime << "\n";

            OS << "RS: " << rrdset_id(RS) << "\n";

            for (size_t Idx = 0; Idx != RDs.size(); Idx++)
                OS << "\tRD[" << Idx << "]: " << rrddim_id(RDs[Idx]) << "\n";

            if (ActiveCharts) {
                OS << "Existing charts:"
                   << "\n";
                for (const auto &P : *ActiveCharts) {
                    const Chart &C = P.second;

                    if (C.RS) {
                        OS << "\tChart ID: " << P.first << "\n";
                        OS << "\tRS: " << rrdset_id(C.RS) << "\n";

                        for (size_t Idx = 0; Idx != C.RDs.size(); Idx++) {
                            OS << "\t\tRD[" << Idx << "]: " << rrddim_id(C.RDs[Idx]) << "\n";
                        }
                    }
                }
            }
        }

        OS.close();
    }

    void update(
        const ScopeConfig *ScopeCfg,
        const pb::Metric &M,
        const std::string &BlakeId,
        const pb::Resource *R,
        const std::unordered_map<std::string, Chart> *Charts)
    {
        ActiveResource = R;
        ActiveMetric = &M;
        ActiveCharts = Charts;

        if (!LastCollectionTime) {
            LastCollectionTime = pb::findOldestCollectionTime(M) / NSEC_PER_SEC;
            return;
        }

        if (!RS) {
            createRS(ScopeCfg, M, BlakeId);
        }

        updateRDs(M);

        ActiveResource = nullptr;
        ActiveMetric = nullptr;
        ActiveCharts = nullptr;
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

    const pb::Resource *ActiveResource = nullptr;
    const pb::Metric *ActiveMetric = nullptr;
    const std::unordered_map<std::string, Chart> *ActiveCharts = nullptr;
};

class MetricProcessor {
public:
    void processMetricsData(const Config *Cfg, const pb::MetricsData *MD);

private:
    std::unordered_map<std::string, Chart> Charts;
};

} // namespace otel

#endif /* NETDATA_OTEL_PROCESS_HPP */
