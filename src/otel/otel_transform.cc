#include <otel_restructure.hpp>

#include <fstream>
#include <iomanip>

template <typename T> static std::string createGroupKey(const std::vector<std::string> *InstanceAttributes, const T &DP)
{
    std::string Key;

    for (const auto &IA : *InstanceAttributes) {
        for (const auto &Attr : DP.attributes()) {
            if (Attr.key() == IA) {
                Key += Attr.value().string_value() + "_";
                break;
            }
        }
    }

    // Remove trailing underscore
    if (!Key.empty()) {
        Key.pop_back();
    }

    return Key;
}

template <typename T>
static std::unordered_map<std::string, std::vector<T> >
groupDataPoints(const std::vector<std::string> *InstanceAttributes, const pb::RepeatedPtrField<T> &DPs)
{
    std::unordered_map<std::string, std::vector<T> > Groups;

    for (const auto &DP : DPs) {
        std::string GroupKey = createGroupKey(InstanceAttributes, DP);
        Groups[GroupKey].push_back(DP);
    }

    return Groups;
}

template <typename T, typename F>
static pb::RepeatedPtrField<pb::Metric> createNewMetrics(
    const pb::Metric &OrigMetric,
    const std::unordered_map<std::string, std::vector<T> > &GDPs,
    F setDataPoints)
{
    pb::RepeatedPtrField<pb::Metric> NewMetrics;

    for (const auto &P : GDPs) {
        pb::Metric *NewMetric = NewMetrics.Add();
        NewMetric->set_name(OrigMetric.name() + "_" + P.first);
        NewMetric->set_description(OrigMetric.description());
        NewMetric->set_unit(OrigMetric.unit());

        setDataPoints(*NewMetric, P.second);
    }

    return NewMetrics;
}

static void dumpArenaStats(const google::protobuf::Arena &arena, const std::string &filename, const std::string &label)
{
    std::ofstream OS(filename, std::ios_base::app);
    if (!OS) {
        std::cerr << "Failed to open file: " << filename << std::endl;
        return;
    }

    OS << "=== Arena Statistics " << label << " ===" << std::endl;
    OS << "SpaceUsed: " << arena.SpaceUsed() << " bytes" << std::endl;
    OS << "SpaceAllocated: " << arena.SpaceAllocated() << " bytes" << std::endl;

    // Calculate and output percentages
    double usedPercentage = (arena.SpaceUsed() * 100.0) / arena.SpaceAllocated();

    OS << std::fixed << std::setprecision(2);
    OS << "Used Percentage: " << usedPercentage << "%" << std::endl;

    OS << std::endl;
    OS.close();
}

static pb::RepeatedPtrField<pb::Metric> restructureGauge(const otel::MetricConfig *CfgMetric, const pb::Metric &M)
{
    auto GDPs = groupDataPoints(CfgMetric->getInstanceAttributes(), M.gauge().data_points());
    return createNewMetrics(M, GDPs, [&](pb::Metric &NewMetric, const auto &DPs) {
        auto *G = NewMetric.mutable_gauge();
        *G->mutable_data_points() = {DPs.begin(), DPs.end()};
    });
}

static pb::RepeatedPtrField<pb::Metric> restructureSum(const otel::MetricConfig *CfgMetric, const pb::Metric &M)
{
    auto GDPs = groupDataPoints(CfgMetric->getInstanceAttributes(), M.sum().data_points());
    return createNewMetrics(M, GDPs, [&](pb::Metric &NewMetric, const auto &DPs) {
        auto *S = NewMetric.mutable_sum();
        *S->mutable_data_points() = {DPs.begin(), DPs.end()};
        S->set_aggregation_temporality(M.sum().aggregation_temporality());
        S->set_is_monotonic(M.sum().is_monotonic());
    });
}

void otel::transformMetrics(const ScopeConfig *ScopeCfg, pb::RepeatedPtrField<pb::Metric> *RPF)
{
    if (!ScopeCfg)
        return;

    pb::Arena *A = RPF->GetArena();
    if (A) {
        dumpArenaStats(*A, "arena_stats.txt", "After Restructuring");
    }

    pb::RepeatedPtrField<pb::Metric> *RestructuredMetrics =
        pb::Arena::CreateMessage<pb::RepeatedPtrField<pb::Metric> >(RPF->GetArena());

    for (const auto &M : *RPF) {
        auto *MetricCfg = ScopeCfg->getMetric(M.name());
        if (!MetricCfg || MetricCfg->getInstanceAttributes()->empty()) {
            *RestructuredMetrics->Add() = M;
            continue;
        }

        if (M.has_gauge()) {
            auto NewMetrics = restructureGauge(MetricCfg, M);

            for (const auto &NewM : NewMetrics)
                *RestructuredMetrics->Add() = NewM;
        } else if (M.has_sum()) {
            auto NewMetrics = restructureSum(MetricCfg, M);

            for (const auto &NewM : NewMetrics)
                *RestructuredMetrics->Add() = NewM;
        } else {
            std::abort();
        }
    }

    RPF->Clear();
    RPF->Swap(RestructuredMetrics);

    if (A) {
        dumpArenaStats(*A, "arena_stats.txt", "Swapping");
    }
}
