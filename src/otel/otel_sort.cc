#include "otel_sort.hpp"

namespace otel {

int compareArrayValue(const pb::ArrayValue &LHS, const pb::ArrayValue &RHS)
{
    if (LHS.values_size() != RHS.values_size())
        return LHS.values_size() - RHS.values_size();

    for (int Idx = 0; Idx < LHS.values_size(); ++Idx) {
        int Result = compareAnyValue(LHS.values(Idx), RHS.values(Idx));
        if (Result != 0)
            return Result;
    }

    return 0;
}

int compareKeyValueList(const pb::KeyValueList &LHS, const pb::KeyValueList &RHS)
{
    if (LHS.values_size() != RHS.values_size())
        return LHS.values_size() - RHS.values_size();

    for (int Idx = 0; Idx < LHS.values_size(); ++Idx) {
        int Result = compareKeyValue(LHS.values(Idx), RHS.values(Idx));
        if (Result != 0)
            return Result;
    }

    return 0;
}

int compareAnyValue(const pb::AnyValue &LHS, const pb::AnyValue &RHS)
{
    if (LHS.value_case() != RHS.value_case())
        return LHS.value_case() - RHS.value_case();

    switch (LHS.value_case()) {
        case pb::AnyValue::kStringValue:
            return LHS.string_value().compare(RHS.string_value());
        case pb::AnyValue::kBoolValue:
            return LHS.bool_value() - RHS.bool_value();
        case pb::AnyValue::kIntValue:
            return (LHS.int_value() < RHS.int_value()) ? -1 : (LHS.int_value() > RHS.int_value()) ? 1 : 0;
        case pb::AnyValue::kDoubleValue:
            return (LHS.double_value() < RHS.double_value()) ? -1 : (LHS.double_value() > RHS.double_value()) ? 1 : 0;
        case pb::AnyValue::kArrayValue:
            return compareArrayValue(LHS.array_value(), RHS.array_value());
        case pb::AnyValue::kKvlistValue:
            return compareKeyValueList(LHS.kvlist_value(), RHS.kvlist_value());
        case pb::AnyValue::kBytesValue:
            return LHS.bytes_value().compare(RHS.bytes_value());
        default:
            return 0;
    }
}

int compareKeyValue(const pb::KeyValue &LHS, const pb::KeyValue &RHS)
{
    int Result = LHS.key().compare(RHS.key());
    if (Result != 0)
        return Result;

    return compareAnyValue(LHS.value(), RHS.value());
}

int compareNumberDataPoint(const pb::NumberDataPoint &LHS, const pb::NumberDataPoint &RHS)
{
    // Compare attributes first
    if (LHS.attributes_size() != RHS.attributes_size())
        return LHS.attributes_size() - RHS.attributes_size();

    for (int Idx = 0; Idx < LHS.attributes_size(); ++Idx) {
        int Result = compareKeyValue(LHS.attributes(Idx), RHS.attributes(Idx));
        if (Result != 0)
            return Result;
    }

    if (LHS.start_time_unix_nano() != RHS.start_time_unix_nano())
        return (LHS.start_time_unix_nano() < RHS.start_time_unix_nano()) ? -1 : 1;

    if (LHS.time_unix_nano() != RHS.time_unix_nano())
        return (LHS.time_unix_nano() < RHS.time_unix_nano()) ? -1 : 1;

    if (LHS.value_case() != RHS.value_case())
        return LHS.value_case() - RHS.value_case();

    if (LHS.value_case() == pb::NumberDataPoint::kAsDouble)
        return (LHS.as_double() < RHS.as_double()) ? -1 : (LHS.as_double() > RHS.as_double()) ? 1 : 0;

    if (LHS.value_case() == pb::NumberDataPoint::kAsInt)
        return (LHS.as_int() < RHS.as_int()) ? -1 : (LHS.as_int() > RHS.as_int()) ? 1 : 0;

    return 0;
}

int compareMetric(const pb::Metric &LHS, const pb::Metric &RHS)
{
    int Result = LHS.name().compare(RHS.name());
    if (Result != 0)
        return Result;

    Result = LHS.description().compare(RHS.description());
    if (Result != 0)
        return Result;

    Result = LHS.unit().compare(RHS.unit());
    if (Result != 0)
        return Result;

    if (LHS.data_case() != RHS.data_case()) {
        return LHS.data_case() - RHS.data_case();
    }

    // Compare data points based on the type of metric
    switch (LHS.data_case()) {
        case pb::Metric::kGauge: {
            const auto &G1 = LHS.gauge();
            const auto &G2 = RHS.gauge();

            if (G1.data_points_size() != G2.data_points_size()) {
                return G1.data_points_size() - G2.data_points_size();
            }
            for (int Idx = 0; Idx < G1.data_points_size(); ++Idx) {
                Result = compareNumberDataPoint(G1.data_points(Idx), G2.data_points(Idx));
                if (Result != 0)
                    return Result;
            }

            break;
        }
        case pb::Metric::kSum: {
            const auto &S1 = LHS.sum();
            const auto &S2 = RHS.sum();

            if (S1.aggregation_temporality() != S2.aggregation_temporality())
                return S1.aggregation_temporality() - S2.aggregation_temporality();

            if (S1.is_monotonic() != S2.is_monotonic())
                return S1.is_monotonic() - S2.is_monotonic();

            if (S1.data_points_size() != S2.data_points_size())
                return S1.data_points_size() - S2.data_points_size();

            for (int Idx = 0; Idx < S1.data_points_size(); ++Idx) {
                Result = compareNumberDataPoint(S1.data_points(Idx), S2.data_points(Idx));
                if (Result != 0)
                    return Result;
            }

            break;
        }
        default:
            std::abort();
    }

    return 0;
}

int compareScopeMetrics(const pb::ScopeMetrics &LHS, const pb::ScopeMetrics &RHS)
{
    int Result = LHS.scope().name().compare(RHS.scope().name());
    if (Result != 0)
        return Result;

    Result = LHS.scope().version().compare(RHS.scope().version());
    if (Result != 0)
        return Result;

    if (LHS.metrics_size() != RHS.metrics_size())
        return LHS.metrics_size() - RHS.metrics_size();

    for (int Idx = 0; Idx < LHS.metrics_size(); ++Idx) {
        Result = compareMetric(LHS.metrics(Idx), RHS.metrics(Idx));
        if (Result != 0)
            return Result;
    }

    return 0;
}

int compareResourceMetrics(const pb::ResourceMetrics &LHS, const pb::ResourceMetrics &RHS)
{
    // Compare resource attributes
    {
        if (LHS.resource().attributes_size() != RHS.resource().attributes_size())
            return LHS.resource().attributes_size() - RHS.resource().attributes_size();

        for (int Idx = 0; Idx < LHS.resource().attributes_size(); ++Idx) {
            int Result = compareKeyValue(LHS.resource().attributes(Idx), RHS.resource().attributes(Idx));
            if (Result != 0)
                return Result;
        }
    }

    // Compare ScopeMetrics
    {
        if (LHS.scope_metrics_size() != RHS.scope_metrics_size())
            return LHS.scope_metrics_size() - RHS.scope_metrics_size();

        for (int Idx = 0; Idx < LHS.scope_metrics_size(); ++Idx) {
            int Result = compareScopeMetrics(LHS.scope_metrics(Idx), RHS.scope_metrics(Idx));
            if (Result != 0)
                return Result;
        }
    }

    return 0;
}

void sortAttributes(pb::RepeatedPtrField<pb::KeyValue> *Attrs)
{
    std::sort(Attrs->begin(), Attrs->end(), [](const auto &LHS, const auto &RHS) {
        return compareKeyValue(LHS, RHS) < 0;
    });
}

void sortDataPoints(pb::Metric &M)
{
    switch (M.data_case()) {
        case pb::Metric::kGauge: {
            auto *G = M.mutable_gauge();
            for (auto &DP : *G->mutable_data_points())
                sortAttributes(DP.mutable_attributes());
            break;
        }
        case pb::Metric::kSum: {
            auto *S = M.mutable_sum();
            for (auto &DP : *S->mutable_data_points())
                sortAttributes(DP.mutable_attributes());
            break;
        }
        default:
            std::abort();
    }

    switch (M.data_case()) {
        case pb::Metric::kGauge: {
            auto *G = M.mutable_gauge();
            std::sort(
                G->mutable_data_points()->begin(),
                G->mutable_data_points()->end(),
                [](const pb::NumberDataPoint &LHS, const pb::NumberDataPoint &RHS) {
                    return compareNumberDataPoint(LHS, RHS) < 0;
                });
            break;
        }
        case pb::Metric::kSum: {
            auto *S = M.mutable_sum();
            std::sort(
                S->mutable_data_points()->begin(),
                S->mutable_data_points()->end(),
                [](const pb::NumberDataPoint &LHS, const pb::NumberDataPoint &RHS) {
                    return compareNumberDataPoint(LHS, RHS) < 0;
                });
            break;
        }
        default:
            std::abort();
    }
}

void sortMetrics(pb::RepeatedPtrField<pb::Metric> *Arr)
{
    for (auto &M : *Arr) {
        sortAttributes(M.mutable_metadata());
        sortDataPoints(M);
    }

    std::sort(
        Arr->begin(), Arr->end(), [](const pb::Metric &LHS, const pb::Metric &RHS) {
            return compareMetric(LHS, RHS) < 0;
        });
}

void sortScopeMetrics(pb::RepeatedPtrField<pb::ScopeMetrics> *Arr) {
    for (auto &SMs : *Arr) {
        sortAttributes(SMs.mutable_scope()->mutable_attributes());
        sortMetrics(SMs.mutable_metrics());
    }

    std::sort(
        Arr->begin(),
        Arr->end(),
        [](const pb::ScopeMetrics &LHS, const pb::ScopeMetrics &RHS) { return compareScopeMetrics(LHS, RHS) < 0; });
}

void sortResourceMetrics(pb::RepeatedPtrField<pb::ResourceMetrics> *Arr) {
    for (auto &RMs : *Arr) {
        sortAttributes(RMs.mutable_resource()->mutable_attributes());
        sortScopeMetrics(RMs.mutable_scope_metrics());
    }

    std::sort(
        Arr->begin(),
        Arr->end(),
        [](const pb::ResourceMetrics &LHS, const pb::ResourceMetrics &RHS) {
            return compareResourceMetrics(LHS, RHS) < 0;
        });
}

void sortMetricsData(pb::MetricsData &MD)
{
    sortResourceMetrics(MD.mutable_resource_metrics());
}

} // namespace otel
