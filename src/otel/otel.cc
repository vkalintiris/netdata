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

static int RunServer(otel::Config *Cfg)
{
    std::string Address("localhost:21212");
    MetricsServiceImpl MS(Cfg);

    grpc::ServerBuilder Builder;
    Builder.AddListeningPort(Address, grpc::InsecureServerCredentials());
    Builder.RegisterService(&MS);

    std::unique_ptr<Server> Srv(Builder.BuildAndStart());
    std::cout << "Server listening on " << Address << std::endl;
    Srv->Wait();
    return 0;
}

#ifdef HAVE_GTEST
int otel_gtests_main(int argc, char *argv[]) {
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
#else
static int otel_gtests_main(int argc, char *argv[]) {
    (void) argc;
    (void) argv;
    return 0;
}
#endif

int main(int argc, char **argv)
{
    CLI::App app{"OTEL plugin"};

    std::string path = "/home/vk/repos/nd/otel/src/otel/otel-receivers-config.yaml";
    app.add_option("--config", path, "Path to the receivers configuration file");

    bool run_tests = false;
#ifdef HAVE_GTEST
    app.add_flag("--test", run_tests, "Run tests");
#endif

    CLI11_PARSE(app, argc, argv);

    if (run_tests) {
        return otel_gtests_main(argc, argv);
    }
    
    absl::StatusOr<otel::Config *> cfg = otel::Config::load(path);
    if (!cfg.ok()) {
        fmt::print(stderr, "{}\n", cfg.status().ToString());
        return 1;
    }

    return RunServer(*cfg);
}
