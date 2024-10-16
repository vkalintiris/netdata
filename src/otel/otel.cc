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
#include "gtest/gtest.h"

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

struct Dimension {
    std::string Name;
    CircularBuffer<Sample> CB;

    void pushSample(const Sample &S)
    {
        CB.push(S);
    }

    Sample popSample()
    {
        return CB.pop();
    }

    size_t numSamples() const
    {
        return CB.size();
    }

    uint32_t startTime() const
    {
        return CB.head().TimePoint;
    }

    uint32_t updateEvery() const
    {
        uint32_t MinDelta = std::numeric_limits<uint32_t>::max();

        for (size_t Idx = 1; Idx < CB.size(); Idx++) {
            uint32_t Delta = CB[Idx].TimePoint - CB[Idx - 1].TimePoint;
            MinDelta = std::min(MinDelta, Delta);
        }

        return MinDelta;
    }

    bool empty() const
    {
        return CB.empty();
    }

    int compareCollectionTime(uint32_t LCT, uint32_t UpdateEvery) const
    {
        double StartTP = LCT + UpdateEvery - (UpdateEvery / 2.0);
        double EndTP = LCT + UpdateEvery + (UpdateEvery / 2.0);

        if (startTime() < StartTP)
            return -1;
        else if (startTime() >= EndTP)
            return 1;
        else
            return 0;
    }
};

template <> struct fmt::formatter<Dimension> {
    constexpr auto parse(format_parse_context &Ctx) -> decltype(Ctx.begin())
    {
        return Ctx.end();
    }

    template <typename FormatContext> auto format(const Dimension &D, FormatContext &Ctx) const -> decltype(Ctx.out())
    {
        return fmt::format_to(Ctx.out(), "{}", D.CB);
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

    void initialize(BlakeId &Id, const std::string &Name)
    {
        this->BID = Id;
        this->Name = Name;
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
        ND.pushSample(S);
    }

    void process(size_t RampUpThreshold, size_t GapThreshold)
    {
        bool Processed = false;

        if (UpdateEvery.has_value()) {
            Processed = processFastPath();
        }

        if (!Processed)
            processSlowPath(RampUpThreshold, GapThreshold);
    }

    bool processFastPath()
    {
        assert(
            UpdateEvery.has_value() && UpdateEvery.value() && LastCollectedTime.has_value() &&
            LastCollectedTime.value());

        absl::InlinedVector<std::pair<std::string, Sample>, 4> IV;

        bool Ok = false;
        while (true) {
            for (auto &[Name, Dim] : Dimensions) {
                if (Dim.empty()) {
                    return Ok;
                }

                if (Dim.compareCollectionTime(LastCollectedTime.value(), UpdateEvery.value())) {
                    return Ok;
                }

                IV.emplace_back(Name, Dim.popSample());

                IV.clear();
                Ok = true;
            }
        };
    }

    void processSlowPath(size_t RampUpThreshold, size_t GapThreshold)
    {
        UNUSED(GapThreshold);

        assert(RampUpThreshold >= 2);
        assert(!Dimensions.empty());
        assert(!Dimensions.begin()->second.empty());

        if (!UpdateEvery.has_value()) {
            if (maxDataPointsInDimensions() < RampUpThreshold) {
                return;
            }

            UpdateEvery = minUpdateEveryInDimensions();
            assert(UpdateEvery != 0);
            LastCollectedTime = minStartTimeInDimensions() - UpdateEvery.value();
            assert(LastCollectedTime != 0);
        }

        dropPastCollectionTimes();

        if (maxDataPointsInDimensions() < GapThreshold) {
            return;
        }

        UpdateEvery = minUpdateEveryInDimensions();
        assert(UpdateEvery != 0);
        LastCollectedTime = minStartTimeInDimensions() - UpdateEvery.value();
        assert(LastCollectedTime != 0);
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
    size_t maxDataPointsInDimensions() const
    {
        size_t N = std::numeric_limits<size_t>::min();

        for (const auto &[Name, Dim] : Dimensions) {
            N = std::max(N, Dim.numSamples());
        }

        return N;
    }

    uint32_t minUpdateEveryInDimensions() const
    {
        uint32_t UE = std::numeric_limits<uint32_t>::max();

        for (const auto &[Name, Dim] : Dimensions) {
            UE = std::min(UE, Dim.updateEvery());
        }

        return UE;
    }

    uint32_t minStartTimeInDimensions() const
    {
        uint32_t TP = std::numeric_limits<uint32_t>::max();

        for (const auto &[Name, Dim] : Dimensions) {
            TP = std::min(TP, Dim.startTime());
        }

        return TP;
    }

    void dropPastCollectionTimes()
    {
        assert(
            UpdateEvery.has_value() && UpdateEvery.value() && LastCollectedTime.has_value() &&
            LastCollectedTime.value());

        double UE = UpdateEvery.value();
        double LCT = LastCollectedTime.value();

        double StartTP = LCT + UE / 2.0;

        for (auto &[Name, Dim] : Dimensions) {
            while (Dim.startTime() < StartTP) {
                Dim.popSample();
            }
        }
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

#if 0
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
class ChartTest : public ::testing::Test {
protected:
    Chart chart;

    void SetUp() override
    {
        // Initialize the chart with a test configuration
        BlakeId testId = {0}; // Assume BlakeId is an array or similar
        chart.initialize(testId, "TestMetric");
    }
};

TEST_F(ChartTest, AddDataPoints)
{
    Chart C;
    BlakeId BID = { 0 };
    C.initialize(BID, "foo");

    {
        std::vector<pb::NumberDataPoint> V;

        for (size_t Idx = 0; Idx != 10; Idx++) {
            pb::NumberDataPoint NDP;
            NDP.set_time_unix_nano(Idx * 1000000000);
            NDP.set_as_int(Idx);
            V.push_back(NDP);

            OtelElement OE;
            OE.DP = DataPoint(&V.back());
            C.add(OE);
        }

        C.process(3, 5);
    }
}

int main(int argc, char *argv[])
{
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
#endif
