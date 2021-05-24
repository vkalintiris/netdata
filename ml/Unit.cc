// SPDX-License-Identifier: GPL-3.0-or-later

#include "Unit.h"

using namespace ml;

/*
 * Copy of the unpack_storage_number which allows us to convert
 * a storage_number to double.
 */
static CalculatedNumber unpack_storage_number_dbl(storage_number value) {
    if(!value)
        return 0;

    int sign = 0, exp = 0;
    int factor = 10;

    // bit 32 = 0:positive, 1:negative
    if(unlikely(value & (1 << 31)))
        sign = 1;

    // bit 31 = 0:divide, 1:multiply
    if(unlikely(value & (1 << 30)))
        exp = 1;

    // bit 27 SN_EXISTS_100
    if(unlikely(value & (1 << 26)))
        factor = 100;

    // bit 26 SN_EXISTS_RESET
    // bit 25 SN_EXISTS

    // bit 30, 29, 28 = (multiplier or divider) 0-7 (8 total)
    int mul = (value & ((1<<29)|(1<<28)|(1<<27))) >> 27;

    // bit 24 to bit 1 = the value, so remove all other bits
    value ^= value & ((1<<31)|(1<<30)|(1<<29)|(1<<28)|(1<<27)|(1<<26)|(1<<25)|(1<<24));

    CalculatedNumber CN = value;

    if(exp) {
        for(; mul; mul--)
            CN *= factor;
    }
    else {
        for( ; mul ; mul--)
            CN /= 10;
    }

    if(sign)
        CN = -CN;

    return CN;
}

std::pair<CalculatedNumber *, unsigned>
Unit::getCalculatedNumbers(unsigned MinN, unsigned MaxN) {
    CalculatedNumber *CNs = new CalculatedNumber[MaxN * (Cfg.LagN + 1)]();

    struct rrddim_volatile::rrddim_query_ops *Ops = &RD->state->query_ops;
    struct rrddim_query_handle Handle;

    // Figure out what our time window should be.
    time_t BeforeT = now_realtime_sec() - 1;
    time_t AfterT = BeforeT - (MaxN * updateEvery());

    BeforeT -=  (BeforeT % updateEvery());
    AfterT -= (AfterT % updateEvery());

    time_t LastT = Ops->latest_time(RD);
    BeforeT = (BeforeT > LastT) ? LastT : BeforeT;

    time_t FirstT = Ops->oldest_time(RD);
    AfterT = (AfterT < FirstT) ? FirstT : AfterT;

    if (AfterT >= BeforeT)
        return { CNs, 0 };

    // Start the query.
    unsigned Idx = 0;
    unsigned CollectedValues = 0;
    unsigned TotalValues = 0;

    CalculatedNumber QuietNaN = std::numeric_limits<CalculatedNumber>::quiet_NaN();
    CalculatedNumber LastValue = QuietNaN;

    Ops->init(RD, &Handle, AfterT, BeforeT);
    while (!Ops->is_finished(&Handle)) {
        if (Idx == MaxN)
            break;

        time_t CurrT;
        storage_number SN = Ops->next_metric(&Handle, &CurrT);

        if (does_storage_number_exist(SN)) {
            CNs[Idx] = unpack_storage_number_dbl(SN);
            LastValue = CNs[Idx];
            CollectedValues++;
        } else
            CNs[Idx] = LastValue;

        Idx++;
    }
    TotalValues = Idx;
    Ops->finalize(&Handle);

    if (CollectedValues < MinN)
        return { CNs, 0 };

    // Find first non-NaN value.
    for (Idx = 0; std::isnan(CNs[Idx]); Idx++, TotalValues--) { }

    // Overwrite NaN values.
    if (Idx != 0)
        memmove(CNs, &CNs[Idx], sizeof(CalculatedNumber) * TotalValues);

    return { CNs, TotalValues };
}

bool Unit::train(TimePoint &Now) {
    std::unique_lock<std::mutex> Lock(Mutex);

    if ((LastTrainedAt + Cfg.TrainEvery) > Now)
        return false;

    LastTrainedAt = SteadyClock::now();

    unsigned MinN = Cfg.MinTrainSecs / Millis{updateEvery() * 1000};
    unsigned MaxN = Cfg.TrainSecs / Millis{updateEvery() * 1000};

    std::pair<CalculatedNumber *, unsigned> P = getCalculatedNumbers(MinN, MaxN);

    CalculatedNumber *CNs = P.first;
    unsigned N = P.second;

    Trained = false;

    if (N >= MinN) {
        SamplesBuffer SB = SamplesBuffer(CNs, N, 1, Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);
        KM.train(SB);

        Trained = true;
        HasModel = true;
    }

    delete[] CNs;
    return true;
}

void Unit::predict() {
    if (!Mutex.try_lock())
        return;


    if (!HasModel)
        return;

    unsigned N = Cfg.DiffN + Cfg.SmoothN + Cfg.LagN;
    std::pair<CalculatedNumber *, unsigned> P = getCalculatedNumbers(N, N);
    CalculatedNumber *CNs = P.first;

    Predicted = false;
    if (P.second == N) {
        SamplesBuffer SB = SamplesBuffer(CNs, N, 1, Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);
        AnomalyScore = KM.anomalyScore(SB);

        Predicted = true;
    }

    delete[] CNs;
    Mutex.unlock();
}
