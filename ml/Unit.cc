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
    using Extent = std::pair<unsigned /* Offset */, unsigned /* Length */>;

    Extent MaxExtent{0, 0}, CurrExtent{0, 0};
    unsigned Idx = 0;

    Ops->init(RD, &Handle, AfterT, BeforeT);
    while (!Ops->is_finished(&Handle)) {
        if (Idx == MaxN)
            break;

        time_t CurrT;
        storage_number SN = Ops->next_metric(&Handle, &CurrT);

        if (!does_storage_number_exist(SN)) {
            CurrExtent = { ++Idx , 0 };
            continue;
        }

        if (++CurrExtent.second >= MaxExtent.second)
            MaxExtent = CurrExtent;

        CNs[Idx++] = unpack_storage_number_dbl(SN);
    }
    Ops->finalize(&Handle);

    // Return if we didn't manage to collect enough samples.
    if (MaxExtent.second < MinN)
        return { CNs, 0 };

    // Move window to the beggining of the buffer.
    if (MaxExtent.first && MaxExtent.second)
        memmove(CNs, &CNs[MaxExtent.first], sizeof(int) * MaxExtent.second);

    return { CNs, MaxExtent.second };
}

void Unit::updateMLUnit(RRDSET *MLRS) {
    if (MLRD) {
        rrddim_set_by_pointer(MLRS, MLRD, getAnomalyScore() * 10000.0);
        return;
    }

    MLRD = rrddim_add(MLRS, RD->id, NULL, 1, 10000, RRD_ALGORITHM_ABSOLUTE);

    rrddim_flag_clear(MLRD, RRDDIM_FLAG_HIDDEN);
    if (rrddim_flag_check(RD, RRDDIM_FLAG_HIDDEN))
        rrddim_flag_set(MLRD, RRDDIM_FLAG_HIDDEN);
}

void Unit::train() {
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
}

void Unit::predict() {
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
}
