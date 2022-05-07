#include "daemon/common.h"
#include "replication.pb.h"
#include <sstream>
#include <iomanip>

#if GOOGLE_PROTOBUF_VERSION < 3001000
#define PROTO_COMPAT_MSG_SIZE(msg) (size_t)msg.ByteSize()
#define PROTO_COMPAT_MSG_SIZE_PTR(msg) (size_t)msg->ByteSize()
#else
#define PROTO_COMPAT_MSG_SIZE(msg) msg.ByteSizeLong()
#define PROTO_COMPAT_MSG_SIZE_PTR(msg) msg->ByteSizeLong()
#endif

#include <random>
#include "base64.h"

using namespace replication;

static void printBuf(const char *Buf, size_t N) {
    std::stringstream SS;

    SS << std::hex << std::setfill('0');
    for (size_t Idx = 0; Idx != N; Idx++)
        SS << std::setw(2) << static_cast<unsigned>(Buf[Idx]) << " ";

    error("hexdump: %s", SS.str().c_str());
}

class GapData {
public:
    static GapData getRandom(std::string Chart, std::string Dimension, size_t NumEntries) {
        std::random_device RD;
        std::mt19937 Gen(RD());

        std::vector<time_t> Timestamps;
        std::vector<storage_number> StorageNumbers;

        for (size_t Idx = 0; Idx != NumEntries; Idx++) {
            Timestamps.push_back(Gen());
            StorageNumbers.push_back(Gen());
        }

        GapData GD;

        GD.setChart(Chart);
        GD.setDimension(Dimension);
        GD.setTimestamps(Timestamps);
        GD.setStorageNumbers(StorageNumbers);

        return GD;
    }

public:
    void setChart(std::string Name) {
        Chart = Name;
    }

    std::string getChart() const {
        return Chart;
    }

    void setDimension(std::string Name) {
        Dimension = Name;
    }

    std::string getDimension() const {
        return Dimension;
    }

    void setTimestamps(std::vector<time_t> V) {
        Timestamps = V;
    }

    std::vector<time_t> getTimestamps() const {
        return Timestamps;
    }

    void setStorageNumbers(std::vector<storage_number> V) {
        StorageNumbers = V;
    }

    std::vector<storage_number> getStorageNumbers() const {
        return StorageNumbers;
    }

    std::string toBase64() {
        pb::GapData PGD = toProto();
        std::string PBS = PGD.SerializeAsString();
        return base64_encode(PBS);
    }

    static GapData fromBase64(const std::string &EncodedData) {
        pb::GapData PGD;

        std::string DecodedData = base64_decode(EncodedData);
        if (!PGD.ParseFromString(DecodedData))
            fatal("Could not decode msg");

        return fromProto(PGD);
    }

private:
    pb::GapData toProto() const {
        pb::GapData PGD;

        PGD.set_chart(Chart);
        PGD.set_dimension(Dimension);

        for (size_t Idx = 0; Idx != Timestamps.size(); Idx++) {
            PGD.mutable_timestamps()->Add(Timestamps[Idx]);
            PGD.mutable_values()->Add(StorageNumbers[Idx]);
        }

        return PGD;
    }

    static GapData fromProto(const pb::GapData &PGD) {
        GapData GD;

        GD.setChart(PGD.chart());
        GD.setDimension(PGD.dimension());

        std::vector<time_t> Timestamps;
        std::vector<storage_number> StorageNumbers;

        Timestamps.reserve(PGD.timestamps_size());
        StorageNumbers.reserve(PGD.values_size());

        for (int Idx = 0; Idx != PGD.timestamps_size(); Idx++) {
            Timestamps.push_back(PGD.timestamps(Idx));
            StorageNumbers.push_back(PGD.values(Idx));
        }

        GD.setTimestamps(Timestamps);
        GD.setStorageNumbers(StorageNumbers);

        return GD;
    }

private:
    std::string Chart;
    std::string Dimension;
    std::vector<time_t> Timestamps;
    std::vector<storage_number> StorageNumbers;
};

void encode_gap_data(BUFFER *build) {
    GapData GD = GapData::getRandom("MyChart", "MyDimension", 2);
    std::string EncodedData = GD.toBase64();
    buffer_sprintf(build, "FILLGAP \"%s\"\n", EncodedData.c_str());
}

void decode_gap_data(char *Base64Buf) {
    if (!Base64Buf)
        fatal("GVD: Could not decode gap data from base64 buf because it's NULL");

    std::string EncodedData(Base64Buf);
    GapData GD = GapData::fromBase64(EncodedData);

    /* Print gap data */
    std::stringstream SS;
    SS << "Gap data for " << GD.getChart() << "." << GD.getDimension() << "\n";
    std::vector<time_t> Timestamps = GD.getTimestamps();
    std::vector<storage_number> StorageNumbers = GD.getStorageNumbers();
    for (size_t Idx = 0; Idx != Timestamps.size(); Idx++)
        SS << Timestamps[Idx] << ": " << StorageNumbers[Idx] << "\n";
    error("GVD: %s", SS.str().c_str());
}
