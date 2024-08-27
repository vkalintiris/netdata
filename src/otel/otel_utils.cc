#include "otel_utils.hpp"

#include <fstream>
#include <iomanip>


uint64_t pb::findOldestCollectionTime(const pb::Metric &M)
{
    uint64_t oldestTime = std::numeric_limits<uint64_t>::max();

    switch (M.data_case()) {
        case pb::Metric::kGauge:
            for (const auto &dp : M.gauge().data_points())
                oldestTime = std::min(oldestTime, dp.time_unix_nano());
            break;
        case pb::Metric::kSum:
            for (const auto &dp : M.sum().data_points())
                oldestTime = std::min(oldestTime, dp.time_unix_nano());
            break;
        case pb::Metric::kHistogram:
            for (const auto &dp : M.histogram().data_points())
                oldestTime = std::min(oldestTime, dp.time_unix_nano());
            break;
        case pb::Metric::kExponentialHistogram:
            for (const auto &dp : M.exponential_histogram().data_points())
                oldestTime = std::min(oldestTime, dp.time_unix_nano());
            break;
        case pb::Metric::kSummary:
            for (const auto &dp : M.summary().data_points())
                oldestTime = std::min(oldestTime, dp.time_unix_nano());
            break;
        default:
            std::abort();
    }

    return (oldestTime == std::numeric_limits<uint64_t>::max()) ? 0 : oldestTime;
}

std::string pb::anyValueToString(const pb::AnyValue &AV)
{
    switch (AV.value_case()) {
        case pb::AnyValue::kStringValue:
            return AV.string_value();
        case pb::AnyValue::kBoolValue:
            return AV.bool_value() ? "true" : "false";
        case pb::AnyValue::kIntValue:
            return std::to_string(AV.int_value());
        case pb::AnyValue::kDoubleValue:
            return std::to_string(AV.double_value());
        case pb::AnyValue::kArrayValue:
            // Placeholder for array values
            return "[array]";
        case pb::AnyValue::kKvlistValue:
            // Placeholder for nested key-value lists
            return "{kvlist}";
        case pb::AnyValue::kBytesValue:
            // Placeholder for byte arrays
            return "[bytes]";
        default:
            return "[unknown]";
    }
}

void pb::dumpArenaStats(const std::string &Path, const std::string &Label, const pb::Arena &A)
{
    std::ofstream OS(Path, std::ios_base::app);
    if (!OS) {
        std::cerr << "Failed to open file: " << Path << std::endl;
        return;
    }

    OS << "=== Arena Statistics " << Label << " ===" << std::endl;

    OS << "SpaceUsed: " << A.SpaceUsed() << " bytes" << std::endl;
    OS << "SpaceAllocated: " << A.SpaceAllocated() << " bytes" << std::endl;

    double UsedPct = (A.SpaceUsed() * 100.0) / A.SpaceAllocated();
    OS << std::fixed << std::setprecision(2);
    OS << "Used Percentage: " << UsedPct << "%" << std::endl;

    OS << std::endl;
    OS.close();
}

#if 0
#include "libnetdata/blake3/blake3.h"
#include "metadata.h"
#include <fstream>
#include <ostream>
#include <iomanip>


using Buckets = opentelemetry::proto::metrics::v1::ExponentialHistogramDataPoint::Buckets;

/*
 * Transform utils
*/

class OTELMetricsRestructurer {
public:
    OTELMetricsRestructurer(const otel::config::Config *Cfg) : Cfg(Cfg)
    {
    }

    std::vector<pb::Metric>
    restructureMetrics(const pb::InstrumentationScope &IS, const std::vector<pb::Metric> &InputMetrics)
    {
        std::vector<pb::Metric> RestructuredMetrics;

        auto *CfgScope = Cfg->getScope(IS.name());
        if (!CfgScope) {
            return InputMetrics;
        }

        std::vector<pb::Metric> NewMetrics;
        for (const auto &M : InputMetrics) {
            auto *CfgMetric = CfgScope->getMetric(M.name());
            if (!CfgMetric || CfgMetric->getInstanceAttributes()->empty()) {
                RestructuredMetrics.push_back(M);
                continue;
            }

            if (M.has_gauge()) {
                NewMetrics = restructureGauge(CfgMetric, M);
            } else if (M.has_sum()) {
                NewMetrics = restructureSum(CfgMetric, M);
            } else if (M.has_histogram()) {
                NewMetrics = restructureHistogram(CfgMetric, M);
            } else if (M.has_summary()) {
                NewMetrics = restructureSummary(CfgMetric, M);
            }

            RestructuredMetrics.insert(RestructuredMetrics.end(), NewMetrics.begin(), NewMetrics.end());
            NewMetrics.clear();
        }

        return RestructuredMetrics;
    }

private:
    std::vector<pb::Metric> restructureGauge(const otel::config::Metric *CfgMetric, const pb::Metric &M)
    {
        auto GDPs = groupDataPoints(CfgMetric->getInstanceAttributes(), M.gauge().data_points());
        return createNewMetrics(M, GDPs, [&](pb::Metric &NewMetric, const auto &DPs) {
            auto *G = NewMetric.mutable_gauge();
            *G->mutable_data_points() = {DPs.begin(), DPs.end()};
        });
    }

    std::vector<pb::Metric> restructureSum(const otel::config::Metric *CfgMetric, const pb::Metric &M)
    {
        auto GDPs = groupDataPoints(CfgMetric->getInstanceAttributes(), M.sum().data_points());
        return createNewMetrics(M, GDPs, [&](pb::Metric &NewMetric, const auto &DPs) {
            auto *S = NewMetric.mutable_sum();
            *S->mutable_data_points() = {DPs.begin(), DPs.end()};
            S->set_aggregation_temporality(M.sum().aggregation_temporality());
            S->set_is_monotonic(M.sum().is_monotonic());
        });
    }

    std::vector<pb::Metric> restructureHistogram(const otel::config::Metric *CfgMetric, const pb::Metric &M)
    {
        auto GDPs = groupDataPoints(CfgMetric->getInstanceAttributes(), M.histogram().data_points());
        return createNewMetrics(M, GDPs, [&](pb::Metric &NewMetric, const auto &DPs) {
            auto *H = NewMetric.mutable_histogram();
            *H->mutable_data_points() = {DPs.begin(), DPs.end()};
            H->set_aggregation_temporality(M.histogram().aggregation_temporality());
        });
    }

    std::vector<pb::Metric> restructureSummary(const otel::config::Metric *CfgMetric, const pb::Metric &M)
    {
        auto GDPs = groupDataPoints(CfgMetric->getInstanceAttributes(), M.summary().data_points());
        return createNewMetrics(M, GDPs, [&](pb::Metric &NewMetric, const auto &DPs) {
            auto *S = NewMetric.mutable_summary();
            *S->mutable_data_points() = {DPs.begin(), DPs.end()};
        });
    }

    template <typename T> std::string createGroupKey(const std::set<std::string> *InstanceAttributes, const T &DP)
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
    std::unordered_map<std::string, std::vector<T> >
    groupDataPoints(const std::set<std::string> *InstanceAttributes, const pb::RepeatedPtrField<T> &DPs)
    {
        std::unordered_map<std::string, std::vector<T> > Groups;

        for (const auto &DP : DPs) {
            std::string GroupKey = createGroupKey(InstanceAttributes, DP);
            Groups[GroupKey].push_back(DP);
        }

        return Groups;
    }

    template <typename T, typename F>
    std::vector<pb::Metric> createNewMetrics(
        const pb::Metric &OrigMetric,
        const std::unordered_map<std::string, std::vector<T> > &GDPs,
        F setDataPoints)
    {
        std::vector<pb::Metric> NewMetrics;

        for (const auto &P : GDPs) {
            pb::Metric NewMetric;
            NewMetric.set_name(OrigMetric.name() + "_" + P.first);
            NewMetric.set_description(OrigMetric.description());
            NewMetric.set_unit(OrigMetric.unit());

            setDataPoints(NewMetric, P.second);

            NewMetrics.push_back(NewMetric);
        }

        return NewMetrics;
    }

private:
    const otel::config::Config *Cfg;
};

void pb::restructureOTELMetrics(const otel::config::Config *Cfg, pb::MetricsData &MD)
{
    OTELMetricsRestructurer Restructurer(Cfg);

    for (auto &RMs : *MD.mutable_resource_metrics()) {
        for (auto &SMs : *RMs.mutable_scope_metrics()) {
            if (!SMs.has_scope()) {
                // TODO: log this somewhere
                continue;
            }

            std::vector<pb::Metric> NewMetrics =
                Restructurer.restructureMetrics(SMs.scope(), {SMs.metrics().begin(), SMs.metrics().end()});

            SMs.clear_metrics();
            *SMs.mutable_metrics() = {NewMetrics.begin(), NewMetrics.end()};
        }
    }
}

/*
 * Flatten attributes
*/

#include <algorithm>
#include <vector>
#include <unordered_map>
#include <string>
#include "opentelemetry/proto/common/v1/common.pb.h"
#include "opentelemetry/proto/resource/v1/resource.pb.h"
#include "opentelemetry/proto/metrics/v1/metrics.pb.h"

void extractFlattenedAttributes(
    const pb::RepeatedPtrField<pb::KeyValue> &Attrs,
    std::unordered_map<std::string, std::string> &Result,
    const std::string &Prefix = "")
{
    for (const auto &Attr : Attrs) {
        std::string Key = Prefix + Attr.key();
        const auto &Value = Attr.value();

        if (Value.has_array_value()) {
            const auto &Arr = Value.array_value().values();
            for (int i = 0; i < Arr.size(); ++i) {
                Result[Key + "_" + std::to_string(i)] = anyValueToString(Arr[i]);
            }
        } else if (Value.has_kvlist_value()) {
            extractFlattenedAttributes(Value.kvlist_value().values(), Result, Key + "_");
        } else {
            Result[Key] = anyValueToString(Value);
        }
    }
}

std::unordered_map<std::string, std::string> extractResourceAttributes(const pb::Resource &R)
{
    std::unordered_map<std::string, std::string> Result;
    extractFlattenedAttributes(R.attributes(), Result, "r");
    return Result;
}

std::unordered_map<std::string, std::string> extractInstrumentationScopeAttributes(const pb::InstrumentationScope &IS)
{
    std::unordered_map<std::string, std::string> Result;
    extractFlattenedAttributes(IS.attributes(), Result, "s_");
    return Result;
}

std::unordered_map<std::string, std::string> extractAllAttributes(const pb::MetricsData &MD)
{
    std::unordered_map<std::string, std::string> allAttributes;

    for (const auto &resourceMetrics : MD.resource_metrics()) {
        auto resourceAttrs = extractResourceAttributes(resourceMetrics.resource());
        allAttributes.insert(resourceAttrs.begin(), resourceAttrs.end());

        for (const auto &scopeMetrics : resourceMetrics.scope_metrics()) {
            auto scopeAttrs = extractInstrumentationScopeAttributes(scopeMetrics.scope());
            allAttributes.insert(scopeAttrs.begin(), scopeAttrs.end());
        }
    }

    return allAttributes;
}

static std::string *createPrefixKey(pb::Arena *A, const std::string &P, const std::string &K)
{
    std::string *NP = google::protobuf::Arena::Create<std::string>(A);
    if (P.empty()) {
        *NP = K;
    } else {
        NP->reserve(P.size() + 1 + K.size());
        *NP = P;
        NP->append(".");
        NP->append(K);
    }

    return NP;
}

// Forward declaration
void flattenAttributes(
    pb::Arena *A,
    const std::string &Prefix,
    const pb::KeyValue &KV,
    pb::RepeatedPtrField<pb::KeyValue> *RPF);

void flattenResourceAttributes(pb::Arena *A, pb::Resource *R)
{
    pb::RepeatedPtrField<pb::KeyValue> *RPF =
        google::protobuf::Arena::CreateMessage<pb::RepeatedPtrField<pb::KeyValue> >(A);

    for (const auto &Attr : R->attributes())
        flattenAttributes(A, "r_", Attr, RPF);

    R->clear_attributes();
    R->mutable_attributes()->Swap(RPF);
}

void flattenAttributes(
    pb::Arena *A,
    const std::string &Prefix,
    const pb::KeyValue &KV,
    pb::RepeatedPtrField<pb::KeyValue> *RPF)
{
    std::string *NewPrefix = createPrefixKey(A, Prefix, KV.key());

    switch (KV.value().value_case()) {
        case pb::AnyValue::kKvlistValue: {
            for (const auto &NestedKV : KV.value().kvlist_value().values())
                flattenAttributes(A, *NewPrefix, NestedKV, RPF);
            break;
        }
        case pb::AnyValue::kArrayValue: {
            for (int Idx = 0; Idx < KV.value().array_value().values_size(); ++Idx) {
                const std::string Position = std::to_string(Idx);

                std::string *AK = pb::Arena::Create<std::string>(A);
                AK->reserve(NewPrefix->size() + 3 + Position.size());
                *AK = *NewPrefix;
                AK->append("[");
                AK->append(Position);
                AK->append("]");

                pb::KeyValue *FlattenedKV = RPF->Add();
                FlattenedKV->set_key(*AK);
                *FlattenedKV->mutable_value() = KV.value().array_value().values(Idx);
            }
            break;
        }
        default:
            pb::KeyValue *FlattenedKV = RPF->Add();
            FlattenedKV->set_key(*NewPrefix);
            *FlattenedKV->mutable_value() = KV.value();
            break;
    }
}

#include <iostream>
#include <cassert>
#include <google/protobuf/util/message_differencer.h>
#include "opentelemetry/proto/resource/v1/resource.pb.h"
#include "opentelemetry/proto/common/v1/common.pb.h"

// Function declaration (implementation in the previous artifact)
void flattenResourceAttributes(google::protobuf::Arena *arena, pb::Resource *resource);

// Helper function to add a nested key-value pair
void addNestedKeyValue(pb::AnyValue *Parent, const std::string &Key, const pb::AnyValue &AV)
{
    auto *KV = Parent->mutable_kvlist_value()->add_values();
    KV->set_key(Key);
    *KV->mutable_value() = AV;
}

// Helper function to create a complex nested structure
void createComplexResource(pb::Resource *resource)
{
    // Add some top-level attributes
    auto attrs = resource->mutable_attributes();

    {
        auto KV = attrs->Add();
        KV->set_key("service.name");
        KV->mutable_value()->set_string_value("test_service");
    }

    {
        auto KV = attrs->Add();
        KV->set_key("container");

        auto *Container = KV->mutable_value()->mutable_kvlist_value();
        addNestedKeyValue(KV->mutable_value(), "id", pb::AnyValue());
        Container->mutable_values(0)->mutable_value()->set_string_value("abc123");
        addNestedKeyValue(KV->mutable_value(), "image", pb::AnyValue());
        Container->mutable_values(1)->mutable_value()->set_string_value("test_image:v1");

        addNestedKeyValue(KV->mutable_value(), "command", pb::AnyValue());
        auto *Command = Container->mutable_values(2)->mutable_value()->mutable_array_value();
        Command->add_values()->set_string_value("./app");
        Command->add_values()->set_string_value("--config");
        Command->add_values()->set_string_value("/etc/app/config.yaml");
    }
}

void dump(const std::string &Path, const pb::Resource *R)
{
    std::ofstream OS(Path);
    if (OS.is_open()) {
        OS << R->Utf8DebugString() << std::endl;
        OS.close();
    } else {
        std::cerr << "Unable to open /tmp/foo.txt for appending" << std::endl;
    }
}

void pb::testFlattenResourceAttributes()
{
    pb::Arena A;

    // Create a complex resource
    pb::Resource *R = pb::Arena::CreateMessage<pb::Resource>(&A);
    createComplexResource(R);

    dump("/tmp/before.txt", R);

    // Flatten the resource attributes
    flattenResourceAttributes(&A, R);

    dump("/tmp/after.txt", R);
}

#endif
