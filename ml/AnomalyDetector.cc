// SPDX-License-Identifier: GPL-3.0-or-later

#include "AnomalyDetector.h"
#include "Config.h"
#include "Query.h"

using namespace ml;

std::vector<bool> AnomalyDetector::getAnomalyBitVector(RRDDIM *RD, bool IsAnomalyRateRD) {
    std::vector<bool> ABV(BeforeT - AfterT + 1, false);

    Query Q = Query(RD);

    time_t StartT = std::max(AfterT, Q.oldestTime());
    time_t EndT = std::min(BeforeT, Q.latestTime());

    if (StartT > EndT)
        return ABV;

    Q.init(StartT, EndT);

    while (!Q.isFinished()) {
        auto P = Q.nextMetric();
        unsigned Idx = P.first - AfterT;
        assert(Idx < ABV.size());

        if (IsAnomalyRateRD && does_storage_number_exist(P.second))
            ABV[Idx] = (unpack_storage_number(P.second) >= Cfg.AnomalousHostRateThreshold);
        else
            ABV[Idx] = P.second & SN_ANOMALOUS;
    }

    return ABV;
}

std::vector<AnomalyEvent>
AnomalyDetector::getAnomalyEvents(RRDDIM *RD, unsigned MinSize, double MinRate) {
    std::vector<AnomalyEvent> AEV;

    std::vector<bool> ABV = getAnomalyBitVector(RD, true);
    if (ABV.size() < MinSize)
        return AEV;

    double Counter = 0;
    for (unsigned Idx = 0; Idx != MinSize; Idx++)
        Counter += ABV[Idx];

    double Rate = Counter / MinSize;
    if (Rate >= MinRate)
        AEV.push_back(std::make_pair(AfterT, AfterT + MinSize - 1));

    for (unsigned WindowStart = 1, WindowEnd = MinSize;
         WindowEnd != ABV.size();
         WindowStart++, WindowEnd++)
    {
        Counter += ABV[WindowEnd] - ABV[WindowStart - 1];
        Rate = Counter / MinSize;

        if (Rate >= MinRate)
            AEV.push_back(std::make_pair(AfterT + WindowStart, AfterT + WindowEnd));
    }

    if (AEV.size() == 0)
        return AEV;

    unsigned N = 1;

    for (unsigned Idx = 1; Idx != AEV.size(); Idx++) {
        AnomalyEvent &PrevAE = AEV[N - 1];
        AnomalyEvent &CurrAE = AEV[Idx];

        if (CurrAE.first <= PrevAE.second)
            PrevAE.second = CurrAE.second;
        else
            AEV[N++] = CurrAE;
    }

    AEV.resize(N);
    return AEV;
}

AnomalyEventInfo AnomalyDetector::getAnomalyEventInfo(RRDDIM *RD) {
    AnomalyEventInfo AEI;

    AEI.Name = RD->name;

    std::vector<bool> ABV = getAnomalyBitVector(RD);

    AEI.AnomalyStatus.reserve(ABV.size());
    AEI.AnomalyRate = 0.0;

    for (const auto &B : ABV) {
        AEI.AnomalyStatus.push_back(B);
        AEI.AnomalyRate += B ? 1 : 0;
    }

    if (ABV.size())
        AEI.AnomalyRate /= ABV.size();

    return AEI;
}
