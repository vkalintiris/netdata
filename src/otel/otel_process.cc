#include "otel_process.hpp"

static std::string origMetricName(const pb::Metric &M) {
    for (const auto &Attr: M.metadata()) {
        if (Attr.key() == "_nd_orig_metric_name") {
            return Attr.value().string_value();
        }
    }

    return M.name();
}

void otel::MetricProcessor::processMetricsData(const Config *Cfg, const pb::MetricsData *MD)
{
    ResourceMetricsHasher RMH;

    for (const auto &RMs : MD->resource_metrics()) {
        ScopeMetricsHasher SMH = RMH.hash(RMs);

        const auto *Resource = RMs.has_resource() ? &RMs.resource() : nullptr;
        for (const auto &SMs : RMs.scope_metrics()) {
            if (!SMs.has_scope()) {
                fatal("No scope in scope metrics");
            }

            const ScopeConfig *ScopeCfg = Cfg->getScope(SMs.scope().name());

            MetricHasher MH = SMH.hash(SMs);
            for (const auto &M : SMs.metrics()) {
                std::string BlakeId = MH.hash(M);
                std::string ChartId = M.name() + "_" + BlakeId;

                auto It = Charts.find(ChartId);
                if (It == Charts.end())
                    It = Charts.emplace(ChartId, Chart()).first;

                std::string OrigMetricName = origMetricName(M);
                It->second.update(ScopeCfg, M, BlakeId, Resource, &Charts);
            }
        }
    }
}

std::string otel::Chart::findDimensionName(const MetricConfig *MetricCfg, const pb::NumberDataPoint &DP)
{
    if (MetricCfg) {
        const std::string *DimensionsAttribute = MetricCfg->getDimensionsAttribute();
        if (DimensionsAttribute) {
            for (const auto &Attr : DP.attributes()) {
                if (Attr.key() == *DimensionsAttribute) {
                    return Attr.value().string_value();
                }
            }
        }
    }

    return "value";
}

template <typename T> void otel::Chart::createRDs(const MetricConfig *MetricCfg, bool Monotonic, const T &DPs)
{
    for (const auto &DP : DPs) {
        std::string Name = findDimensionName(MetricCfg, DP);

        auto Algorithm = Monotonic ? RRD_ALGORITHM_INCREMENTAL : RRD_ALGORITHM_ABSOLUTE;
        RRDDIM *RD = rrddim_add(RS, Name.c_str(), nullptr, 1, 1000, Algorithm);

        RDs.push_back(RD);
    }
}

void otel::Chart::createRDs(const MetricConfig *MetricCfg, const pb::Metric &M)
{
    if (M.has_gauge()) {
        createRDs(MetricCfg, false, M.gauge().data_points());
    } else if (M.has_sum()) {
        createRDs(MetricCfg, M.sum().is_monotonic(), M.sum().data_points());
    } else {
        std::abort();
    }
}

void otel::Chart::createRS(const ScopeConfig *ScopeCfg, const pb::Metric &M, const std::string &BlakeId)
{
    uint64_t UpdateEvery = (pb::findOldestCollectionTime(M) / NSEC_PER_SEC) - LastCollectionTime;
    if (UpdateEvery == 0) {
        fatal("[GVD] WTF!? alfkjalkrjwoi");
    }

    const std::string ChartId = M.name() + "_" + BlakeId;
    const std::string OrigMetricName = origMetricName(M);
    const std::string ContextName = "otel." + OrigMetricName;

    RS = rrdset_create_localhost(
        "otel", // type
        ChartId.c_str(), // id
        M.name().c_str(), // name
        ContextName.c_str(), // family
        ContextName.c_str(), // context
        M.description().c_str(), // title
        M.unit().c_str(), // units
        "otel.plugin", // plugin
        "otel.module", // module
        666666, // priority
        UpdateEvery, // update_every
        RRDSET_TYPE_LINE // chart_type
    );

    const auto *MetricCfg = ScopeCfg ? ScopeCfg->getMetric(OrigMetricName) : nullptr;
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
            Value = DP.as_int() * 1000;
        } else {
            std::abort();
        }

        struct timeval PIT;
        PIT.tv_sec = pb::collectionTime(DP) / NSEC_PER_SEC;
        PIT.tv_usec = 0;

        rrddim_timed_set_by_pointer(RS, RD, PIT, Value);
    }

    rrdset_done(RS);
}
