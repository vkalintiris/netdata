// SPDX-License-Identifier: GPL-3.0-or-later

#include "otel.h"
#include "otel_chart.h"
#include "otel_config.h"
#include "otel_flatten.h"
#include "otel_hash.h"
#include "otel_iterator.h"
#include "otel_process.h"
#include "otel_sort.h"
#include "otel_transform.h"
#include "otel_utils.h"

#include "libnetdata/required_dummies.h"

#include "absl/status/statusor.h"
#include "absl/strings/str_join.h"

#include "cli.h"

static absl::StatusOr<POPEN_INSTANCE *> runCommand(std::string Command)
{
    // FIXME: needs status report
    POPEN_INSTANCE *PI = spawn_popen_run(Command.c_str());
    if (!PI) {
        auto Msg = absl::StrFormat("Failed to run command '%s'", Command);
        return absl::FailedPreconditionError(Msg);
    }

    return PI;
}

static absl::Status stopCommand(POPEN_INSTANCE *PI)
{
    // FIXME: needs status report
    UNUSED(PI);
    return absl::Status();
}

absl::Status readExactly(int FD, char *Buffer, size_t N)
{
    size_t TotalBytesRead = 0;

    while (TotalBytesRead < N) {
        errno = 0;
        ssize_t BytesRead = read(FD, Buffer + TotalBytesRead, N - TotalBytesRead);

        if (BytesRead > 0) {
            TotalBytesRead += BytesRead;
            continue;
        }

        if (BytesRead == 0) {
            if (TotalBytesRead == 0) {
                return absl::OutOfRangeError("End of file reached");
            }

            return absl::OutOfRangeError(absl::StrFormat(
                "Unexpected EOF while reading from file descriptor %d, expected %zu bytes, got %zu",
                FD,
                N,
                TotalBytesRead));
        } 

        if (errno != EINTR) {
            std::string Msg = absl::StrFormat("Failed to read from file descriptor '%d': %s", FD, strerror(errno));
            return absl::InternalError(Msg);
        }
    }

    return absl::OkStatus();
}

class NetdataOtelOptions {
private:
    static std::string getOtelCollectorBinary()
    {
#if 0
        std::string Path = "/usr/local/bin/otelcontribcol";
        return config_get("otel", "otel collector binary", Path.c_str());
#else
        return "foo";
#endif
    }

    static std::string getOtelCollectorConfigFilename()
    {
#if 0
        std::string Path = absl::StrJoin({netdata_configured_user_config_dir, "otel-config.yaml"}, "/");
        return config_get("otel", "collector configuration file", Path.c_str());
#else
        return "foo";
#endif
    }

    static std::string getOtelReceiversConfigFilename()
    {
#if 0
        std::string Path = absl::StrJoin({netdata_configured_user_config_dir, "otel-receivers-config.yaml"}, "/");
        return config_get("otel", "receivers configuration file", Path.c_str());
#else
        return "foo";
#endif
    }

    static std::string getOtelMetricsPipePath()
    {
#if 0
        std::string Path = absl::StrJoin({netdata_configured_cache_dir, "otel-metrics.pipe"}, "/");
        return config_get("otel", "metrics file path", Path.c_str());
#else
        return "foo";
#endif
    }

public:
    NetdataOtelOptions()
    {
        CollectorBinary = getOtelCollectorBinary();
        CollectorConfig = getOtelCollectorConfigFilename();
        ReceiversConfig = getOtelCollectorConfigFilename();
        MetricsPipePath = getOtelMetricsPipePath();
    }

    std::string otelCollectorCommand() const
    {
        return absl::StrJoin({CollectorBinary.c_str(), "--config", CollectorConfig.c_str()}, " ");
    }

    std::string CollectorBinary;
    std::string CollectorConfig;
    std::string ReceiversConfig;
    std::string MetricsPipePath;
};

class PipeReader {
public:
    static absl::StatusOr<PipeReader> create(const std::string &Command, const std::string &PipePath)
    {
        if (unlink(PipePath.c_str()) != 0 && errno != ENOENT) {
            std::string Msg =
                absl::StrFormat("failed to unlink existing named pipe '%s': %s", PipePath, strerror(errno));
            return absl::InternalError(Msg);
        }

        if (mkfifo(PipePath.c_str(), 0666) == -1) {
            std::string Msg = absl::StrFormat("failed to create named pipe '%s': %s", PipePath, strerror(errno));
            return absl::InternalError(Msg);
        }

        auto PI = runCommand(Command);
        if (!PI.ok()) {
            return PI.status();
        }

        int FD = open(PipePath.c_str(), O_RDONLY);
        if (FD == -1) {
            std::string Msg = absl::StrFormat("failed to open named pipe '%s': %s", PipePath, strerror(errno));
            return absl::InternalError(Msg);
        }

        return PipeReader(PipePath, FD, *PI);
    }

private:
    explicit PipeReader(const std::string &Path, int FD, POPEN_INSTANCE *PI) : Path(Path), FD(FD), PI(PI)
    {
    }

public:
    absl::StatusOr<std::vector<char> > readMessage()
    {
        auto MessageSize = readU32();
        if (!MessageSize.ok()) {
            return MessageSize.status();
        }

        netdata_log_error("GVD: reading message of size: %u", *MessageSize);

        Message.clear();
        Message.resize(*MessageSize);

        auto S = readExactly(FD, Message.data(), Message.size());
        if (!S.ok()) {
            return S;
        }

        return Message;
    }

    absl::Status shutdown()
    {
        return stopCommand(PI);
    }

private:
    absl::StatusOr<uint32_t> readU32() const
    {
        char Buf[sizeof(uint32_t)] = { 0, 0, 0, 0};

        auto S = readExactly(FD, Buf, sizeof(uint32_t));
        if (!S.ok()) {
            return S;
        }

        uint32_t N;
        memcpy(&N, Buf, sizeof(uint32_t));
        return ntohl(N);
    }

private:
    std::string Path;
    int FD = -1;
    POPEN_INSTANCE *PI = nullptr;
    std::vector<char> Message;
};

void writeEnvToFile() {
    const char* Path = "/tmp/env.txt";
    std::ofstream OS(Path);

    if (!OS.is_open()) {
        std::cerr << "Error: Unable to open file " << Path << " for writing." << std::endl;
        return;
    }

    extern char **environ;
    for (char **env = environ; *env != nullptr; ++env) {
        OS << *env << std::endl;
    }

    OS.close();
}

#if 0
int main(int argc, char *argv[]) {
    UNUSED(argc);
    UNUSED(argv);

    writeEnvToFile();
    sleep(1);
    return 0;
    
    const NetdataOtelOptions NetdataOtelOpts;

    auto PR = PipeReader::create(NetdataOtelOpts.otelCollectorCommand(), NetdataOtelOpts.MetricsPipePath);
    if (!PR.ok()) {
        netdata_log_error("GVD: %s", PR.status().ToString().c_str());
        exit(EXIT_FAILURE);
    }

    while (true) {
        const auto &Msg = PR->readMessage();
        if (!Msg.ok()) {
            netdata_log_error("GVD: %s", Msg.status().ToString().c_str());
            // TODO: shutdown
            exit(EXIT_FAILURE);
        }

        netdata_log_error("GVD: message length: %zu", Msg->size());
    }

    exit(EXIT_FAILURE);
}
#else
int main(int argc, char *argv[]) {
    CLI::App app{"Netdata Configuration"};
    NetdataConfig config;

    config.set_defaults_from_env();
    config.add_options(app);

    CLI11_PARSE(app, argc, argv);

    // Access configuration values
    std::cout << "Cache Dir: " << config.get("NETDATA_CACHE_DIR") << std::endl;
    std::cout << "Hostname: " << config.get("NETDATA_HOSTNAME") << std::endl;
    return 0;
}
#endif
