// SPDX-License-Identifier: GPL-3.0-or-later

#include "otel_chart.h"

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
