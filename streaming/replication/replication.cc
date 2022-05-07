#include "replication-private.h"
#include <stack>
#include <queue>

using namespace replication;

class Host {
public:
    Host(RRDHOST *RH) : RH(RH) {
        size_t Len = 8192;
        char Buf[Len];
        memset(Buf, 0, Len);
        replication_load_gaps(&RH->host_uuid, Buf, Len);

        std::vector<TimeRange> TRs;
        if (!deserializeTimeRanges(TRs, Buf, Len)) {
            error("GVD[%s]: failed to load receiver gaps", RH->hostname);
            return;
        }

        error("GVD[%s]: num loaded gaps %zu", RH->hostname, TRs.size());
        for (const auto &TR : TRs)
            error("GVD[%s]: deserialized gap <%ld, %ld>", RH->hostname, TR.first, TR.second);

        {
            std::lock_guard<Mutex> L(ReceiverMutex);
            ReceiverGaps = TRs;
        }
    }

    ~Host() {
        size_t Len = 8192;
        char Buf[Len];
        memset(Buf, 0, Len);

        std::vector<TimeRange> TRs;
        {
            std::lock_guard<Mutex> L(ReceiverMutex);
            TRs = ReceiverGaps;
        }

        if (!serializeTimeRanges(TRs, Buf, Len)) {
            error("GVD[%s]: failed to serialize receiver gaps", RH->hostname);
            return;
        }

        replication_save_gaps(&RH->host_uuid, Buf, Len);
    }

    void addDimension(RRDDIM *RD) {
        std::lock_guard<Mutex> L(DimsMapMutex);
        DimensionsMap[RD] = false;
    };

    void removeDimension(RRDDIM *RD) {
        std::lock_guard<Mutex> L(DimsMapMutex);
        DimensionsMap.erase(RD);
    };

    GapData getGapData(RRDDIM *RD, const TimeRange &TR) {
        GapData GD;

        rrdset_rdlock(RD->rrdset);
        GD.setChart(RD->rrdset->id);
        GD.setDimension(RD->id);
        GD.setStorageNumbers(Query::getSNs(RD, TR.first, TR.second));
        rrdset_unlock(RD->rrdset);

        return GD;
    }

    time_t firstEntry() const {
        return rrdhost_first_entry_t(RH);
    }

    time_t lastEntry() const {
        return rrdhost_last_entry_t(RH);
    }

    std::vector<TimeRange> getReceiverGaps() {
        std::lock_guard<Mutex> L(ReceiverMutex);
        return ReceiverGaps;
    }

    std::vector<TimeRange> getSenderGaps() {
        std::lock_guard<Mutex> L(SenderMutex);
        return SenderGaps;
    }

    void setSenderGaps(std::vector<TimeRange> &TRs) {
        std::lock_guard<Mutex> L(SenderMutex);
        SenderGaps = TRs;
    }

    void receiverConnect() {
        std::lock_guard<Mutex> L(ReceiverMutex);

        time_t LastEntry = lastEntry();
        time_t Timestamp = now_realtime_sec();

        if (LastEntry == 0) {
            time_t SavedAfter = 0, SavedBefore = 0;
            replication_load_host_entries_range(&RH->host_uuid, &SavedAfter, &SavedBefore);
            LastEntry = (SavedBefore != 0) ? (SavedBefore + 1) : Timestamp - 600; // TODO: make this configurable.
        }

        if (Timestamp <= LastEntry) {
            error("GVD[%s]: Skipping invalid time range on connect: <%ld, %ld>", RH->hostname, LastEntry, Timestamp);
            return;
        }

        ReceiverGaps.emplace_back(LastEntry, Timestamp);

        error("GVD[%s]: num gaps before coalescing %zu", RH->hostname, ReceiverGaps.size());
        for (const auto &TR : ReceiverGaps)
            error("GVD[%s]: gap <%ld, %ld>", RH->hostname, TR.first, TR.second);

        ReceiverGaps = coalesceTimeRanges(ReceiverGaps);

        error("GVD[%s]: num gaps after coalescing %zu", RH->hostname, ReceiverGaps.size());
        for (const auto &TR : ReceiverGaps)
            error("GVD[%s]: gap <%ld, %ld>", RH->hostname, TR.first, TR.second);
    }

    void receiverDropGap(const TimeRange &TR) {
        std::lock_guard<Mutex> L(ReceiverMutex);
        error("GVD: dropping gap <%ld, %ld>", TR.first, TR.second);
        ReceiverGaps.erase(std::remove(ReceiverGaps.begin(), ReceiverGaps.end(), TR), ReceiverGaps.end());
    }

    enum class FillOneResult {
        NoAvailableGaps,
        NoAvailableDims,
        OkGapFilled,
        OkDimVanished,
    };

    enum FillOneResult fillOne() {
        // get one gap
        TimeRange Gap;
        {
            std::lock_guard<Mutex> L(SenderMutex);
            if (SenderGaps.size() == 0)
                return FillOneResult::NoAvailableGaps;
            Gap = SenderGaps.back();
        }
        error("GVD[%s@fillOne]: active gap <%ld, %ld>", RH->hostname, Gap.first, Gap.second);

        GapData GD;
        RRDDIM *RD = nullptr;

        // find one dimension to fill for this gap
        rrdhost_disable_obsoletion(RH);
        {
            std::lock_guard<Mutex> L(DimsMapMutex);
            for (const auto &P : DimensionsMap) {
                if (!P.second) {
                    RD = P.first;
                    error("GVD[%s@fillOne]: active dim %s.%s", RH->hostname, RD->rrdset->id, RD->id);
                    break;
                }
            }

            if (RD)
                GD = getGapData(RD, Gap);
        }
        rrdhost_enable_obsoletion(RH);

        // no more dims for this gap
        if (!RD) {
            {
                std::lock_guard<Mutex> L(SenderMutex);

                error("GVD[%s@fillOne]: removing gap <%ld, %ld>", RH->hostname, Gap.first, Gap.second);
                SenderGaps.pop_back();
            }

            sender_start(RH->sender);
            buffer_sprintf(RH->sender->build, "DROPGAP \"%ld\" \"%ld\"\n", Gap.first, Gap.second);
            sender_commit(RH->sender);

            return FillOneResult::NoAvailableDims;
        }

        // wait until we can append data to the circular buffer
        while (!GD.push(RH->sender)) {
            error("GVD[%s@fillOne] Can not append <%ld, %ld> for dim %s.%s in circular buffer", RH->hostname, Gap.first, Gap.second, GD.getChart().c_str(), GD.getDimension().c_str());
            std::this_thread::sleep_for(std::chrono::seconds(1));
        }

        // mark the dim as sent for this gap
        {
            std::lock_guard<Mutex> L(DimsMapMutex);
            for (auto &P : DimensionsMap) {
                if (P.first == RD) {
                    error("GVD[%s@fillOne]: gap <%ld, %ld> filled with dimension %s.%s", RH->hostname, Gap.first, Gap.second, GD.getChart().c_str(), GD.getDimension().c_str());
                    P.second = true;
                    return FillOneResult::OkGapFilled;
                }
            }
        }

        error("GVD[%s@fillOne]: dim %s.%s vanished while filling gap <%ld, %ld>", RH->hostname, GD.getChart().c_str(), GD.getDimension().c_str(), Gap.first, Gap.second);
        return FillOneResult::OkDimVanished;
    }

    void senderReplicateGaps() {
        std::this_thread::sleep_for(std::chrono::seconds(4));

        while (!netdata_exit) {
            std::this_thread::sleep_for(std::chrono::seconds(1));

            switch (fillOne()) {
            case FillOneResult::NoAvailableGaps:
                error("GVD[%s]: no available gaps; sleeping for 1 sec", RH->hostname);
                std::this_thread::sleep_for(std::chrono::seconds(1));
                break;
            case FillOneResult::NoAvailableDims: {
                std::lock_guard<Mutex> L(DimsMapMutex);
                error("GVD[%s]: no available dims (resetting dims)", RH->hostname);
                for (auto &P : DimensionsMap) {
                    DimensionsMap[P.first] = false;
                }
                break;
            }
            case FillOneResult::OkGapFilled:
                error("GVD[%s]: ok gap filled", RH->hostname);
                break;
            case FillOneResult::OkDimVanished:
                error("GVD[%s]: ok gap vanished", RH->hostname);
                break;
            }
        }
    }

    void startReplicationThread() {
        error("GVD[%s]: Starting replication thread", RH->hostname);
        ReplicationThread = std::thread(&Host::senderReplicateGaps, this);
    }

    void stopReplicationThread() {
        error("GVD[%s]: Cancelling replication thread", RH->hostname);
        netdata_thread_cancel(ReplicationThread.native_handle());
        error("GVD[%s]: Joining replication thread", RH->hostname);
        ReplicationThread.join();
        error("GVD[%s]: Stopped replication thread", RH->hostname);
    }

private:
    RRDHOST *RH;

    Mutex DimsMapMutex;
    std::map<RRDDIM *, bool> DimensionsMap;

    Mutex ReceiverMutex;
    std::vector<TimeRange> ReceiverGaps;

    Mutex SenderMutex;
    std::vector<TimeRange> SenderGaps;

    std::thread ReplicationThread;
};


/*
 * C API
 */

void replication_init(void) {
    Cfg.readReplicationConfig();
}

void replication_fini(void) {
}

void replication_new_host(RRDHOST *RH) {
    if (!Cfg.EnableReplication)
        return;

    Host *H = new Host(RH);
    RH->repl_handle = static_cast<replication_handle_t>(H);
}

void replication_delete_host(RRDHOST *RH) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    delete H;
    RH->repl_handle = nullptr;
}

void replication_new_dimension(RRDHOST *RH, RRDDIM *RD) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    H->addDimension(RD);
}

void replication_delete_dimension(RRDHOST *RH, RRDDIM *RD) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    H->removeDimension(RD);
}

void replication_connected(RRDHOST *RH) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    H->receiverConnect();
}

void replication_disconnected(RRDHOST *RH) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;
}

bool replication_receiver_serialize_gaps(RRDHOST *RH, char *Buf, size_t Len) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return false;

    std::vector<TimeRange> TRs = H->getReceiverGaps();
    return serializeTimeRanges(TRs, Buf, Len);
}

bool replication_receiver_fill_gap(RRDHOST *RH, const char *Buf) {
    GapData GD = GapData::fromBase64(Buf);
    return GD.flushToDBEngine(RH);
}

void replication_receiver_drop_gap(RRDHOST *RH, time_t After, time_t Before) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    H->receiverDropGap({ After, Before });
}

bool replication_sender_deserialize_gaps(RRDHOST *RH, const char *Buf, size_t Len) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return false;

    std::vector<TimeRange> TRs;
    if (!deserializeTimeRanges(TRs, Buf, Len)) {
        error("GVD[%s]: sender failed to deserialize gaps", RH->hostname);
        return false;
    }

    error("GVD[%s]: num deserialized gaps %zu", RH->hostname, TRs.size());
    for (const auto &TR : TRs)
        error("GVD[%s]: deserialized gap <%ld, %ld>", RH->hostname, TR.first, TR.second);

    H->setSenderGaps(TRs);
    return true;
}

void replication_thread_start(RRDHOST *RH) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    H->startReplicationThread();
}

void replication_thread_stop(RRDHOST *RH) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    H->stopReplicationThread();
}
