#include "replication-private.h"

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

    /* adds a new gap */
    void receiverConnect() {
        std::lock_guard<Mutex> L(ReceiverMutex);

        time_t LastEntry = rrdhost_last_entry_t(RH);
        time_t CurrTime = now_realtime_sec();

        if (LastEntry == 0) {
            time_t SavedAfter = 0, SavedBefore = 0;
            replication_load_host_entries_range(&RH->host_uuid, &SavedAfter, &SavedBefore);
            LastEntry = (SavedBefore != 0) ? (SavedBefore + 1) :
                                             (CurrTime - Cfg.SecondsToReplicateOnFirstConnection + 1);
        }

        if (CurrTime <= LastEntry) {
            error("GVD[%s]: Skipping invalid time range on connect: <%ld, %ld>", RH->hostname, LastEntry, CurrTime);
            return;
        }

        TimeRange TR(LastEntry, CurrTime);
        std::vector<TimeRange> NewTRs = splitTimeRange(TR, Cfg.MaxEntriesPerGapData);
        for (const TimeRange &NewTR : NewTRs)
            ReceiverGaps.push_back(NewTR);

        ReceiverGaps = coalesceTimeRanges(ReceiverGaps);
    }

    /* drops a received gap */
    void receiverDropGap(const TimeRange &TR) {
        std::lock_guard<Mutex> L(ReceiverMutex);
        error("GVD: dropping gap <%ld, %ld>", TR.first, TR.second);
        ReceiverGaps.erase(std::remove(ReceiverGaps.begin(), ReceiverGaps.end(), TR), ReceiverGaps.end());
    }

    /* replicate gaps */
    void senderReplicateGaps() {
        while (!netdata_exit) {
            /*
             * Sleep while we don't have any gaps to fill.
             */

            size_t NumGaps = 0;
            while (NumGaps == 0) {
                {
                    std::lock_guard<Mutex> L(SenderMutex);
                    NumGaps = SenderGaps.size();
                }

                error("GVD[%s]: replication thread has not received any gaps yet", RH->hostname);
                std::this_thread::sleep_for(std::chrono::seconds(1));
            }
            error("GVD[%s]: replication thread will process %zu gaps", RH->hostname, NumGaps);

            /*
             * Find the next gap we want to process.
             */

            TimeRange Gap;
            {
                std::lock_guard<Mutex> L(SenderMutex);
                if (SenderGaps.size() == 0) {
                    error("GVD[%s]: replication thread has no more gaps", RH->hostname);
                    continue;
                }
                Gap = SenderGaps.back();
            }
            error("GVD[%s]: replication thread will fill gap <%ld, %ld>", RH->hostname, Gap.first, Gap.second);

            /*
             * Create a vector that will contain the list of dimensions that
             * we want to fill for this gap. Right now, we consider only
             * dimensions that are in mem.
             */

            std::vector<GapData> GDV;

            rrdhost_rdlock(RH);
            RRDSET *RS;
            rrdset_foreach_read(RS, RH) {
                rrdset_rdlock(RS);
                RRDDIM *RD;
                rrddim_foreach_read(RD, RS) {
                    GapData GD;
                    GD.setChart(RS->id);
                    GD.setDimension(RD->id);
                    GDV.push_back(GD);
                }
                rrdset_unlock(RS);
            }
            rrdhost_unlock(RH);

            /*
             * Start sending the gap data for each individual dimension
             */

            RateLimiter RL(Cfg.MaxQueriesPerSecond, std::chrono::seconds(1));
            for (GapData &GD : GDV) {
                RL.request();

                /*
                 * Find the dim we are interested in and query it.
                 */

                rrdhost_rdlock(RH);
                RRDSET *RS = rrdset_find(RH, GD.getChart().c_str());
                if (!RS) {
                    error("GVD[%s]: Could not find chart %s for dim %s to fill <%ld, %ld>",
                          RH->hostname, GD.getChart().c_str(), GD.getDimension().c_str(), Gap.first, Gap.second);
                    rrdhost_unlock(RH);
                    continue;
                }

                rrdset_rdlock(RS);

                if (!rrdset_push_chart_definition_now(RS)) {
                    /* We shouldn't check this chart upstream. Unlock the
                     * chart/host and continue with the next entry in the
                     * GapData vector */
                    rrdset_unlock(RS);
                    rrdhost_unlock(RH);
                    continue;
                }

                RRDDIM *RD = rrddim_find(RS, GD.getDimension().c_str());
                if (!RS) {
                    error("GVD[%s]: Could not find dim %s.%s to fill <%ld, %ld>",
                          RH->hostname, GD.getChart().c_str(), GD.getDimension().c_str(), Gap.first, Gap.second);
                    rrdset_unlock(RS);
                    rrdhost_unlock(RH);
                    continue;
                }

                error("GVD[%s]: Filling %s.%s -- <%ld, %ld>",
                      RH->hostname, GD.getChart().c_str(), GD.getDimension().c_str(), Gap.first, Gap.second);

                GD.setStorageNumbers(Query::getSNs(RD, Gap.first, Gap.second));

                rrdset_unlock(RS);
                rrdhost_unlock(RH);

                /*
                 * Try to send the data upstream
                 */

                while (!GD.push(RH->sender)) {
                    error("GVD[%s]: Sender buffer is full (Dim=%s.%s, Gap=<%ld, %ld>)",
                          RH->hostname, GD.getChart().c_str(), GD.getDimension().c_str(), Gap.first, Gap.second);
                    std::this_thread::sleep_for(std::chrono::seconds(1));
                }
            }

            /*
             * Now that we filled this gap, send a GAPFILL command to let
             * the parent know that we have no more data to send
             */

            sender_start(RH->sender);
            buffer_sprintf(RH->sender->build, "DROPGAP \"%ld\" \"%ld\"\n", Gap.first, Gap.second);
            sender_commit(RH->sender);

            /*
             * Nothing else to do... Just remove the gap
             */
            {
                std::lock_guard<Mutex> L(SenderMutex);
                SenderGaps.erase(std::remove(SenderGaps.begin(), SenderGaps.end(), Gap), SenderGaps.end());
            }
        }
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
