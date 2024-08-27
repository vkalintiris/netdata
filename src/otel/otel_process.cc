#include "otel_process.hpp"

void otel::MetricProcessor::processMetrics(const Config *Cfg, const pb::MetricsData *MD)
{
    ResourceMetricsHasher RMH;

    for (const auto &RMs : MD->resource_metrics()) {
        ScopeMetricsHasher SMH = RMH.hash(RMs);

        for (const auto &SMs : RMs.scope_metrics()) {
            if (!SMs.has_scope())
                continue;

            MetricHasher MH = SMH.hash(SMs);
            const ScopeConfig *ScopeCfg = Cfg->getScope(SMs.scope().name());

            for (const auto &M : SMs.metrics()) {
                std::string BlakeId = MH.hash(M);
                std::string ChartId = M.name() + "_" + BlakeId;

                auto It = Charts.find(ChartId);
                if (It == Charts.end())
                    It = Charts.emplace(ChartId, Chart()).first;

                It->second.update(ScopeCfg->getMetric(M.name()), M, BlakeId);
            }
        }
    }
}

std::string otel::Chart::findDimensionName(const MetricConfig *MetricCfg, const pb::NumberDataPoint &DP)
{
    const std::string *DimensionName = MetricCfg->getDimensionsAttribute();
    if (!DimensionName)
        return "value";

    for (const auto &Attr : DP.attributes()) {
        if (Attr.key() == *DimensionName) {
            return Attr.value().string_value();
        }
    }
    return "unknown"; // Default dimension name if not found
}

template <typename T> void otel::Chart::createRDs(const MetricConfig *MetricCfg, const T &DPs)
{
    for (const auto &DP : DPs) {
        std::string Name = findDimensionName(MetricCfg, DP);

        // TODO: real implementation here
        RRDDIM *RD = rrddim_add(RS, Name.c_str(), nullptr, 1, 1, RRD_ALGORITHM_ABSOLUTE);

        RDs.push_back(RD);
    }
}

void otel::Chart::createRDs(const MetricConfig *MetricCfg, const pb::Metric &M)
{
    if (M.has_gauge()) {
        createRDs(MetricCfg, M.gauge().data_points());
    } else if (M.has_sum()) {
        createRDs(MetricCfg, M.sum().data_points());
    } else {
        std::abort();
    }
}

void otel::Chart::createRS(const MetricConfig *MetricCfg, const pb::Metric &M, const std::string &BlakeId)
{
    // TODO: sec/msec/usec?
    uint64_t UpdateEvery = pb::findOldestCollectionTime(M) - LastCollectionTime;

    const std::string ChartID = M.name() + "_" + BlakeId;

    // TODO: real implementation here
    RS = rrdset_create_localhost(
        "otel",
        ChartID.c_str(),
        ChartID.c_str(),
        "otel metrics",
        "context",
        "title",
        M.unit().c_str(),
        "OpenTelemetry",
        "otel",
        1000,
        UpdateEvery,
        RRDSET_TYPE_LINE);

    createRDs(MetricCfg, M);
}

void otel::Chart::updateRDs(const pb::Metric &M)
{
    if (M.has_gauge()) {
        updateRDs(M.gauge().data_points());
    } else if (M.has_sum()) {
        updateRDs(M.sum().data_points());
    } else {
        std::abort();
    }
}

template <typename T> void otel::Chart::updateRDs(const pb::RepeatedPtrField<T> &DPs)
{
    if (DPs.size() != static_cast<int>(RDs.size())) {
        std::abort();
    }

    for (int Idx = 0; Idx != DPs.size(); Idx++) {
        const auto &DP = DPs[Idx];
        RRDDIM *RD = RDs[Idx];

        collected_number Value = 0;
        if (DP.value_case() == pb::NumberDataPoint::kAsDouble) {
            Value = DP.as_double() * 1000;
        } else if (DP.value_case() == pb::NumberDataPoint::kAsInt) {
            Value = DP.as_int();
        }

        // TODO: used timed set
        rrddim_set_by_pointer(RS, RD, Value);
    }

    rrdset_done(RS);
}
