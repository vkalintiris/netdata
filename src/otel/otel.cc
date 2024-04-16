#include "daemon/common.h"

#include "libnetdata/log/log.h"
#include <fstream>
#include <google/protobuf/repeated_field.h>
#include <iomanip>
#include <sstream>

#include "otel_utils.h"

#include <yaml-cpp/yaml.h>
#include "resource_attributes.h"

enum class InitStatus : unsigned int {
    Uninitialized = 0,
    HaveLoop = 1 << 0,
    HaveAsync = 1 << 1,
    HaveCompletion = 1 << 2,
    HaveMetricsFifo = 1 << 3,
    HaveLogsFifo = 1 << 4,
    HaveTracesFifo = 1 << 5,
    HaveSpawnedCollector = 1 << 6,
    HaveRunLoop = 1 << 7,
};

inline InitStatus operator|(InitStatus lhs, InitStatus rhs)
{
    return static_cast<InitStatus>(
        static_cast<std::underlying_type<InitStatus>::type>(lhs) |
        static_cast<std::underlying_type<InitStatus>::type>(rhs));
}

inline InitStatus operator&(InitStatus lhs, InitStatus rhs)
{
    return static_cast<InitStatus>(
        static_cast<std::underlying_type<InitStatus>::type>(lhs) &
        static_cast<std::underlying_type<InitStatus>::type>(rhs));
}

inline InitStatus &operator|=(InitStatus &lhs, InitStatus rhs)
{
    lhs = lhs | rhs;
    return lhs;
}

typedef enum {
    OTEL_FIFO_KIND_METRICS,
    OTEL_FIFO_KIND_LOGS,
    OTEL_FIFO_KIND_TRACES,
} otel_fifo_kind_t;

static const char *otel_fifo_kind_to_string(otel_fifo_kind_t otel_fifo_kind)
{
    switch (otel_fifo_kind) {
        case OTEL_FIFO_KIND_METRICS:
            return "metrics";
        case OTEL_FIFO_KIND_LOGS:
            return "logs";
        case OTEL_FIFO_KIND_TRACES:
            return "traces";
    }

    fatal("Unknown fifo kind: %d", otel_fifo_kind);
}

typedef struct {
    otel_fifo_kind_t kind;
    const char *path;
    int fd;
    uv_pipe_t *pipe;
} otel_fifo_t;

typedef struct otel_state {
    InitStatus init_status;

    uv_loop_t *loop;
    uv_process_t otel_process;
    struct completion otel_process_completion;

    otel_fifo_t metrics_fifo;
    otel_fifo_t logs_fifo;
    otel_fifo_t traces_fifo;

    uv_async_t async;
    struct completion shutdown_completion;

    unsigned loop_counter;

    bool haveLoop() const
    {
        return haveInitStatus(InitStatus::HaveLoop);
    }

    bool haveMetricsFifo() const
    {
        return haveInitStatus(InitStatus::HaveMetricsFifo);
    }

    bool haveLogsFifo() const
    {
        return haveInitStatus(InitStatus::HaveLogsFifo);
    }

    bool haveTracesFifo() const
    {
        return haveInitStatus(InitStatus::HaveTracesFifo);
    }

    bool haveAllFifos() const
    {
        return haveMetricsFifo() && haveLogsFifo() && haveTracesFifo();
    }

    bool haveSpawnedCollector() const
    {
        return haveInitStatus(InitStatus::HaveSpawnedCollector);
    }

    bool haveRunLoop() const
    {
        return haveInitStatus(InitStatus::HaveRunLoop);
    }

    bool haveAsync() const
    {
        return haveInitStatus(InitStatus::HaveAsync);
    }

    void dump() const
    {
        netdata_log_error(
            "[GVD] loop: %d, metrics: %d, logs: %d, traces: %d, spawned: %d, run: %d, async: %d (raw=%u)",
            haveLoop(),
            haveMetricsFifo(),
            haveLogsFifo(),
            haveTracesFifo(),
            haveSpawnedCollector(),
            haveRunLoop(),
            haveAsync(),
            static_cast<unsigned int>(init_status));
    }

private:
    bool haveInitStatus(const InitStatus Flag) const
    {
        return (init_status & Flag) != InitStatus::Uninitialized;
    }
} otel_state_t;

static otel_state_t otel_state;

static void alloc_buffer(uv_handle_t *handle, size_t suggested_size, uv_buf_t *buf)
{
    UNUSED(handle);

    suggested_size = 16 * 1024 * 1024;

    char *ptr = static_cast<char *>(callocz(suggested_size, sizeof(char)));
    if (!ptr)
        fatal("[OTEL] Could not allocate buffer for libuv");

    *buf = uv_buf_init(ptr, suggested_size);
}

// Function to convert a byte to a hex string
static std::string byteToHex(unsigned char byte)
{
    std::ostringstream oss;
    oss << std::hex << std::setfill('0') << std::setw(2) << static_cast<int>(byte);
    return oss.str();
}

// Function to convert binary data to a hex string
static std::string dataToHexString(const char *data, std::size_t len)
{
    std::ostringstream oss;
    for (std::size_t i = 0; i < len; ++i) {
        oss << byteToHex(data[i]);
    }
    return oss.str();
}

template <typename T> class BufferManager {
public:
    void fill(const uv_buf_t &buf)
    {
        if (pos > data.size())
            fatal("invalid position");
        else if (pos == data.size()) {
            netdata_log_error("OTEL GVD: clearing data: pos = data.size() = %zu", pos);
            data.clear();
        } else if (pos != 0) {
            data.erase(data.begin(), data.begin() + pos);
            netdata_log_error("OTEL GVD: erasing first %zu bytes", pos);
        }

        pos = 0;
        data.insert(data.end(), buf.base, buf.base + buf.len);
    }

    bool getMessages(std::vector<T> &messages)
    {
        T message;

        while (readMessage(message))
            messages.push_back(message);

        return remainingBytes() == 0;
    }

private:
    bool readMessage(T &message)
    {
        uv_buf_t dst = {.base = nullptr, .len = 0};

        if (!haveAtLeastXBytes(2 * sizeof(uint32_t)))
            return false;

        uint32_t bytes = 0;
        memcpy(&bytes, &data[pos], sizeof(uint32_t));
        bytes = ntohl(bytes);
        pos += sizeof(uint32_t);

        uint32_t checksum_go = 0;
        memcpy(&checksum_go, &data[pos], sizeof(uint32_t));
        checksum_go = ntohl(checksum_go);
        pos += sizeof(uint32_t);

        if (!haveAtLeastXBytes(bytes)) {
            pos -= 2 * sizeof(uint32_t);
            return false;
        }

        dst.base = &data[pos];
        dst.len = bytes;

        uint32_t checksum_cpp = 0;
        for (size_t i = 0; i != dst.len; i++)
            checksum_cpp += (unsigned char)dst.base[i];

        if (checksum_cpp != checksum_go) {
            std::ofstream OS("/tmp/cpp.bin", std::ios::out);
            OS << "message bytes: " << bytes << "\n";
            OS << std::hex << "checksum (go): " << checksum_go << "\n";
            OS << "checksum (c++): " << checksum_cpp << "\n";
            OS << "data: " << dataToHexString(dst.base, dst.len) << "\n\n";
            OS.close();

            fatal("Checksum mismatch cpp = %u, go = %u", checksum_cpp, checksum_go);
        } else {
            netdata_log_error(
                "Checksum matches sum (%u == %u). message length: %zu bytes", checksum_go, checksum_cpp, dst.len);
        }

        if (!message.ParseFromArray(dst.base, dst.len))
            fatal("Failed to parse protobuf message");

        pos += bytes;
        return true;
    }

    inline size_t remainingBytes() const
    {
        return data.size() - pos;
    }

    inline bool haveAtLeastXBytes(uint32_t bytes) const
    {
        return remainingBytes() >= bytes;
    }

private:
    std::vector<char> data;
    size_t pos = {0};
};

struct OtelElement {
    const pb::MetricsData *MD;
    const pb::ResourceMetrics *RM;
    const pb::ScopeMetrics *SM;
    const pb::Metric *M;

    union {
        const pb::NumberDataPoint *NDP;
        const pb::SummaryDataPoint *SDP;
        const pb::HistogramDataPoint *HDP;
        const pb::ExponentialHistogramDataPoint *EHDP;
    };
};

enum class DataPointKind {
    Number,
    Sum,
    Summary,
    Histogram,
    Exponential,

    NotAvailable,
};

class OtelIterator {
public:
    using MetricsDataIterator = typename std::vector<pb::MetricsData>::const_iterator;
    using ResourceMetricsIterator = typename pb::ConstFieldIterator<pb::ResourceMetrics>;
    using ScopeMetricsIterator = typename pb::ConstFieldIterator<pb::ScopeMetrics>;
    using MetricsIterator = typename pb::ConstFieldIterator<pb::Metric>;

    using NumberDataPointIterator = typename pb::ConstFieldIterator<pb::NumberDataPoint>;
    using SummaryDataPointIterator = typename pb::ConstFieldIterator<pb::SummaryDataPoint>;
    using HistogramDataPointIterator = typename pb::ConstFieldIterator<pb::HistogramDataPoint>;
    using ExponentialHistogramDataPointIterator = typename pb::ConstFieldIterator<pb::ExponentialHistogramDataPoint>;

    union DataPointIterator {
        NumberDataPointIterator NDPIt;
        SummaryDataPointIterator SDPIt;
        HistogramDataPointIterator HDPIt;
        ExponentialHistogramDataPointIterator EHDPIt;

        DataPointIterator()
        {
        }
        ~DataPointIterator()
        {
        }
    };

    OtelIterator(MetricsDataIterator MDBegin, MetricsDataIterator MDEnd)
        : MDIt(MDBegin), MDEnd(MDEnd), DPKind(DataPointKind::NotAvailable)
    {
        if (MDIt != MDEnd) {
            RMIt = MDIt->resource_metrics().begin();
            RMEnd = MDIt->resource_metrics().end();

            if (RMIt != RMEnd) {
                SMIt = RMIt->scope_metrics().begin();
                SMEnd = RMIt->scope_metrics().end();

                if (SMIt != SMEnd) {
                    MIt = SMIt->metrics().begin();
                    MEnd = SMIt->metrics().end();

                    if (MIt != MEnd) {
                        const pb::Metric &M = *MIt;
                        initializeDataPointIterator(M);
                    } else {
                        netdata_log_error("OtelIterator(): m it == m end");
                    }

                } else {
                    netdata_log_error("OtelIterator(): sm it == sm end");
                }
            } else {
                netdata_log_error("OtelIterator(): rm it == rm end");
            }
        } else {
            netdata_log_error("OtelIterator(): md it == md end");
        }
    }

    ~OtelIterator()
    {
        destroyCurrentIterator();
    }

    inline bool hasNext() const
    {
        if (MDIt == MDEnd) {
            netdata_log_error("hasNext(): md it = md end");
            return false;
        }

        if (RMIt == RMEnd) {
            netdata_log_error("hasNext(): rm it = rm end");
            return false;
        }

        if (SMIt == SMEnd) {
            netdata_log_error("hasNext(): sm it = sm end");
            return false;
        }

        if (MIt == MEnd) {
            netdata_log_error("hasNext(): m it = m end");
            return false;
        }

        switch (DPKind) {
            case DataPointKind::Number:
            case DataPointKind::Sum:
                if (DPIt.NDPIt == DPEnd.NDPIt)
                    netdata_log_error("hasNext(): ndp it = ndp end");
                return DPIt.NDPIt != DPEnd.NDPIt;
            case DataPointKind::Summary:
                if (DPIt.SDPIt == DPEnd.SDPIt)
                    netdata_log_error("hasNext(): sdp it = sdp end");
                return DPIt.SDPIt != DPEnd.SDPIt;
            case DataPointKind::Histogram:
                if (DPIt.HDPIt == DPEnd.HDPIt)
                    netdata_log_error("hasNext(): hdp it = hdp end");
                return DPIt.HDPIt != DPEnd.HDPIt;
            case DataPointKind::Exponential:
                if (DPIt.EHDPIt == DPEnd.EHDPIt)
                    netdata_log_error("hasNext(): ehdp it = ehdp end");
                return DPIt.EHDPIt != DPEnd.EHDPIt;
            case DataPointKind::NotAvailable:
                netdata_log_error("hasNext(): not available dp kind");
                return false;
            default:
                throw std::out_of_range("WTF?");
        }
    }

    OtelElement next()
    {
        if (!hasNext())
            throw std::out_of_range("No more elements");

        // Fill element
        OtelElement OE;
        {
            OE.MD = &*MDIt;
            OE.RM = &*RMIt;
            OE.SM = &*SMIt;
            OE.M = &*MIt;

            switch (DPKind) {
                case DataPointKind::Number:
                case DataPointKind::Sum:
                    OE.NDP = &*DPIt.NDPIt;
                    break;
                case DataPointKind::Summary:
                    OE.SDP = &*DPIt.SDPIt;
                    break;
                case DataPointKind::Histogram:
                    OE.HDP = &*DPIt.HDPIt;
                    break;
                case DataPointKind::Exponential:
                    OE.EHDP = &*DPIt.EHDPIt;
                    break;
                case DataPointKind::NotAvailable:
                default:
                    throw std::out_of_range("No more elements");
            }
        }

        advance();
        return OE;
    }

private:
    MetricsDataIterator MDIt, MDEnd;
    ResourceMetricsIterator RMIt, RMEnd;
    ScopeMetricsIterator SMIt, SMEnd;
    MetricsIterator MIt, MEnd;

    DataPointIterator DPIt, DPEnd;
    DataPointKind DPKind;

    inline DataPointKind dpKind(const pb::Metric &M) const
    {
        if (M.has_gauge())
            return DataPointKind::Number;
        if (M.has_sum())
            return DataPointKind::Sum;
        if (M.has_summary())
            return DataPointKind::Summary;
        else if (M.has_histogram())
            return DataPointKind::Histogram;
        else if (M.has_exponential_histogram())
            return DataPointKind::Exponential;

        throw std::out_of_range("Unknown data point kind");
    }

    inline void destroyCurrentIterator()
    {
        switch (DPKind) {
            case DataPointKind::Number:
            case DataPointKind::Sum:
                DPIt.NDPIt.~NumberDataPointIterator();
                DPEnd.NDPIt.~NumberDataPointIterator();
                break;
            case DataPointKind::Summary:
                DPIt.SDPIt.~SummaryDataPointIterator();
                DPEnd.SDPIt.~SummaryDataPointIterator();
                break;
            case DataPointKind::Histogram:
                DPIt.HDPIt.~HistogramDataPointIterator();
                DPEnd.HDPIt.~HistogramDataPointIterator();
                break;
            case DataPointKind::Exponential:
                DPIt.EHDPIt.~ExponentialHistogramDataPointIterator();
                DPEnd.EHDPIt.~ExponentialHistogramDataPointIterator();
                break;
            case DataPointKind::NotAvailable:
                break;
            default:
                throw std::out_of_range("Unknown data point kind");
        }
    }

    void initializeDataPointIterator(const pb::Metric &M)
    {
        DPKind = dpKind(M);

        switch (DPKind) {
            case DataPointKind::Number:
                DPIt.NDPIt = M.gauge().data_points().begin();
                DPEnd.NDPIt = M.gauge().data_points().end();
                if (DPIt.NDPIt == DPEnd.NDPIt) {
                    netdata_log_error(
                        "initializeDataPointIterator(): number - ndp it = ndp end (size: %d)\n>>>%s<<<",
                        M.gauge().data_points().size(),
                        M.DebugString().c_str());
                }
                break;
            case DataPointKind::Sum:
                DPIt.NDPIt = M.sum().data_points().begin();
                DPEnd.NDPIt = M.sum().data_points().end();
                if (DPIt.NDPIt == DPEnd.NDPIt)
                    netdata_log_error("initializeDataPointIterator(): sum - ndp it = ndp end");
                break;
            case DataPointKind::Summary:
                DPIt.SDPIt = M.summary().data_points().begin();
                DPEnd.SDPIt = M.summary().data_points().end();
                if (DPIt.SDPIt == DPEnd.SDPIt)
                    netdata_log_error("initializeDataPointIterator(): sdp it = sdp end");
                break;
            case DataPointKind::Histogram:
                DPIt.HDPIt = M.histogram().data_points().begin();
                DPEnd.HDPIt = M.histogram().data_points().end();
                if (DPIt.HDPIt == DPEnd.HDPIt)
                    netdata_log_error("initializeDataPointIterator(): hdp it = hdp end");
                break;
            case DataPointKind::Exponential:
                DPIt.EHDPIt = M.exponential_histogram().data_points().begin();
                DPEnd.EHDPIt = M.exponential_histogram().data_points().end();
                if (DPIt.EHDPIt == DPEnd.EHDPIt)
                    netdata_log_error("initializeDataPointIterator(): ehdp it = ehdp end");
                break;
            default:
                throw std::out_of_range("WTF?");
        }
    }

    void advance()
    {
        switch (DPKind) {
            case DataPointKind::Number:
            case DataPointKind::Sum: {
                if (++DPIt.NDPIt != DPEnd.NDPIt) {
                    netdata_log_error("advance(): ndp");
                    return;
                }
                break;
            }
            case DataPointKind::Summary: {
                if (++DPIt.SDPIt != DPEnd.SDPIt) {
                    netdata_log_error("advance(): sdp");
                    return;
                }
                break;
            }
            case DataPointKind::Histogram: {
                if (++DPIt.HDPIt != DPEnd.HDPIt) {
                    netdata_log_error("advance(): hdp");
                    return;
                }
                break;
            }
            case DataPointKind::Exponential: {
                if (++DPIt.EHDPIt != DPEnd.EHDPIt) {
                    netdata_log_error("advance(): ehdp");
                    return;
                }
                break;
            }
            case DataPointKind::NotAvailable:
                netdata_log_error("advance(): not available");
                break;
        }

        if (++MIt != MEnd) {
            netdata_log_error("advance(): m it");
            initializeDataPointIterator(*MIt);
            return;
        }

        if (++SMIt != SMEnd) {
            netdata_log_error("advance(): sm it");

            MIt = SMIt->metrics().begin();
            MEnd = SMIt->metrics().end();

            if (MIt != MEnd)
                initializeDataPointIterator(*MIt);

            return;
        }

        if (++RMIt != RMEnd) {
            netdata_log_error("advance(): rm it");

            SMIt = RMIt->scope_metrics().begin();
            SMEnd = RMIt->scope_metrics().end();

            if (SMIt != SMEnd) {
                MIt = SMIt->metrics().begin();
                MEnd = SMIt->metrics().end();

                if (MIt != MEnd) {
                    initializeDataPointIterator(*MIt);
                }
            }

            return;
        }

        if (++MDIt != MDEnd) {
            netdata_log_error("advance(): md it");

            RMIt = MDIt->resource_metrics().begin();
            RMEnd = MDIt->resource_metrics().end();

            if (RMIt != RMEnd) {
                SMIt = RMIt->scope_metrics().begin();
                SMEnd = RMIt->scope_metrics().end();

                if (SMIt != SMEnd) {
                    MIt = SMIt->metrics().begin();
                    MEnd = SMIt->metrics().end();

                    if (MIt != MEnd) {
                        initializeDataPointIterator(*MIt);
                    }
                }
            }

            return;
        }
    }
};

static std::vector<pb::MetricsData> *otelModMessages = nullptr;

class MessageReader {
public:
    bool processMessages(const uv_buf_t &Buf)
    {
        BM.fill(Buf);

        BM.getMessages(Messages);
        netdata_log_error("GVD OTEL received %zu messages", Messages.size());

        otelModMessages = &Messages;

        if (Messages.size()) {
            netdata_log_error("GVD OTEL - Dumping metric names");
            auto OtelIter = OtelIterator(Messages.begin(), Messages.end());

            while (OtelIter.hasNext()) {
                OtelElement OE = OtelIter.next();
                std::stringstream SS;

                netdata_log_error("Metric name: %s", OE.M->name().c_str());
                pb::printMetric(SS, *OE.M);

                netdata_log_error("printed metric: >>>%s<<<", SS.str().c_str());
            }
        }

        Messages.clear();
        return true;
    }

private:
    BufferManager<pb::MetricsData> BM;
    std::vector<pb::MetricsData> Messages;
};

static MessageReader MR;

static void on_read(uv_stream_t *stream, ssize_t nread, const uv_buf_t *buf)
{
    UNUSED(stream);

    if (nread > 0) {
        netdata_log_error("[OTEL] Received %zu bytes...", nread);
        const uv_buf_t data = {.base = buf->base, .len = (size_t)nread};
        MR.processMessages(data);
    } else if (nread < 0) {
        if (nread == UV_EOF) {
            netdata_log_error("[OTEL] Reached EOF...");
        } else {
            netdata_log_error("[OTEL] Read error: %s", uv_strerror(nread));
        }
    }

    if (buf->base)
        free(buf->base);
}

static otel_fifo_t create_fifo(otel_fifo_kind_t otel_fifo_kind)
{
    otel_fifo_t otel_fifo = {
        .kind = otel_fifo_kind,
        .path = nullptr,
        .fd = -1,
        .pipe = nullptr,
    };

    const char *fifo_kind = otel_fifo_kind_to_string(otel_fifo_kind);

    char key[128 + 1];
    snprintfz(key, 128, "fifo path for %s", fifo_kind);

    char value[FILENAME_MAX + 1];
    snprintfz(value, FILENAME_MAX, "%s/otel-%s.fifo", netdata_configured_cache_dir, fifo_kind);

    otel_fifo.path = config_get(CONFIG_SECTION_OTEL, key, value);

    // remove any leftover files
    unlink(otel_fifo.path);

    // create fifo
    errno = 0;
    if (mkfifo(otel_fifo.path, 0664) != 0) {
        netdata_log_error(
            "Could not create %s FIFO at %s: %s (errno=%d)", fifo_kind, otel_fifo.path, strerror(errno), errno);
        otel_fifo.path = nullptr;
        return otel_fifo;
    }

    // open for reading
    otel_fifo.fd = open(otel_fifo.path, O_RDONLY | O_NONBLOCK);
    if (otel_fifo.fd == -1) {
        netdata_log_error(
            "Could not open %s FIFO at %s: %s (errno=%d)", fifo_kind, otel_fifo.path, strerror(errno), errno);

        unlink(otel_fifo.path);
        otel_fifo.path = NULL;
        return otel_fifo;
    }

    /*
     * create a uv_pipe out of the FIFO fd
    */

    otel_fifo.pipe = reinterpret_cast<uv_pipe_t *>(callocz(1, sizeof(uv_pipe_t)));
    int err = uv_pipe_init(otel_state.loop, otel_fifo.pipe, 0);
    if (err) {
        netdata_log_error("uv_pipe_init(): %s", uv_strerror(err));
        goto LBL_PIPE_ERROR;
    }

    err = uv_pipe_open(otel_fifo.pipe, otel_fifo.fd);
    if (err) {
        netdata_log_error("uv_pipe_open(): %s", uv_strerror(err));
        goto LBL_PIPE_ERROR;
    }

    err = uv_read_start((uv_stream_t *)otel_fifo.pipe, alloc_buffer, on_read);
    if (err) {
        netdata_log_error("uv_read_start(): %s", uv_strerror(err));
        goto LBL_PIPE_ERROR;
    }

    switch (otel_fifo_kind) {
        case OTEL_FIFO_KIND_METRICS:
            otel_state.init_status |= InitStatus::HaveMetricsFifo;
            break;
        case OTEL_FIFO_KIND_LOGS:
            otel_state.init_status |= InitStatus::HaveLogsFifo;
            break;
        case OTEL_FIFO_KIND_TRACES:
            otel_state.init_status |= InitStatus::HaveTracesFifo;
            break;
    }

    return otel_fifo;

LBL_PIPE_ERROR:
    freez(otel_fifo.pipe);
    close(otel_fifo.fd);
    unlink(otel_fifo.path);

    return otel_fifo_t{
        .kind = otel_fifo_kind,
        .path = nullptr,
        .fd = -1,
        .pipe = nullptr,
    };
}

static void destroy_fifo(otel_fifo_t *otel_fifo)
{
    freez(otel_fifo->pipe);
    close(otel_fifo->fd);
    unlink(otel_fifo->path);

    memset(otel_fifo, 0, sizeof(otel_fifo_t));
}

static void spawn_otel_collector()
{
    uv_process_options_t options;
    memset(&options, 0, sizeof(uv_process_options_t));

    options.file = "/home/vk/.local/bin/otelcontribcol";

    char **args = new char *[4]{nullptr, nullptr, nullptr, nullptr};
    // args[0] = strdupz("otelcontribcol");
    args[0] = strdupz(options.file);
    args[1] = strdupz("--config");
    // args[2] = strdupz("/home/vk/.local/etc/netdata-otel.yml");
    args[2] = strdupz("/home/vk/repos/tmp/monitoring/setup/otel-collector/otelcol-config.yaml");
    args[3] = nullptr;

    options.args = args;
    options.exit_cb = [](uv_process_t *req, int64_t exit_status, int term_signal) {
        netdata_log_error("GVD OTEL collector exit_status: %lu, term_signal: %d", exit_status, term_signal);
        char **arg = (char **)req->data;
        while (*arg)
            freez(*arg++);

        free(req->data);

        completion_mark_complete(&otel_state.otel_process_completion);
    };

    otel_state.otel_process.data = (void *)args;

    {
        // Set up stdio containers
        uv_stdio_container_t stdio[3];

        // stdin
        stdio[0].flags = UV_IGNORE;

        // stdout
        stdio[1].flags = UV_IGNORE;

        // stderr
        stdio[2].flags = UV_IGNORE;

        int fd_stderr = netdata_logger_fd(NDLS_COLLECTORS);
        if (fd_stderr != -1) {
            stdio[2].flags = UV_INHERIT_FD;
            stdio[2].data.fd = fd_stderr;
        }

        options.stdio_count = 3;
        options.stdio = stdio;
    }

    int err = uv_spawn(otel_state.loop, &otel_state.otel_process, &options);
    if (err) {
        char **arg = (char **)args;
        while (*arg)
            freez(*arg++);
        delete[] args;
    } else {
        // For good measure...
        signals_restore_SIGCHLD();
        otel_state.init_status |= InitStatus::HaveSpawnedCollector;
    }
}

static void shutdown_libuv_handles(uv_async_t *handle)
{
    UNUSED(handle);

    // FIXME: the process shutdowns but the exit callback does not run
    // This times out the completion and we end up killing the process.
    // However the process will not get reaped by the agent.
    if (otel_state.haveRunLoop()) {
        uv_process_kill(&otel_state.otel_process, SIGTERM);

        bool ok = completion_timedwait_for(&otel_state.otel_process_completion, 3);
        if (!ok)
            uv_process_kill(&otel_state.otel_process, SIGKILL);
    }

    if (otel_state.haveMetricsFifo())
        uv_close((uv_handle_t *)otel_state.metrics_fifo.pipe, NULL);
    if (otel_state.haveLogsFifo())
        uv_close((uv_handle_t *)otel_state.logs_fifo.pipe, NULL);
    if (otel_state.haveTracesFifo())
        uv_close((uv_handle_t *)otel_state.traces_fifo.pipe, NULL);

    if (otel_state.haveAsync())
        uv_close((uv_handle_t *)&otel_state.async, NULL);

    if (otel_state.haveSpawnedCollector())
        uv_close((uv_handle_t *)&otel_state.otel_process, NULL);

    if (otel_state.haveRunLoop())
        uv_stop(otel_state.loop);
}

extern "C" void otel_init(void)
{
    memset(&otel_state, 0, sizeof(otel_state_t));

    otel_state.loop = reinterpret_cast<uv_loop_t *>(callocz(1, sizeof(uv_loop_t)));
    int err = uv_loop_init(otel_state.loop);
    if (err) {
        freez(otel_state.loop);
        netdata_log_error("Failed to initialize libuv loop: %s", uv_strerror(err));
        return;
    }
    otel_state.init_status |= InitStatus::HaveLoop;

    err = uv_async_init(otel_state.loop, &otel_state.async, shutdown_libuv_handles);
    if (err) {
        netdata_log_error("Failed to initialize async handle: %s", uv_strerror(err));
        return;
    }
    otel_state.init_status |= InitStatus::HaveAsync;

    completion_init(&otel_state.shutdown_completion);
    completion_init(&otel_state.otel_process_completion);
    otel_state.init_status |= InitStatus::HaveCompletion;

    otel_state.metrics_fifo = create_fifo(OTEL_FIFO_KIND_METRICS);
    otel_state.logs_fifo = create_fifo(OTEL_FIFO_KIND_LOGS);
    otel_state.traces_fifo = create_fifo(OTEL_FIFO_KIND_TRACES);

    if (!otel_state.haveAllFifos()) {
        netdata_log_error("Could not create required FIFOs to collect OTEL data");
        return;
    }

    spawn_otel_collector();
}

extern "C" void otel_shutdown(void)
{
    if (otel_state.haveRunLoop()) {
        uv_async_send(&otel_state.async);
        completion_wait_for(&otel_state.shutdown_completion);
        completion_destroy(&otel_state.shutdown_completion);
    }

    if (otel_state.haveMetricsFifo())
        destroy_fifo(&otel_state.metrics_fifo);
    if (otel_state.haveLogsFifo())
        destroy_fifo(&otel_state.logs_fifo);
    if (otel_state.haveTracesFifo())
        destroy_fifo(&otel_state.traces_fifo);

    if (otel_state.haveLoop())
        freez(otel_state.loop);
}

static void otel_main_cleanup(void *data)
{
    struct netdata_static_thread *static_thread = (struct netdata_static_thread *)data;
    static_thread->enabled = NETDATA_MAIN_THREAD_EXITING;

    // Nothing to do here everything's cleaned up during otel_shutdown()

    static_thread->enabled = NETDATA_MAIN_THREAD_EXITED;
}

static void loadAttributesFromYaml(const std::string &filename)
{
    std::stringstream SS;

    YAML::Node config = YAML::LoadFile(filename);
    if (!config["resource_attributes"]) {
        SS << absl::NotFoundError("The key 'resource_attributes' does not exist in the YAML file.");
        netdata_log_error("Failed to load file: %s", SS.str().c_str());
    }

    auto ResAttrs = ResourceAttributes::get(config);
    if (!ResAttrs.ok()) {
        SS << ResAttrs.status();
        netdata_log_error("Failed to load resource attributes: %s", SS.str().c_str());
        return;
    }

    ResAttrs->printAttributes(SS);
    netdata_log_error("Resource attributes loaded from %s:\n%s", filename.c_str(), SS.str().c_str());
}

extern "C" void *otel_main(void *ptr)
{
    netdata_thread_cleanup_push(otel_main_cleanup, ptr);

    const std::string Path =
        "/home/vk/repos/otel/opentelemetry-collector-contrib/receiver/elasticsearchreceiver/metadata.yaml";
    loadAttributesFromYaml(Path);
    // if (otel_state.haveSpawnedCollector()) {
    //     otel_state.init_status |= InitStatus::HaveRunLoop;
    //     uv_run(otel_state.loop, UV_RUN_DEFAULT);
    //     completion_mark_complete(&otel_state.shutdown_completion);

    //     int ret = uv_loop_close(otel_state.loop);
    //     if (ret == UV_EBUSY)
    //         fatal("GVD P[OTEL] libuv loop closed with EBUSY");
    // }

    netdata_thread_cleanup_pop(1);
    return nullptr;
}
