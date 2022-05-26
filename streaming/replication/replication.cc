#include "replication-private.h"
#include <stack>
#include <queue>

using namespace replication;

class Host {
public:
    Host(RRDHOST *RH) : RH(RH) {}

    void setReceiverGaps(std::vector<TimeRange> &TRs) {
        std::lock_guard<Mutex> L(ReceiverMutex);
        ReceiverGaps = TRs;
    }

    std::vector<TimeRange> getReceiverGaps() {
        std::lock_guard<Mutex> L(ReceiverMutex);
        return ReceiverGaps;
    }

    void setSenderGaps(std::vector<TimeRange> &TRs) {
        std::lock_guard<Mutex> L(SenderMutex);
        SenderGaps = TRs;
    }

    std::vector<TimeRange> getSenderGaps() {
        std::lock_guard<Mutex> L(SenderMutex);
        return SenderGaps;
    }

    void receiverConnect() {
        std::lock_guard<Mutex> L(ReceiverMutex);

        time_t LastEntry = rrdhost_last_entry_t(RH);
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
        ReceiverGaps = coalesceTimeRanges(ReceiverGaps);
    }

    void receiverDropGap(const TimeRange &TR) {
        std::lock_guard<Mutex> L(ReceiverMutex);
        error("GVD: dropping gap <%ld, %ld>", TR.first, TR.second);
        ReceiverGaps.erase(std::remove(ReceiverGaps.begin(), ReceiverGaps.end(), TR), ReceiverGaps.end());
    }

    void senderReplicateGaps() {
        while (!netdata_exit) {
            error("Hello from replication thread");
            std::this_thread::sleep_for(std::chrono::seconds(1));
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

    /*
     * Load receiver gaps from sqlite db
    */
    size_t Len = 8192;
    char Buf[Len];
    memset(Buf, 0, Len);
    replication_load_gaps(&RH->host_uuid, Buf, Len);
    std::vector<TimeRange> TRs = deserializeTimeRanges(Buf, Len);

    /*
     * Create host
    */
    Host *H = new Host(RH);
    H->setReceiverGaps(TRs);
    RH->repl_handle = static_cast<replication_handle_t>(H);
}

void replication_delete_host(RRDHOST *RH) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    /*
     * Save receiver gaps to sqlite DB
     */
    size_t Len = 8192;
    char Buf[Len];
    memset(Buf, 0, Len);

    std::vector<TimeRange> TRs = H->getReceiverGaps();
    serializeTimeRanges(TRs, Buf, Len);
    replication_save_gaps(&RH->host_uuid, Buf, Len);

    /*
     * Delete host
     */
    delete H;
    RH->repl_handle = nullptr;
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

void replication_receiver_connect(RRDHOST *RH, char *Buf, size_t Len) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    H->receiverConnect();
    std::vector<TimeRange> TRs = H->getReceiverGaps();
    serializeTimeRanges(TRs, Buf, Len);
}

void replication_sender_connect(RRDHOST *RH, const char *Buf, size_t Len) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    std::vector<TimeRange> TRs = deserializeTimeRanges(Buf, Len);
    H->setSenderGaps(TRs);
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
