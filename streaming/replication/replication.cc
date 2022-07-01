#include "replication-private.h"
#include "Logger.h"

using namespace replication;

class Host {
public:
    Host(RRDHOST *RH) : RH(RH), L(RH->hostname) {}

    void setReceiverGaps(std::vector<TimeRange> &TRs) {
        std::lock_guard<Mutex> Lock(ReceiverMutex);
        ReceiverGaps = TRs;
    }

    std::vector<TimeRange> getReceiverGaps() {
        std::lock_guard<Mutex> Lock(ReceiverMutex);
        return ReceiverGaps;
    }

    void setSenderGaps(std::vector<TimeRange> &TRs) {
        std::lock_guard<Mutex> Lock(SenderMutex);
        SenderGaps = TRs;
    }

    std::vector<TimeRange> getSenderGaps() {
        std::lock_guard<Mutex> Lock(SenderMutex);
        return SenderGaps;
    }

    void startReplicationThread() {
        ReplicationThread = std::thread(&Host::senderReplicateGaps, this);
    }

    void stopReplicationThread() {
        netdata_thread_cancel(ReplicationThread.native_handle());
        ReplicationThread.join();

        time_t FirstEntryT = rrdhost_first_entry_t(RH);
        time_t LastEntryT = rrdhost_last_entry_t(RH);
        replication_save_host_entries_range(&RH->host_uuid, FirstEntryT, LastEntryT);

        error("GVD[stopReplicationThread]: %s entries [%ld, %ld]",
              RH->hostname, FirstEntryT, LastEntryT);
    }

    /* adds a new gap */
    void receiverConnect() {
        std::lock_guard<Mutex> Lock(ReceiverMutex);

        time_t CurrT = now_realtime_sec();

        time_t FirstEntryT = 0, LastEntryT = 0;
        replication_load_host_entries_range(&RH->host_uuid, &FirstEntryT, &LastEntryT);

        if (LastEntryT == 0)
            LastEntryT = CurrT - Cfg.SecondsToReplicateOnFirstConnection + 1;
        else
            LastEntryT -= maxUpdateEvery();

        if (LastEntryT >= CurrT) {
            error("[%s] Skipping invalid replication time range on connect: <%ld, %ld>", RH->hostname, LastEntryT, CurrT);
            return;
        }

        TimeRange TR(LastEntryT, CurrT + 60);
        std::vector<TimeRange> NewTRs = splitTimeRange(TR, Cfg.MaxEntriesPerGapData);
        for (const TimeRange &NewTR : NewTRs)
            ReceiverGaps.push_back(NewTR);

        ReceiverGaps = coalesceTimeRanges(ReceiverGaps);
    }

    /* drops a received gap */
    void receiverDropGap(const TimeRange &TR) {
        std::lock_guard<Mutex> Lock(ReceiverMutex);
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
                    std::lock_guard<Mutex> Lock(SenderMutex);
                    NumGaps = SenderGaps.size();
                }

                std::this_thread::sleep_for(std::chrono::seconds(1));
            }

            /*
             * Find the next gap we want to process.
             */

            TimeRange Gap;
            {
                std::lock_guard<Mutex> Lock(SenderMutex);
                if (SenderGaps.size() == 0)
                    continue;
                Gap = SenderGaps.back();
            }

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
             * Sleep enough time to let the streaming thread push
             * chart defs & 1st values of dims.
             */
            time_t MaxUE = maxUpdateEvery();
            error("[%s]: sleeping for max update_every=%ld", RH->hostname, MaxUE);
            std::this_thread::sleep_for(std::chrono::seconds(2 * MaxUE));

            /*
             * Start sending the gap data for each individual dimension
             */

            RateLimiter RL(Cfg.MaxQueriesPerSecond, std::chrono::seconds(1));
            for (GapData &GD : GDV) {
                RL.request();

                /*
                 * Sleep while we are receiving gaps for this host
                 */

                while (!netdata_exit) {
                    size_t NumReceiverGaps = 0;
                    {
                        std::lock_guard<Mutex> Lock(ReceiverMutex);
                        NumReceiverGaps = ReceiverGaps.size();
                    }

                    if (!NumReceiverGaps)
                        break;

                    error("[%s] Replication thread sleeping because we are receiving gaps", RH->hostname);
                    std::this_thread::sleep_for(std::chrono::seconds(1));
                }

                /*
                 * Find the dim we are interested in and query it.
                 */

                rrdhost_rdlock(RH);
                RRDSET *RS = rrdset_find(RH, GD.getChart().c_str());
                if (!RS) {
                    error("[%s] Could not find chart %s for dim %s to fill <%ld, %ld>",
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
                    error("[%s] Could not find dim %s.%s to fill <%ld, %ld>",
                          RH->hostname, GD.getChart().c_str(), GD.getDimension().c_str(), Gap.first, Gap.second);
                    rrdset_unlock(RS);
                    rrdhost_unlock(RH);
                    continue;
                }

                GD.setStorageNumbers(Query::getSNs(RD, Gap.first, Gap.second));

                rrdset_unlock(RS);
                rrdhost_unlock(RH);

                /*
                 * Try to send the data upstream
                 */

                while (!GD.push(RH->sender)) {
                    error("[%s] Sender buffer is full (Dim=%s.%s, Gap=<%ld, %ld>)",
                          RH->hostname, GD.getChart().c_str(), GD.getDimension().c_str(), Gap.first, Gap.second);
                    std::this_thread::sleep_for(std::chrono::seconds(1));
                }

                L.senderFilledGap(GD);
            }

            /*
             * Now that we filled this gap, send a GAPFILL command to let
             * the parent know that we have no more data to send
             */

            sender_start(RH->sender);
            buffer_sprintf(RH->sender->build, "DROPGAP \"%ld\" \"%ld\"\n", Gap.first, Gap.second);
            sender_commit(RH->sender);

            error("[%s] Sent DROPGAP command for time range <%ld, %ld>",
                  RH->hostname, Gap.first, Gap.second);

            /*
             * Nothing else to do... Just remove the gap
             */
            {
                std::lock_guard<Mutex> Lock(SenderMutex);
                SenderGaps.erase(std::remove(SenderGaps.begin(), SenderGaps.end(), Gap), SenderGaps.end());
            }
            L.senderDroppedGap(Gap);
        }
    }

    time_t maxUpdateEvery() const {
        rrdhost_rdlock(RH);
        time_t MaxUE = RH->rrd_update_every;
        RRDSET *RS;
        rrdset_foreach_read(RS, RH) {
            rrdset_rdlock(RS);
            MaxUE = std::max<time_t>(RS->update_every, MaxUE);

            RRDDIM *RD;
            rrddim_foreach_read(RD, RS) {
                MaxUE = std::max<time_t>(RD->update_every, MaxUE);
            }
            rrdset_unlock(RS);
        }
        rrdhost_unlock(RH);

        return MaxUE;
    }

    const char *getLogs() {
        return strdupz(L.serialize().c_str());
    }

    Logger &getLogger() {
        return L;
    }

private:
    RRDHOST *RH;
    Logger L;

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

    /*
     * Log info
     */
    auto &L = H->getLogger();
    L.createdHost(TRs);
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
     * Log info
     */
    auto &L = H->getLogger();
    L.deletedHost(TRs);

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

    /*
     * Log info
     */
    auto &L = H->getLogger();
    L.startedReplicationThread();
}

void replication_thread_stop(RRDHOST *RH) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    H->stopReplicationThread();

    /*
     * Log info
     */
    auto &L = H->getLogger();
    L.stoppedReplicationThread();
}

void replication_receiver_connect(RRDHOST *RH, char *Buf, size_t Len) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    H->receiverConnect();
    std::vector<TimeRange> TRs = H->getReceiverGaps();
    serializeTimeRanges(TRs, Buf, Len);

    /*
     * Log info
     */
    auto &L = H->getLogger();
    L.receiverSentGaps(TRs);
}

void replication_sender_connect(RRDHOST *RH, const char *Buf, size_t Len) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    std::vector<TimeRange> TRs = deserializeTimeRanges(Buf, Len);

    /* Assign the recv'd gaps to the host. The parent sends the gaps
     * in increasing timestamp order; reverse the vector because
     * we always pop from the back */
    std::reverse(TRs.begin(), TRs.end());
    H->setSenderGaps(TRs);

    /*
     * Log info
     */
    auto &L = H->getLogger();
    L.senderReceivedGaps(TRs);
}

bool replication_receiver_fill_gap(RRDHOST *RH, const char *Buf) {
    GapData GD = GapData::fromBase64(Buf);

    Host *H = static_cast<Host *>(RH->repl_handle);

    /*
     * Log info
     */
    Logger &L = H->getLogger();
    L.receiverFilledGap(GD);

    return GD.flushToDBEngine(RH);
}

void replication_receiver_drop_gap(RRDHOST *RH, time_t After, time_t Before) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H)
        return;

    TimeRange TR(After, Before);
    H->receiverDropGap(TR);

    /*
     * Log info
     */
    auto &L = H->getLogger();
    L.receiverDroppedGap(TR);
}

const char *replication_logs(RRDHOST *RH) {
    Host *H = static_cast<Host *>(RH->repl_handle);
    if (!H) {
        std::stringstream SS;
        SS << "Replication is not enabled for host " << RH->hostname;
        return strdupz(SS.str().c_str());
    }

    return H->getLogs();
}
