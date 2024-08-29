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
        const pb::RepeatedPtrField<pb::KeyValue> &Labels,
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
            setLabels(Labels);
        }

        updateRDs(M);

        ActiveResource = nullptr;
        ActiveMetric = nullptr;
        ActiveCharts = nullptr;
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

    const pb::Resource *ActiveResource = nullptr;
    const pb::Metric *ActiveMetric = nullptr;
    const std::unordered_map<std::string, Chart> *ActiveCharts = nullptr;
};

} // namespace otel

#endif /* NETDATA_OTEL_CHART_HPP */
