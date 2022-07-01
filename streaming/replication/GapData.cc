#include "replication-private.h"

#include <iomanip>
#include <ctime>

using namespace replication;


void GapData::print(RRDHOST *RH) const {
    std::stringstream SS;

    SS << "[" << RH->hostname << "] ";
    SS << "GapData (Chart=" << Chart << ", Dimension=" << Dimension << ", Entries=" << StorageNumbers.size() << ")\n";
    for (const auto &P : StorageNumbers)
        SS << "Timestamp: " << P.first << " SN: " << P.second << " CN: " << unpack_storage_number(P.second) << std::endl;
    error("%s", SS.str().c_str());
}

bool GapData::push(struct sender_state *Sender) const {
    /*
     * Parent's dbengine functions will cause a crash if we send
     * a GapData with 0 entries.
     */
    if (StorageNumbers.size() == 0)
        return true;

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

    if (PGD.timestamps_size() != PGD.values_size()) {
        error("Protobuf message has different number of timestamps vs. values (%d != %d)",
              PGD.timestamps_size(), PGD.values_size());
        return GD;
    }

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
        error("Could not decode protobuf message for GapData");

    return fromProto(PGD);
}

std::vector<TimeRange> GapData::getTimeRanges() const {
    std::vector<TimeRange> TRs;

    if (StorageNumbers.size() == 0)
        return TRs;

    TimeRange TR(StorageNumbers[0].first, StorageNumbers[0].first);
    time_t RangeStep = 0;

    for (size_t Idx = 1; Idx != StorageNumbers.size(); Idx++) {
        time_t ThisStep = StorageNumbers[Idx].first - TR.second;

        if (RangeStep == 0)
            RangeStep = ThisStep;

        if (RangeStep == ThisStep) {
            TR.second = StorageNumbers[Idx].first;
            continue;
        }

        TRs.push_back(TR);
        TR = { StorageNumbers[Idx].first, StorageNumbers[Idx].first };
        RangeStep = 0;
    }

    if (TRs.size() == 0 || TRs.back() != TR)
        TRs.push_back(TR);

    return TRs;
}

#ifdef ENABLE_DBENGINE
bool GapData::flushToDBEngine(RRDHOST *RH) const {
    if (StorageNumbers.size() == 0) {
        error("[%s] No storage numbers to flush to DBEngine for %s.%s",
              RH->hostname, Chart.c_str(), Dimension.c_str());
        return false;
    }

    if (RH->rrd_memory_mode != RRD_MEMORY_MODE_DBENGINE) {
        error("[%s] host memory mode is not dbengine (dropping gap data for %s.%s.",
              RH->hostname, Chart.c_str(), Dimension.c_str());
        return false;
    }

    constexpr time_t MaxEntriesPerPage = RRDENG_BLOCK_SIZE / sizeof(storage_number);
    storage_number Page[MaxEntriesPerPage] = { 0 };

    RRDDIM_PAST_DATA DPD;
    memset(&DPD, 0, sizeof(DPD));

    DPD.host = RH;
    DPD.page = Page;

    rrdhost_rdlock(RH);
    DPD.st = rrdset_find(RH, Chart.c_str());
    if (!DPD.st) {
        error("[%s] Can not find chart %s", RH->hostname, Chart.c_str());
        rrdhost_unlock(RH);
        return false;
    }

    if (DPD.st->rrd_memory_mode != RRD_MEMORY_MODE_DBENGINE) {
        error("[%s] Can not fill gap data because chart %s is not using dbengine", RH->hostname, Chart.c_str());
        rrdhost_unlock(RH);
        return false;
    }

    rrdset_rdlock(DPD.st);
    DPD.rd = rrddim_find(DPD.st, Dimension.c_str());
    if (!DPD.rd) {
        error("[%s] Can not find dimension %s.%s", RH->hostname, Chart.c_str(), Dimension.c_str());
        rrdset_unlock(DPD.st);
        rrdhost_unlock(RH);
        return false;
    }

    std::vector<TimeRange> TRs = getTimeRanges();
    for (const auto &TR : TRs) {
        size_t PageIdx = 0;
        for (const auto &P : StorageNumbers) {
            if ((P.first < TR.first) || (P.first > TR.second))
                continue;

            Page[PageIdx++] = P.second;
        }

        DPD.start_time = TR.first * USEC_PER_SEC;
        DPD.end_time = TR.second * USEC_PER_SEC;
        DPD.page_length = PageIdx * sizeof(storage_number);

        if (rrdeng_store_past_metrics_realtime(DPD.rd, &DPD)) {
            if (rrdeng_store_past_metrics_page_init(&DPD)) {
                fatal("Cannot initialize db engine page: Flushing collected past data skipped!");
                rrdset_unlock(DPD.st);
                rrdhost_unlock(RH);
                return false;
            }

            rrdeng_store_past_metrics_page(&DPD);
            rrdeng_flush_past_metrics_page(&DPD);
            rrdeng_store_past_metrics_page_finalize(&DPD);
            debug(D_REPLICATION, "[%s] Flushed gap data for %s.%s", RH->hostname, Chart.c_str(), Dimension.c_str());
        }
    }

    rrdset_unlock(DPD.st);
    rrdhost_unlock(RH);
    return true;
}
#else
bool GapData::flushToDBEngine(RRDHOST *RH) const {
    error("[%s] Can not fill gap data for %s.%s because the agent does not support DBEngine",
          RH->hostname, Chart.c_str(), Dimension.c_str());
    return false;
}
#endif
