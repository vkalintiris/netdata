#include "otel_process.hpp"

static std::string origMetricName(const pb::Metric &M)
{
    for (const auto &Attr : M.metadata()) {
        if (Attr.key() == "_nd_orig_metric_name") {
            return Attr.value().string_value();
        }
    }

    return M.name();
}

void otel::MetricsDataProcessor::onResourceMetrics(const pb::ResourceMetrics &RMs)
{
    SMH = RMH.hash(RMs);

    Labels.Clear();
    if (RMs.has_resource()) {
        pb::flattenResource(Labels, RMs.resource());
    }
}

void otel::MetricsDataProcessor::onScopeMetrics(const pb::ResourceMetrics &RMs, const pb::ScopeMetrics &SMs)
{
    UNUSED(RMs);

    MH = SMH.hash(SMs);

    if (SMs.has_scope()) {
        ScopeCfg = Ctx.config()->getScope(SMs.scope().name());
    } else {
        ScopeCfg = nullptr;
    }
}

void otel::MetricsDataProcessor::onMetric(
    const pb::ResourceMetrics &RMs,
    const pb::ScopeMetrics &SMs,
    const pb::Metric &M)
{
    UNUSED(RMs);
    UNUSED(SMs);
    auto &Charts = Ctx.charts();

    const std::string BlakeId = MH.hash(M);
    std::string ChartId = M.name() + "_" + BlakeId;

    auto It = Charts.find(ChartId);
    if (It == Charts.end()) {
        It = Charts.emplace(ChartId, Chart()).first;
    }

    std::string OrigMetricName = origMetricName(M);

    const auto *Resource = RMs.has_resource() ? &RMs.resource() : nullptr;
    It->second.update(ScopeCfg, M, BlakeId, Labels, Resource, &Charts);
}
