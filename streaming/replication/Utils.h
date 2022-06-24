#ifndef REPLICATION_UTILS_H
#define REPLICATION_UTILS_H

#include "replication-private.h"

namespace replication {

/*
 * Wraps a standard std::mutex and enables/disables cancelability
 * in lock/unlock operations.
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

        netdata_thread_enable_cancelability();
        return false;
    }

private:
    std::mutex M;
};

/*
 * Wraps the query ops under a single class. It exposes just a single
 * static function "getSNs()" which we use to get the data of a dimensions
 * for the time range we want.
 */
class Query {
public:
    static std::vector<std::pair<time_t, storage_number>> getSNs(RRDDIM *RD, time_t After, time_t Before)
    {
        std::vector<std::pair<time_t, storage_number>> SNs;

        if (After > Before) {
            error("[%s] Tried to query %s.%s with <%ld, %ld>",
                  RD->rrdset->rrdhost->hostname, RD->rrdset->id, RD->id, After, Before);
            return SNs;
        }

        Query Q(RD);

        After = std::max(After, Q.oldestTime());
        Before = std::min(Before, Q.latestTime());

        if (After > Before) {
            error("[%s] Ignoring invalid Query range <%ld, %ld>",
                  RD->rrdset->rrdhost->hostname, After, Before);
            return SNs;
        }

        SNs.reserve(Before - After + 1);

        Q.init(After, Before);
        while (!Q.isFinished()) {
            auto P = Q.nextMetric();
            if (P.first < After || P.first > Before)
                continue;
            SNs.push_back(P);
        }

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
        time_t CurrT, EndT;
        SN_FLAGS Flags;

        calculated_number CN = Ops->next_metric(&Handle, &CurrT, &EndT, &Flags);
        storage_number SN = pack_storage_number(CN, Flags);

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

/*
 * A rate-limiter class that is initialized with the number of requests
 * we want to execute in the specified time window. The replication
 * thread uses this to limit the total amount of queries we perform
 * to collect history data of dimensions.
 */
class RateLimiter {

using SystemClock = std::chrono::system_clock;
using TimePoint = std::chrono::time_point<SystemClock>;

public:
    RateLimiter(size_t NumRequests, std::chrono::milliseconds Window)
        : NumRequests(NumRequests), Window(Window),
          Q(NumRequests, TimePoint()), Index(0) {}

    void request() {
        auto CurrT = SystemClock::now();
        bool BelowLimit = (CurrT - Q[Index]) >= Window;

        if (!BelowLimit) {
            std::this_thread::sleep_for(Window * 0.25);
            CurrT = SystemClock::now();
        }

        addTimePoint(CurrT);
    }

private:
    void addTimePoint(TimePoint TP) {
        Q[Index] = TP;
        Index = (Index + 1) % NumRequests;
    }

private:
    size_t NumRequests;
    std::chrono::milliseconds Window;

    std::vector<TimePoint> Q;
    size_t Index = 0;
};

} // namespace replication

#endif /* REPLICATION_UTILS_H */
