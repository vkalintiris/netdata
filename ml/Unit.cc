// SPDX-License-Identifier: GPL-3.0-or-later

#include "Unit.h"
#include "Query.h"

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

template<>
std::pair<CalculatedNumber *, size_t>
TrainableDimension<Dimension>::getCalculatedNumbers(size_t MinN, size_t MaxN) {
    CalculatedNumber *CNs = new CalculatedNumber[MaxN * (Cfg.LagN + 1)]();

    Dimension& Dim = static_cast<Dimension&>(*this);
    RRDDIM *RD = Dim.getRD();

    // Figure out what our time window should be.
    time_t BeforeT = now_realtime_sec() - 1;
    time_t AfterT = BeforeT - (MaxN * Dim.updateEvery().count());

    BeforeT -=  (BeforeT % Dim.updateEvery().count());
    AfterT -= (AfterT % Dim.updateEvery().count());

    time_t LastT = Dim.latestTime();
    BeforeT = (BeforeT > LastT) ? LastT : BeforeT;

    time_t FirstT = Dim.oldestTime();
    AfterT = (AfterT < FirstT) ? FirstT : AfterT;

    if (AfterT >= BeforeT)
        return { CNs, 0 };

    // Start the query.
    unsigned Idx = 0;
    unsigned CollectedValues = 0;
    unsigned TotalValues = 0;

    CalculatedNumber QuietNaN = std::numeric_limits<CalculatedNumber>::quiet_NaN();
    CalculatedNumber LastValue = QuietNaN;

    Query Q = Query(RD);

    Q.init(AfterT, BeforeT);
    while (!Q.isFinished()) {
        if (Idx == MaxN)
            break;

        auto P = Q.nextMetric();
        storage_number SN = P.second;

        if (does_storage_number_exist(SN)) {
            CNs[Idx] = unpack_storage_number_dbl(SN);
            LastValue = CNs[Idx];
            CollectedValues++;
        } else
            CNs[Idx] = LastValue;

        Idx++;
    }
    TotalValues = Idx;

    if (CollectedValues < MinN)
        return { CNs, CollectedValues };

    // Find first non-NaN value.
    for (Idx = 0; std::isnan(CNs[Idx]); Idx++, TotalValues--) { }

    // Overwrite NaN values.
    if (Idx != 0)
        memmove(CNs, &CNs[Idx], sizeof(CalculatedNumber) * TotalValues);

    return { CNs, TotalValues };
}

template<>
MLError TrainableDimension<Dimension>::train(TimePoint &Now) {
    std::unique_lock<std::mutex> Lock(Mutex, std::defer_lock);
    if (!Lock.try_lock())
        return MLError::TryLockFailed;

    if ((LastTrainedAt + Cfg.TrainEvery) >= Now)
        return MLError::ShouldNotTrainNow;
    LastTrainedAt = Now;

    Dimension &Dim = static_cast<Dimension&>(*this);

    unsigned MinN = Cfg.MinTrainSecs / Dim.updateEvery();
    unsigned MaxN = Cfg.TrainSecs / Dim.updateEvery();

    std::pair<CalculatedNumber *, unsigned> P = getCalculatedNumbers(MinN, MaxN);

    CalculatedNumber *CNs = P.first;
    unsigned N = P.second;

    if (N < MinN) {
        delete[] CNs;
        return MLError::MissingData;
    }

    SamplesBuffer SB = SamplesBuffer(CNs, N, 1, Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);
    KM.train(SB);

    delete[] CNs;
    HasModel = true;
    return MLError::Success;
}

template<>
std::pair<MLError, bool> TrainableDimension<Dimension>::predict() {
    std::unique_lock<std::mutex> Lock(Mutex, std::defer_lock);
    if (!Lock.try_lock())
        return { MLError::TryLockFailed, AnomalyBit };

    // Should we "reset" AnomalyScore here?
    if (!HasModel)
        return { MLError::NoModel, AnomalyBit };

    unsigned N = Cfg.DiffN + Cfg.SmoothN + Cfg.LagN;
    std::pair<CalculatedNumber *, unsigned> P = getCalculatedNumbers(N, N);
    CalculatedNumber *CNs = P.first;

    if (P.second != N) {
        delete[] CNs;
        return { MLError::MissingData, AnomalyBit };
    }

    SamplesBuffer SB = SamplesBuffer(CNs, N, 1, Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);
    AnomalyScore = KM.anomalyScore(SB);
    delete[] CNs;

    AnomalyBit = AnomalyScore >= Cfg.AnomalyScoreThreshold;
    return { MLError::Success, AnomalyBit }; 
}

template<>
void TrainableDimension<Dimension>::updateMLRD(RRDSET *MLRS) {
    if (AnomalyScoreRD && AnomalyBitRD) {
        rrddim_set_by_pointer(MLRS, AnomalyScoreRD, AnomalyScore * 100);
        rrddim_set_by_pointer(MLRS, AnomalyBitRD, AnomalyBit * 100);
        return;
    }

    std::stringstream AnomalyScoreName;
    AnomalyScoreName << getRD()->name << "-as";
    AnomalyScoreRD = rrddim_add(MLRS, AnomalyScoreName.str().c_str(), NULL, 1, 100,
                                RRD_ALGORITHM_ABSOLUTE);

    std::stringstream AnomalyBitName;
    AnomalyBitName << getRD()->name << "-ab";
    AnomalyBitRD = rrddim_add(MLRS, AnomalyBitName.str().c_str(), NULL, 1, 1,
                              RRD_ALGORITHM_ABSOLUTE);

    rrddim_flag_clear(AnomalyScoreRD, RRDDIM_FLAG_HIDDEN);
    rrddim_flag_clear(AnomalyBitRD, RRDDIM_FLAG_HIDDEN);
    if (rrddim_flag_check(getRD(), RRDDIM_FLAG_HIDDEN)) {
        rrddim_flag_set(AnomalyScoreRD, RRDDIM_FLAG_HIDDEN);
        rrddim_flag_set(AnomalyBitRD, RRDDIM_FLAG_HIDDEN);
    }
}
