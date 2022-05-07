#include "replication-private.h"

#include <iomanip>
#include <ctime>

using namespace replication;

void GapData::print() const {
    std::stringstream SS;

    SS << "GVD: GapData (Chart=" << Chart << ", Dimension=" << Dimension << ", Entries=" << StorageNumbers.size() << ")\n";
    for (const auto &P : StorageNumbers) {
        auto tm = *std::localtime(&P.first);
        SS << std::put_time(&tm, "%Y-%m-%dT%H:%M:%S.%z%Z") << " SN: " << P.second << " CN: " << unpack_storage_number(P.second) << std::endl;
    }
    error("%s", SS.str().c_str());
}

bool GapData::push(struct sender_state *Sender) const {
    netdata_mutex_lock(&Sender->mutex);
    double MaxBufferCapacity = Sender->buffer->max_size;
    double RemainingBufferCapacity = cbuffer_remaining_capacity(Sender->buffer, false);
    double RemainingRatio = RemainingBufferCapacity / MaxBufferCapacity;
    netdata_mutex_unlock(&Sender->mutex);

    // Close enough but not 100% correct because we release the lock
    if (RemainingRatio < 0.25)
        return false;

    sender_start(Sender);
    buffer_sprintf(Sender->build, "FILLGAP \"%s\"\n", toBase64().c_str());
    sender_commit(Sender);

    print();
    return true;
}

pb::GapData GapData::toProto() const {
    pb::GapData PGD;

    PGD.set_chart(Chart);
    PGD.set_dimension(Dimension);

    for (size_t Idx = 0; Idx != StorageNumbers.size(); Idx++) {
        PGD.mutable_timestamps()->Add(StorageNumbers[Idx].first);
        PGD.mutable_values()->Add(StorageNumbers[Idx].second);
    }

    return PGD;
}

GapData GapData::fromProto(const pb::GapData &PGD) {
    GapData GD;

    GD.setChart(PGD.chart());
    GD.setDimension(PGD.dimension());

    if (PGD.timestamps_size() != PGD.values_size())
        fatal("GVD: Protobuf message has different number of timestamps vs. values (%d != %d)",
              PGD.timestamps_size(), PGD.values_size());

    for (int Idx = 0; Idx != PGD.timestamps_size(); Idx++)
        GD.StorageNumbers.emplace_back(PGD.timestamps(Idx), PGD.values(Idx));

    return GD;
}

std::string GapData::toBase64() const {
    pb::GapData PGD = toProto();
    std::string PBS = PGD.SerializeAsString();
    return base64_encode(PBS);
}

GapData GapData::fromBase64(const std::string &EncodedData) {
    pb::GapData PGD;

    std::string DecodedData = base64_decode(EncodedData);
    if (!PGD.ParseFromString(DecodedData))
        fatal("GVD: Could not decode protobuf message");

    return fromProto(PGD);
}

bool GapData::flushToDBEngine(RRDHOST *RH) const {
    RRDDIM_PAST_DATA *DPD;

    print();

    DPD = replication_collect_past_metric_init(RH, Chart.c_str(), Dimension.c_str());
    if (!DPD)
        return false;

    for (const auto &P : StorageNumbers)
        replication_collect_past_metric(DPD, P.first, P.second);

    replication_collect_past_metric_done(DPD);
    return true;
}
