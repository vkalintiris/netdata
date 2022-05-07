#ifndef REPLICATION_UTILS_H
#define REPLICATION_UTILS_H

#include "replication-private.h"

namespace replication {

/*
 * Mutex
 */

class Mutex {
public:
    void lock() {
        netdata_thread_disable_cancelability();
        M.lock();
    }

    void unlock() {
        netdata_thread_enable_cancelability();
        M.unlock();
    }

    bool try_lock() {
        netdata_thread_disable_cancelability();
        if (M.try_lock())
            return true;

        netdata_thread_disable_cancelability();
        return false;
    }

private:
    std::mutex M;
};

/*
 * Query
 */

class Query {
public:
    static std::vector<std::pair<time_t, storage_number>> getSNs(RRDDIM *RD, time_t After, time_t Before)
    {
        std::vector<std::pair<time_t, storage_number>> SNs;

        if (After >= Before) {
            fatal("GVD: Tried to query rd with <%ld, %ld>", After, Before);
            return SNs;
        }

        Query Q(RD);

        error("GVD: Initial Query<After=%ld vs. Oldest=%ld, Before=%ld vs. Latest=%ld>", After, Q.oldestTime(), Before, Q.latestTime());
        After = std::max(After, Q.oldestTime());
        Before = std::min(Before, Q.latestTime());
        error("GVD: Fixed Query<After=%ld vs. Oldest=%ld, Before=%ld vs. Latest=%ld>", After, Q.oldestTime(), Before, Q.latestTime());

        if (After > Before) {
            error("GVD: Ignoring invalid Query range <%ld, %ld>", After, Before);
            return SNs;
        }

        SNs.reserve(Before - After + 1);

        Q.init(After, Before);
        while (!Q.isFinished())
            SNs.push_back(Q.nextMetric());

        return SNs;
    }

private:
    Query(RRDDIM *RD) : RD(RD), Initialized(false) {
        Ops = &RD->state->query_ops;
    }

    time_t latestTime() {
        return Ops->latest_time(RD);
    }

    time_t oldestTime() {
        return Ops->oldest_time(RD);
    }

    void init(time_t AfterT, time_t BeforeT) {
        Ops->init(RD, &Handle, AfterT, BeforeT);
        Initialized = true;
    }

    bool isFinished() {
        return Ops->is_finished(&Handle);
    }

    std::pair<time_t, storage_number> nextMetric() {
        time_t CurrT;
        storage_number SN = Ops->next_metric(&Handle, &CurrT);
        return { CurrT, SN };
    }

    ~Query() {
        if (Initialized)
            Ops->finalize(&Handle);
    }

private:
    RRDDIM *RD;
    struct rrddim_query_ops *Ops;

    bool Initialized;
    struct rrddim_query_handle Handle;
};

} // namespace replication

#endif /* REPLICATION_UTILS_H */
