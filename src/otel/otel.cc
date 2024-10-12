// SPDX-License-Identifier: GPL-3.0-or-later

#include "circular_buffer.h"

#include "absl/container/flat_hash_map.h"
#include "absl/container/inlined_vector.h"
#include "google/protobuf/arena.h"

#include "fmt_utils.h"
#include "libnetdata/blake3/blake3.h"
#include "otel_utils.h"
#include "otel_config.h"
#include "otel_iterator.h"
#include "otel_hash.h"

#include "libnetdata/required_dummies.h"

#include "CLI/CLI.hpp"
#include "opentelemetry/proto/collector/metrics/v1/metrics_service.grpc.pb.h"
#include "grpcpp/grpcpp.h"

#include <chrono>
#include <iostream>
#include <limits>
#include <memory>

using grpc::Server;
using grpc::Status;

#include <google/protobuf/repeated_field.h>
#include <opentelemetry/proto/common/v1/common.pb.h>
#include <string>

static google::protobuf::ArenaOptions ArenaOpts = {
    .start_block_size = 16 * 1024 * 1024,
    .max_block_size = 512 * 1024 * 1024,
};

static void printClientMetadata(const grpc::ServerContext *context)
{
    const auto &client_metadata = context->client_metadata();
    for (const auto &pair : client_metadata) {
        std::cout << "Key: " << pair.first << ", Value: " << pair.second << std::endl;
    }
}

struct Sample {
    uint64_t Value;
    uint32_t TimePoint;
};

template <> struct fmt::formatter<Sample> {
    constexpr auto parse(format_parse_context &Ctx) -> decltype(Ctx.begin())
    {
        return Ctx.end();
    }

    template <typename FormatContext> auto format(const Sample &S, FormatContext &Ctx) const -> decltype(Ctx.out())
    {
        return fmt::format_to(Ctx.out(), "[{}]={}", S.TimePoint, static_cast<double>(S.Value) / 1000);
    }
};

class DataPoints {
public:
    uint32_t startTime() const {
        return CB.head().TimePoint;
    }

    uint32_t endTime() const {
        return CB.tail().TimePoint;
    }

    uint32_t updateEvery() const {
        uint32_t MinDelta = std::numeric_limits<uint32_t>::max();

        for (size_t Idx = 1; Idx < CB.size(); ++Idx) {
            uint32_t Delta = CB[Idx].TimePoint - CB[Idx - 1].TimePoint;
            if (Delta < MinDelta) {
                MinDelta = Delta;
            }
        }

        return MinDelta;
    }

    void add(const Sample &S) {
        CB.push(S);
    }

    bool empty() const {
        return CB.empty();
    }

private:
    CircularBuffer<Sample> CB;
};

struct Dimension {
    uint32_t startTime() const
    {
        return DPs.startTime();
    }

    uint32_t endTime() const
    {
        return DPs.endTime();
    }

    uint32_t updateEvery() const
    {
        return DPs.updateEvery();
    }

    void addSample(const Sample &S) {
        DPs.add(S);
    }

    std::string Name;
    mutable DataPoints DPs;
};

template <> struct fmt::formatter<Dimension> {
    constexpr auto parse(format_parse_context &Ctx) -> decltype(Ctx.begin())
    {
        return Ctx.end();
    }

    template <typename FormatContext> auto format(const Dimension &D, FormatContext &Ctx) const -> decltype(Ctx.out())
    {
        return fmt::format_to(Ctx.out(), "{}", D.DPs);
    }
};

class Chart {
public:
    void initialize(BlakeId &Id, const pb::ResourceMetrics *RM, const pb::ScopeMetrics *SM, const pb::Metric *M)
    {
        UNUSED(RM);
        UNUSED(SM);

        this->BID = Id;
        Name = M->name();
        Committed = false;
    }

    void add(const OtelElement &OE)
    {
        absl::string_view DimName = "value";
        if (auto Result = OE.name(); Result.ok()) {
            DimName = *Result.value();
        }

        auto [It, Emplaced] = Dimensions.try_emplace(DimName.data());
        auto &ND = It->second;

        if (Emplaced) {
            ND.Name = DimName;
            Committed = false;
        }

        uint32_t TP = static_cast<uint32_t>(OE.time() / std::chrono::nanoseconds::period::den);
        Sample S{OE.value(1000), TP};
        ND.addSample(S);
    }

    void process(size_t RampUpThreshold, size_t GapThreshold)
    {
        assert(RampUpThreshold >= 2);
        assert(!Dimensions.empty());
        assert(!Dimensions.begin()->second.DPs.empty());

        if (!UpdateEvery.has_value()) {
            const auto &ND = Dimensions.begin()->second;
            if (ND.Samples.size() < RampUpThreshold) {
                return;
            }

            UpdateEvery = ND.updateEvery();
            LastCollectedTime = ND.startTime() - UpdateEvery.value();
        }

        auto [StartTime, EndTime] = timeRange();

    }

    const std::string &name() const
    {
        return Name;
    }

    const absl::flat_hash_map<std::string, Dimension> &dimensions() const
    {
        return Dimensions;
    }

private:
    std::pair<uint32_t, uint32_t> timeRange() const {
        uint32_t StartTime = std::numeric_limits<uint32_t>::max();
        uint32_t EndTime = std::numeric_limits<uint32_t>::min();

        for (const auto &[Name, Dim]: Dimensions) {
            StartTime = std::min(StartTime, Dim.startTime());
            EndTime = std::max(EndTime, Dim.endTime());
        }

        return { StartTime, EndTime };
    }

    uint32_t updateEvery() const {
        uint32_t UE = std::numeric_limits<uint32_t>::max();

        for (const auto &[Name, Dim]: Dimensions) {
            UE = std::min(UE, Dim.updateEvery());
        }

        return UE;
    }

    absl::StatusOr<uint32_t> numSamplesPerDimension() const {
        uint32_t NumSamples = 0;

        for (const auto &[Name, Dim]: Dimensions) {
            if (NumSamples == 0) {
                NumSamples = Dim.Samples.size();
            } else if (NumSamples != Dim.Samples.size()) {
                return false;
            }
        }

        return true;
    }

private:
    BlakeId BID;
    std::string Name;

    absl::flat_hash_map<std::string, Dimension> Dimensions;
    std::optional<uint32_t> UpdateEvery;
    std::optional<uint32_t> LastCollectedTime;
    bool Committed = false;
};

template <> struct fmt::formatter<Chart> {
    constexpr auto parse(format_parse_context &Ctx) -> decltype(Ctx.begin())
    {
        return Ctx.end();
    }

    template <typename FormatContext> auto format(const Chart &C, FormatContext &Ctx) const -> decltype(Ctx.out())
    {
        return fmt::format_to(Ctx.out(), "{}: {}", C.name(), C.dimensions());
    }
};

class MetricsServiceImpl final : public opentelemetry::proto::collector::metrics::v1::MetricsService::Service {
    using ExportMetricsServiceRequest = opentelemetry::proto::collector::metrics::v1::ExportMetricsServiceRequest;
    using ExportMetricsServiceResponse = opentelemetry::proto::collector::metrics::v1::ExportMetricsServiceResponse;

public:
    MetricsServiceImpl(otel::Config *Cfg) : Cfg(Cfg), Arena(ArenaOpts), Counter(0)
    {
    }

    Status Export(
        grpc::ServerContext *Ctx,
        const ExportMetricsServiceRequest *Request,
        ExportMetricsServiceResponse *Response) override
    {
        (void)Ctx;
        (void)Response;

        fmt::println(
            "{} Received {} resource metrics ({} KiB)",
            Counter++,
            Request->resource_metrics_size(),
            Request->ByteSizeLong() / 1024);

        // Fill data
        OtelData OD(Cfg, &Request->resource_metrics());
        std::vector<OtelElement> Elements(OD.begin(), OD.end());
        std::sort(Elements.begin(), Elements.end());
        for (const OtelElement &OE : Elements) {
            BlakeId BID = OE.chartHash();

            auto [It, Emplaced] = PendingCharts.try_emplace(BID);
            auto &NC = It->second;

            if (Emplaced) {
                NC.initialize(BID, OE.RM, OE.SM, OE.M);
            }

            NC.add(OE);
        }

        for (auto &[BID, NC] : PendingCharts) {
            NC.process(10, 100);
        }

        return Status::OK;
    }

private:
    otel::Config *Cfg;
    pb::Arena Arena;
    size_t Counter;
    absl::flat_hash_map<BlakeId, Chart> PendingCharts;
};

static void RunServer(otel::Config *Cfg)
{
    std::string Address("localhost:21212");
    MetricsServiceImpl MS(Cfg);

    grpc::ServerBuilder Builder;
    Builder.AddListeningPort(Address, grpc::InsecureServerCredentials());
    Builder.RegisterService(&MS);

    std::unique_ptr<Server> Srv(Builder.BuildAndStart());
    std::cout << "Server listening on " << Address << std::endl;
    Srv->Wait();
}

#if 1
int main(int argc, char **argv)
{
    CLI::App App{"OTEL plugin"};

    std::string Path = "/home/vk/repos/nd/otel/src/otel/otel-receivers-config.yaml";
    App.add_option("--config", Path, "Path to the receivers configuration file");

    CLI11_PARSE(App, argc, argv);

    absl::StatusOr<otel::Config *> Cfg = otel::Config::load(Path);
    if (!Cfg.ok()) {
        fmt::print(stderr, "{}\n", Cfg.status().ToString());
        return 1;
    }

    RunServer(*Cfg);
    return 0;
}
#else
int main()
{
    // String value
    pb::AnyValue string_value;
    string_value.set_string_value("Hello, OpenTelemetry!");
    std::cout << fmt::format("String value: {}\n", string_value);

    // Boolean value
    pb::AnyValue bool_value;
    bool_value.set_bool_value(true);
    std::cout << fmt::format("Boolean value: {}\n", bool_value);

    // Integer value
    pb::AnyValue int_value;
    int_value.set_int_value(42);
    std::cout << fmt::format("Integer value: {}\n", int_value);

    // Double value
    pb::AnyValue double_value;
    double_value.set_double_value(3.14159);
    std::cout << fmt::format("Double value: {}\n", double_value);

    // Array value
    pb::AnyValue array_value;
    auto *array = array_value.mutable_array_value();
    array->add_values()->set_string_value("one");
    array->add_values()->set_int_value(2);
    array->add_values()->set_bool_value(true);
    std::cout << fmt::format("Array value: {}\n", array_value);

    // KeyValueList value
    {
        pb::AnyValue kvlist_value;
        auto *kvlist = kvlist_value.mutable_kvlist_value();

        auto *kv1 = kvlist->add_values();
        kv1->set_key("key1");
        kv1->mutable_value()->set_string_value("value1");

        auto *kv2 = kvlist->add_values();
        kv2->set_key("key2");
        kv2->mutable_value()->set_int_value(42);

        auto *kv3 = kvlist->add_values();
        kv3->set_key("key3");
        auto *nested_kvlist = kv3->mutable_value()->mutable_kvlist_value();

        // Adding values to the nested KeyValueList
        auto *nested_kv1 = nested_kvlist->add_values();
        nested_kv1->set_key("nested_key1");
        nested_kv1->mutable_value()->set_string_value("nested_value1");

        auto *nested_kv2 = nested_kvlist->add_values();
        nested_kv2->set_key("nested_key2");
        nested_kv2->mutable_value()->set_double_value(3.14);

        std::cout << fmt::format("KVList value: {}\n", kvlist_value);
    }

    // Bytes value
    pb::AnyValue bytes_value;
    bytes_value.set_bytes_value("\x00\x01\x02\x03\x04");
    std::cout << fmt::format("Bytes value: {}\n", bytes_value);

    // Unknown value
    pb::AnyValue unknown_value;
    std::cout << fmt::format("Unknown value: {}\n", unknown_value);

    // Instrumentation scope
    {
        pb::InstrumentationScope IS;
        IS.set_name("example_scope");
        IS.set_version("1.0.0");
        auto *attr = IS.add_attributes();
        attr->set_key("example_key");
        attr->mutable_value()->set_string_value("example_value");
        IS.set_dropped_attributes_count(2);

        std::cout << fmt::format("{}\n", IS);
    }

    // Resource
    {
        pb::Resource Res;

        auto *A1 = Res.add_attributes();
        A1->set_key("example_key");
        A1->mutable_value()->set_string_value("example_value");

        Res.set_dropped_attributes_count(2);

        std::cout << fmt::format("{}\n", Res);
    }

    // Exemplar
    {
        opentelemetry::proto::metrics::v1::Exemplar E;
        E.set_time_unix_nano(1234567890);
        E.set_as_double(42.0);
        auto *Attr = E.add_filtered_attributes();
        Attr->set_key("example_key");
        Attr->mutable_value()->set_string_value("example_value");
        E.set_span_id(std::string("\x01\x02\x03\x04\x05\x06\x07\x08", 8));
        E.set_trace_id(std::string("\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0A\x0B\x0C\x0D\x0E\x0F\x10", 16));

        std::cout << fmt::format("{}\n", E);
    }

    // NumberDataPoint
    {
        unsigned long long TS = (unsigned long long)1635724810000000000;
        pb::NumberDataPoint NDP;
        NDP.set_start_time_unix_nano(TS);
        NDP.set_time_unix_nano(TS);
        NDP.set_as_double(42.0);
        auto *attr = NDP.add_attributes();
        attr->set_key("example_key");
        attr->mutable_value()->set_string_value("example_value");
        NDP.set_flags(1);

        // Add an exemplar
        auto *E = NDP.add_exemplars();
        E->set_time_unix_nano(1234567890);
        E->set_as_double(42.0);

        auto *Attr = E->add_filtered_attributes();
        Attr->set_key("example_key");
        Attr->mutable_value()->set_string_value("example_value");
        E->set_span_id(std::string("\x01\x02\x03\x04\x05\x06\x07\x08", 8));
        E->set_trace_id(std::string("\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0A\x0B\x0C\x0D\x0E\x0F\x10", 16));

        std::cout << fmt::format("{}\n", NDP);
    }

    // Gauge/Sum
    {
        unsigned long long TS = (unsigned long long)1635724810000000000;

        opentelemetry::proto::metrics::v1::Gauge gauge;
        auto dataPoint = gauge.add_data_points();
        dataPoint->set_start_time_unix_nano(TS);
        dataPoint->set_time_unix_nano(TS);
        dataPoint->set_as_double(42.0);

        std::cout << fmt::format("{}\n", gauge);

        opentelemetry::proto::metrics::v1::Sum sum;
        sum.set_aggregation_temporality(
            opentelemetry::proto::metrics::v1::AggregationTemporality::AGGREGATION_TEMPORALITY_CUMULATIVE);
        sum.set_is_monotonic(true);
        auto sumDataPoint = sum.add_data_points();
        sumDataPoint->set_start_time_unix_nano(TS);
        sumDataPoint->set_time_unix_nano(TS);
        sumDataPoint->set_as_double(100.0);

        std::cout << fmt::format("{}\n", sum);
    }

    return 0;
}
#endif
