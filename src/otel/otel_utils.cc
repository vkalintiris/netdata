#include "otel_utils.h"

#include "metadata.h"
#include <fstream>
#include <ostream>

using Buckets = opentelemetry::proto::metrics::v1::ExponentialHistogramDataPoint::Buckets;

static void printBuckets(std::ostream &OS, const Buckets &B)
{
    OS << "Offset: " << B.offset() << "\n";

    OS << "Bucket Counts: ";
    for (const auto &Count : B.bucket_counts()) {
        OS << Count << " ";
    }

    OS << "\n";
}

void pb::printAnyValue(std::ostream &OS, const pb::AnyValue &Value)
{
    switch (Value.value_case()) {
        case pb::AnyValue::kStringValue:
            OS << Value.string_value();
            break;
        case pb::AnyValue::kBoolValue:
            OS << std::boolalpha << Value.bool_value();
            break;
        case pb::AnyValue::kIntValue:
            OS << Value.int_value();
            break;
        case pb::AnyValue::kDoubleValue:
            OS << Value.double_value();
            break;
        case pb::AnyValue::kArrayValue:
            pb::printArrayValue(OS, Value.array_value());
            break;
        case pb::AnyValue::kKvlistValue:
            pb::printKeyValueList(OS, Value.kvlist_value());
            break;
        case pb::AnyValue::kBytesValue:
            OS << "[bytes]"; // Placeholder, handling bytes can be more complex
            break;
        default:
            OS << "Empty or unknown type";
    }
}

void pb::printArrayValue(std::ostream &OS, const pb::ArrayValue &AV)
{
    OS << "[";
    for (int i = 0; i < AV.values_size(); ++i) {
        printAnyValue(OS, AV.values(i));
        if (i < AV.values_size() - 1) {
            OS << ", ";
        }
    }
    OS << "]";
}

void pb::printKeyValueList(std::ostream &OS, const pb::KeyValueList &KVL)
{
    OS << "{";

    for (int i = 0; i < KVL.values_size(); ++i) {
        const pb::KeyValue &KV = KVL.values(i);

        OS << KV.key() << ": ";
        printAnyValue(OS, KV.value());

        if (i < KVL.values_size() - 1)
            OS << ", ";
    }

    OS << "}";
}

void pb::printInstrumentationScope(std::ostream &OS, const pb::InstrumentationScope &IS)
{
    OS << "Instrumentation Scope:\n";
    OS << "Name: " << IS.name() << "\n";
    OS << "Version: " << IS.version() << "\n";
    OS << "Attributes:\n";

    for (const auto &KV : IS.attributes()) {
        OS << KV.key() << ": ";
        printAnyValue(OS, KV.value());
        OS << "\n";
    }
    OS << "Dropped Attributes Count: " << IS.dropped_attributes_count() << "\n";
}

void pb::printResource(std::ostream &OS, const pb::Resource &Res)
{
    OS << "Resource Attributes:\n";

    for (const auto &KV : Res.attributes()) {
        OS << KV.key() << ": ";
        printAnyValue(OS, KV.value());
        OS << "\n";
    }

    OS << "Dropped Attributes Count: " << Res.dropped_attributes_count() << "\n";
}

void pb::printExemplar(std::ostream &OS, const pb::Exemplar &Ex)
{
    OS << "Exemplar:\n";
    OS << "Filtered Attributes:\n";

    for (const auto &KV : Ex.filtered_attributes()) {
        OS << KV.key() << ": ";
        printAnyValue(OS, KV.value());
        OS << "\n";
    }

    OS << "Time Unix Nano: " << Ex.time_unix_nano() << "\n";
    OS << "Value: ";
    switch (Ex.value_case()) {
        case Exemplar::kAsDouble:
            OS << Ex.as_double();
            break;
        case Exemplar::kAsInt:
            OS << Ex.as_int();
            break;
        default:
            OS << "Invalid or unknown value type";
    }
    OS << "\n";

    if (!Ex.span_id().empty()) {
        OS << "Span ID: " << std::hex;
        for (unsigned char c : Ex.span_id())
            OS << static_cast<int>(c);
        OS << std::dec << "\n";
    }

    if (!Ex.trace_id().empty()) {
        OS << "Trace ID: " << std::hex;
        for (unsigned char c : Ex.trace_id())
            OS << static_cast<int>(c);
        OS << std::dec << "\n";
    }
}

void pb::printNumberDataPoint(std::ostream &OS, const pb::NumberDataPoint &DP)
{
    OS << "NumberDataPoint:\n";
    OS << "Attributes:\n";
    for (const auto &KV : DP.attributes()) {
        OS << KV.key() << ": ";
        printAnyValue(OS, KV.value());
        OS << "\n";
    }

    OS << "Start Time Unix Nano: " << DP.start_time_unix_nano() << "\n";
    OS << "Time Unix Nano: " << DP.time_unix_nano() << "\n";

    OS << "Value: ";
    switch (DP.value_case()) {
        case NumberDataPoint::kAsDouble:
            OS << DP.as_double();
            break;
        case NumberDataPoint::kAsInt:
            OS << DP.as_int();
            break;
        default:
            OS << "Invalid or unknown value type";
    }
    OS << "\n";

    OS << "Exemplars:\n";
    for (const auto &Ex : DP.exemplars()) {
        printExemplar(OS, Ex);
        OS << "\n";
    }

    OS << "Flags: " << DP.flags();
    if (DP.flags() & DataPointFlags::DATA_POINT_FLAGS_NO_RECORDED_VALUE_MASK) {
        OS << " (No Recorded Value)";
    }

    OS << "\n";
}

void pb::printSummaryDataPoint(std::ostream &OS, const pb::SummaryDataPoint &DP)
{
    OS << "SummaryDataPoint:\n";

    OS << "Attributes:\n";
    for (const auto &KV : DP.attributes()) {
        OS << KV.key() << ": ";
        printAnyValue(OS, KV.value());
        OS << "\n";
    }

    OS << "Start Time Unix Nano: " << DP.start_time_unix_nano() << "\n";
    OS << "Time Unix Nano: " << DP.time_unix_nano() << "\n";
    OS << "Count: " << DP.count() << "\n";
    OS << "Sum: " << DP.sum() << "\n";

    OS << "Quantile Values:\n";
    for (const auto &quantileValue : DP.quantile_values()) {
        OS << "Quantile: " << quantileValue.quantile() << ", Value: " << quantileValue.value() << "\n";
    }

    OS << "Flags: " << DP.flags();
    if (DP.flags() & DataPointFlags::DATA_POINT_FLAGS_NO_RECORDED_VALUE_MASK) {
        OS << " (No Recorded Value)";
    }
    OS << "\n";
}

void pb::printHistogramDataPoint(std::ostream &OS, const pb::HistogramDataPoint &DP)
{
    OS << "HistogramDataPoint:\n";

    OS << "Attributes:\n";
    for (const auto &attribute : DP.attributes()) {
        OS << attribute.key() << ": ";
        printAnyValue(OS, attribute.value());
        OS << "\n";
    }

    OS << "Start Time Unix Nano: " << DP.start_time_unix_nano() << "\n";
    OS << "Time Unix Nano: " << DP.time_unix_nano() << "\n";
    OS << "Count: " << DP.count() << "\n";
    if (DP.has_sum()) {
        OS << "Sum: " << DP.sum() << "\n";
    }

    OS << "Bucket Counts:\n";
    for (const auto &count : DP.bucket_counts()) {
        OS << count << "\n";
    }

    OS << "Explicit Bounds:\n";
    for (const auto &bound : DP.explicit_bounds()) {
        OS << bound << "\n";
    }

    OS << "Exemplars:\n";
    for (const auto &exemplar : DP.exemplars()) {
        printExemplar(OS, exemplar);
        OS << "\n";
    }

    OS << "Flags: " << DP.flags();
    if (DP.flags() & DataPointFlags::DATA_POINT_FLAGS_NO_RECORDED_VALUE_MASK) {
        OS << " (No Recorded Value)";
    }
    OS << "\n";

    if (DP.has_min()) {
        OS << "Min: " << DP.min() << "\n";
    }

    if (DP.has_max()) {
        OS << "Max: " << DP.max() << "\n";
    }
}

void pb::printExponentialHistogramDataPoint(std::ostream &OS, const pb::ExponentialHistogramDataPoint &DP)
{
    OS << "ExponentialHistogramDataPoint:\n";

    OS << "Attributes:\n";
    for (const auto &attribute : DP.attributes()) {
        OS << attribute.key() << ": ";
        printAnyValue(OS, attribute.value());
        OS << "\n";
    }

    OS << "Start Time Unix Nano: " << DP.start_time_unix_nano() << "\n";
    OS << "Time Unix Nano: " << DP.time_unix_nano() << "\n";
    OS << "Count: " << DP.count() << "\n";

    if (DP.has_sum()) {
        OS << "Sum: " << DP.sum() << "\n";
    }

    OS << "Scale: " << DP.scale() << "\n";
    OS << "Zero Count: " << DP.zero_count() << "\n";

    OS << "Positive Buckets:\n";
    printBuckets(OS, DP.positive());

    OS << "Negative Buckets:\n";
    printBuckets(OS, DP.negative());

    OS << "Flags: " << DP.flags();
    if (DP.flags() & DataPointFlags::DATA_POINT_FLAGS_NO_RECORDED_VALUE_MASK) {
        OS << " (No Recorded Value)";
    }
    OS << "\n";

    OS << "Exemplars:\n";
    for (const auto &exemplar : DP.exemplars()) {
        printExemplar(OS, exemplar);
        OS << "\n";
    }

    if (DP.has_min()) {
        OS << "Min: " << DP.min() << "\n";
    }

    if (DP.has_max()) {
        OS << "Max: " << DP.max() << "\n";
    }

    if (DP.zero_threshold() != 0) {
        OS << "Zero Threshold: " << DP.zero_threshold() << "\n";
    }
}

void pb::printGauge(std::ostream &OS, const pb::Gauge &G)
{
    OS << "Gauge:\n";
    for (const auto &DP : G.data_points()) {
        printNumberDataPoint(OS, DP);
        OS << "\n";
    }
}

void pb::printSum(std::ostream &OS, const pb::Sum &S)
{
    OS << "Sum:\n";
    OS << "Aggregation Temporality: " << S.aggregation_temporality() << "\n";
    OS << "Is Monotonic: " << std::boolalpha << S.is_monotonic() << "\n";

    for (const auto &DP : S.data_points()) {
        printNumberDataPoint(OS, DP);
        OS << "\n";
    }
}

void pb::printHistogram(std::ostream &OS, const pb::Histogram &H)
{
    OS << "Histogram:\n";
    OS << "Aggregation Temporality: " << H.aggregation_temporality() << "\n";

    for (const auto &DP : H.data_points()) {
        printHistogramDataPoint(OS, DP);
        OS << "\n";
    }
}

void pb::printExponentialHistogram(std::ostream &OS, const pb::ExponentialHistogram &H)
{
    OS << "ExponentialHistogram:\n";
    OS << "Aggregation Temporality: " << H.aggregation_temporality() << "\n";

    for (const auto &DP : H.data_points()) {
        printExponentialHistogramDataPoint(OS, DP);
        OS << "\n";
    }
}

void pb::printSummary(std::ostream &OS, const pb::Summary &S)
{
    OS << "Summary:\n";
    for (const auto &DP : S.data_points()) {
        printSummaryDataPoint(OS, DP);
        OS << "\n";
    }
}

void pb::printMetric(std::ostream &OS, const pb::Metric &M)
{
    OS << "Metric Name: " << M.name() << "\n";
    OS << "Description: " << M.description() << "\n";
    OS << "Unit: " << M.unit() << "\n";

    if (M.has_gauge()) {
        printGauge(OS, M.gauge());
    } else if (M.has_sum()) {
        printSum(OS, M.sum());
    } else if (M.has_histogram()) {
        printHistogram(OS, M.histogram());
    } else if (M.has_exponential_histogram()) {
        printExponentialHistogram(OS, M.exponential_histogram());
    } else if (M.has_summary()) {
        printSummary(OS, M.summary());
    }

    OS << "Metadata:\n";
    for (const auto &attribute : M.metadata()) {
        OS << attribute.key() << ": ";
        printAnyValue(OS, attribute.value());
        OS << "\n";
    }
}

void pb::printScopeMetrics(std::ostream &OS, const ScopeMetrics &SM)
{
    OS << "ScopeMetrics:\n";

    OS << "Scope:\n";
    printInstrumentationScope(OS, SM.scope());

    OS << "Metrics:\n";
    for (const auto &M : SM.metrics()) {
        printMetric(OS, M);
        OS << "\n";
    }

    OS << "Schema URL: " << SM.schema_url() << "\n";
}

void pb::printResourceMetrics(std::ostream &OS, const pb::ResourceMetrics &RM)
{
    OS << "ResourceMetrics:\n";

    OS << "Resource:\n";
    printResource(OS, RM.resource());

    OS << "Schema URL: " << RM.schema_url() << "\n";

    for (const auto &SM : RM.scope_metrics()) {
        printScopeMetrics(OS, SM);
        OS << "\n";
    }
}

void pb::printMetricsData(std::ostream &OS, const pb::MetricsData &MD)
{
    OS << "MetricsData:\n";
    for (const auto &resourceMetrics : MD.resource_metrics()) {
        printResourceMetrics(OS, resourceMetrics);
        OS << "\n";
    }
}

/*
 * Transform utils
*/

class OTELMetricsRestructurer {
public:
    OTELMetricsRestructurer(const otel::config::Config *Cfg) : Cfg(Cfg)
    {
    }

    std::vector<pb::Metric> restructureMetrics(const pb::InstrumentationScope &IS, const std::vector<pb::Metric> &InputMetrics)
    {
        auto *CfgScope = Cfg->getScope(IS.name());

        std::vector<pb::Metric> RestructuredMetrics;

        std::vector<pb::Metric> NewMetrics;
        for (const auto &M : InputMetrics) {
            auto *CfgMetric = CfgScope->getMetric(M.name());

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

        for (const auto &IA: *InstanceAttributes) {
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
    std::unordered_map<std::string, std::vector<T> > groupDataPoints(const std::set<std::string> *InstanceAttributes, const pb::RepeatedPtrField<T> &DPs)
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
            }

            std::vector<pb::Metric> NewMetrics =
                Restructurer.restructureMetrics(SMs.scope(), {SMs.metrics().begin(), SMs.metrics().end()});

            SMs.clear_metrics();
            *SMs.mutable_metrics() = {NewMetrics.begin(), NewMetrics.end()};
        }
    }
}

/*
 * Attributes sorting utils
*/

static void sortKeyValueList(pb::KeyValueList *KVL);

struct KeyValueComparator {
    bool operator()(const pb::KeyValue &LHS, const pb::KeyValue &RHS) const
    {
        return LHS.key() < RHS.key();
    }
};

static void sortArrayValue(pb::ArrayValue *AV)
{
    auto *Values = AV->mutable_values();

    for (auto &Value : *Values) {
        if (Value.has_array_value()) {
            sortArrayValue(Value.mutable_array_value());
        } else if (Value.has_kvlist_value()) {
            sortKeyValueList(Value.mutable_kvlist_value());
        }
    }
}

static void sortKeyValueList(pb::KeyValueList *KVL)
{
    auto *Values = KVL->mutable_values();

    std::sort(Values->begin(), Values->end(), KeyValueComparator());

    for (auto &KV : *Values) {
        if (KV.value().has_array_value()) {
            sortArrayValue(KV.mutable_value()->mutable_array_value());
        } else if (KV.value().has_kvlist_value()) {
            sortKeyValueList(KV.mutable_value()->mutable_kvlist_value());
        }
    }
}

static void sortRepeatedAttributes(pb::RepeatedPtrField<pb::KeyValue> *Attrs)
{
    std::sort(Attrs->begin(), Attrs->end(), KeyValueComparator());

    for (auto &Attr : *Attrs) {
        if (Attr.value().has_array_value()) {
            sortArrayValue(Attr.mutable_value()->mutable_array_value());
        } else if (Attr.value().has_kvlist_value()) {
            sortKeyValueList(Attr.mutable_value()->mutable_kvlist_value());
        }
    }
}

static void sortResourceAttributes(pb::Resource *R)
{
    sortRepeatedAttributes(R->mutable_attributes());
}

static void sortInstrumentationScopeAttributes(pb::InstrumentationScope *S)
{
    sortRepeatedAttributes(S->mutable_attributes());
}

void pb::sortMetricsDataAttributes(pb::MetricsData &MD)
{
    for (auto &RMs : *MD.mutable_resource_metrics()) {
        sortResourceAttributes(RMs.mutable_resource());

        for (auto &scopeMetrics : *RMs.mutable_scope_metrics()) {
            sortInstrumentationScopeAttributes(scopeMetrics.mutable_scope());
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

std::string anyValueToString(const pb::AnyValue &AV) {
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

void extractFlattenedAttributes(const pb::RepeatedPtrField<pb::KeyValue>& Attrs,
                                std::unordered_map<std::string, std::string>& Result,
                                const std::string& Prefix = "") {
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

std::unordered_map<std::string, std::string> extractResourceAttributes(const pb::Resource &R) {
    std::unordered_map<std::string, std::string> Result;
    extractFlattenedAttributes(R.attributes(), Result, "r_");
    return Result;
}

std::unordered_map<std::string, std::string> extractInstrumentationScopeAttributes(const pb::InstrumentationScope& IS) {
    std::unordered_map<std::string, std::string> Result;
    extractFlattenedAttributes(IS.attributes(), Result, "s_");
    return Result;
}

std::unordered_map<std::string, std::string> extractAllAttributes(const pb::MetricsData &MD) {
    std::unordered_map<std::string, std::string> allAttributes;

    for (const auto& resourceMetrics : MD.resource_metrics()) {
        auto resourceAttrs = extractResourceAttributes(resourceMetrics.resource());
        allAttributes.insert(resourceAttrs.begin(), resourceAttrs.end());

        for (const auto& scopeMetrics : resourceMetrics.scope_metrics()) {
            auto scopeAttrs = extractInstrumentationScopeAttributes(scopeMetrics.scope());
            allAttributes.insert(scopeAttrs.begin(), scopeAttrs.end());
        }
    }

    return allAttributes;
}

// Example usage
void processMetricsDataAttributes(const pb::MetricsData &MD) {
    auto flattenedAttributes = extractAllAttributes(MD);

    // Use the flattened attributes
    for (const auto& pair : flattenedAttributes) {
        std::cout << pair.first << ": " << pair.second << std::endl;
    }
}
