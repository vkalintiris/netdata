#ifndef NETDATA_OTEL_INGESTOR_HPP
#define NETDATA_OTEL_INGESTOR_HPP

#include "otel_config.hpp"
#include "otel_utils.hpp"

#include "database/rrd.h"

#include <absl/status/statusor.h>
#include <absl/types/optional.h>
#include <absl/strings/str_cat.h>
#include <fstream>

namespace otel
{
template <typename T> class BufferManager {
public:
    void fill(const uv_buf_t &Buf);

    uint32_t messageLength() const;

    absl::StatusOr<T> readMessage(uint32_t MessageLength);

private:
    inline size_t remainingBytes() const
    {
        return Data.size() - Pos;
    }

    inline bool haveAtLeastXBytes(uint32_t Bytes) const
    {
        return remainingBytes() >= Bytes;
    }

private:
    std::vector<char> Data;
    size_t Pos = {0};
};

class Otel {
public:
    static Otel *get(const std::string &Path)
    {
        otel::Config *Cfg = new otel::Config(Path);
        return new Otel(Cfg);
    }

    bool processMessages(const uv_buf_t &Buf)
    {
        BM.fill(Buf);

        uint32_t MessageLength = BM.messageLength();
        if (MessageLength == 0)
            return true;

        auto MD = BM.readMessage(MessageLength);
        if (!MD.ok())
            return true;

        return true;
    }

private:
    template<typename T>
    void dump(const std::string &Path, const T &PB)
    {
        std::ofstream OS(Path, std::ios_base::app);
        if (OS.is_open()) {
            OS << PB.Utf8DebugString() << std::endl;
            OS.close();
        } else {
            std::cerr << "Unable to open /tmp/foo.txt for appending" << std::endl;
        }
    }

private:
    Otel(const otel::Config *Cfg) : Cfg(Cfg)
    {
    }

private:
    BufferManager<pb::MetricsData> BM;
    const otel::Config *Cfg;
};

} // namespace otel

#endif /* NETDATA_OTEL_INGESTOR_HPP */
