#include "otel_process.hpp"

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

    It->second.update(ScopeCfg, M, BlakeId, Labels);
}
