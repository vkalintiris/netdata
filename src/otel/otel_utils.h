#ifndef OTEL_UTILS_HPP
#define OTEL_UTILS_HPP

#include "libnetdata/blake3/blake3.h"

#include "opentelemetry/proto/metrics/v1/metrics.pb.h"
#include "metadata.h"

#include <iostream>
#include <vector>
#include <cstring>

namespace pb
{
using AnyValue = opentelemetry::proto::common::v1::AnyValue;
using ArrayValue = opentelemetry::proto::common::v1::ArrayValue;
using KeyValue = opentelemetry::proto::common::v1::KeyValue;
using KeyValueList = opentelemetry::proto::common::v1::KeyValueList;
using InstrumentationScope = opentelemetry::proto::common::v1::InstrumentationScope;

using Resource = opentelemetry::proto::resource::v1::Resource;

using MetricsData = opentelemetry::proto::metrics::v1::MetricsData;
using ResourceMetrics = opentelemetry::proto::metrics::v1::ResourceMetrics;
using ScopeMetrics = opentelemetry::proto::metrics::v1::ScopeMetrics;
using Metric = opentelemetry::proto::metrics::v1::Metric;
using NumberDataPoint = opentelemetry::proto::metrics::v1::NumberDataPoint;
using DataPointFlags = opentelemetry::proto::metrics::v1::DataPointFlags;
using SummaryDataPoint = opentelemetry::proto::metrics::v1::SummaryDataPoint;
using Exemplar = opentelemetry::proto::metrics::v1::Exemplar;
using HistogramDataPoint = opentelemetry::proto::metrics::v1::HistogramDataPoint;
using ExponentialHistogramDataPoint = opentelemetry::proto::metrics::v1::ExponentialHistogramDataPoint;

using Gauge = opentelemetry::proto::metrics::v1::Gauge;
using Sum = opentelemetry::proto::metrics::v1::Sum;
using Histogram = opentelemetry::proto::metrics::v1::Histogram;
using ExponentialHistogram = opentelemetry::proto::metrics::v1::ExponentialHistogram;
using Summary = opentelemetry::proto::metrics::v1::Summary;

template <typename Element> using RepeatedPtrField = google::protobuf::RepeatedPtrField<Element>;

template <typename Element> using ConstFieldIterator = typename RepeatedPtrField<Element>::const_iterator;

template <typename Element> using FieldIterator = typename RepeatedPtrField<Element>::const_iterator;

void restructureOTELMetrics(const otel::config::Config *Cfg, pb::MetricsData &MD);

void sortMetricsData(pb::MetricsData &MD);

class ScopeMetricsHasher;
class MetricHasher;

class ResourceMetricsHasher {
    friend void digestAttributes(const RepeatedPtrField<KeyValue> &KVs);

public:
    ScopeMetricsHasher hash(const ResourceMetrics &RMs);
};


class ScopeMetricsHasher {
    friend void digestAttributes(const RepeatedPtrField<KeyValue> &KVs);

public:
    ScopeMetricsHasher(blake3_hasher &BH) {
        this->BH = BH;
    }

    MetricHasher hash(const ScopeMetrics &RMs);

private:
    blake3_hasher BH;
};

class MetricHasher {
    friend void digestAttributes(const RepeatedPtrField<KeyValue> &KVs);

public:
    MetricHasher(blake3_hasher &BH) {
        this->BH = BH;
    }

    std::string hash(const Metric &M);

private:
    blake3_hasher BH;
};

} // namespace pb

#endif /* OTEL_UTILS_HPP */
