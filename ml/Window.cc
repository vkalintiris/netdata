// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Window.h"
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

/*
 * Get a buffer of values contained in the window.
 */
CalculatedNumber *ml::Window::getCalculatedNumbers() {
    assert(std::numeric_limits<CalculatedNumber>::has_quiet_NaN);

    CalculatedNumber *CNs = new CalculatedNumber[NumSamples * (Cfg.LagN + 1)]();

    RRDDIM *RD = U->getDim();
    struct rrddim_volatile::rrddim_query_ops *Ops = &RD->state->query_ops;
    struct rrddim_query_handle Handle;

    /*
     * Figure out what our time window should be.
     */

    time_t BeforeT = now_realtime_sec() - 1;
    time_t AfterT = BeforeT - (NumSamples * U->updateEvery());

    BeforeT -=  (BeforeT % U->updateEvery());
    AfterT -= (AfterT % U->updateEvery());

    time_t LastT = Ops->latest_time(RD);
    BeforeT = (BeforeT > LastT) ? LastT : BeforeT;

    time_t FirstT = Ops->oldest_time(RD);
    AfterT = (AfterT < FirstT) ? FirstT : AfterT;

    if (AfterT >= BeforeT)
        return CNs;

    /*
     * Start the query.
     */

    unsigned Idx = 0;

    Ops->init(RD, &Handle, AfterT, BeforeT);
    while (!Ops->is_finished(&Handle)) {
        time_t CurrT;

        storage_number SN = Ops->next_metric(&Handle, &CurrT);

        if (!does_storage_number_exist(SN)) {
            CNs[Idx] = std::numeric_limits<CalculatedNumber>::quiet_NaN();
            NumEmpty++;
        } else if (did_storage_number_reset(SN)) {
            NumReset++;
        }

        CNs[Idx] = unpack_storage_number_dbl(SN);

        Idx += 1;
        if (Idx == NumSamples)
            break;
    }
    Ops->finalize(&Handle);

    NumCollected = Idx;

    if (NumEmpty == 0 && NumCollected == NumSamples)
        return CNs;

    if (NumReset) {
        error("Found %u overflown numbers", NumReset);
    }

    /*
     * Pack numbers that are not NaNs to the beginning of the array.
     */

    unsigned OldIdx, NewIdx;

    for (OldIdx = 0, NewIdx = 0; OldIdx != NumSamples && NewIdx != NumCollected; OldIdx++) {
        if (std::isnan(CNs[OldIdx]))
            continue;

        CNs[NewIdx++] = OldIdx;
    }

    assert(NewIdx == NumSamples || NewIdx == NumCollected);
    return CNs;
}
