// SPDX-License-Identifier: GPL-3.0-or-later

#include "AnomalyDetector.h"
#include "Query.h"

using namespace ml;

std::vector<bool> AnomalyDetector::getAnomalyBitVector(RRDDIM *RD) {
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
        ABV[Idx] = P.second & SN_ANOMALOUS;
    }

    return ABV;
}

std::vector<AnomalyEvent>
AnomalyDetector::getAnomalyEvents(RRDDIM *RD, unsigned MinSize, double MinRate) {
    std::vector<AnomalyEvent> AEV;

    std::vector<bool> ABV = getAnomalyBitVector(RD);
    if (ABV.size() < MinSize)
        return AEV;

    int WindowStart = 0;
    int WindowEnd = MinSize - 1;

    double Counter = 0;
    for (unsigned Idx = 0; Idx != MinSize; Idx++)
        Counter += ABV[Idx];

    double Rate = Counter / MinSize;
    if (Rate >= MinRate)
        AEV.push_back(std::make_pair(WindowStart, WindowEnd));

    for (unsigned Idx = MinSize; Idx != ABV.size(); Idx++) {
        WindowStart++;
        WindowEnd++;

        Counter += ABV[Idx] - ABV[Idx - MinSize];
        Rate = Counter / MinSize;

        if (Rate >= MinRate)
            AEV.push_back(std::make_pair(WindowStart, WindowEnd));
    }

    if (AEV.size() == 0)
        return AEV;

    int NumAnomalyEvents = 1;
    AnomalyEvent &AE = AEV[0];

    for (unsigned Idx = 1; Idx != AEV.size(); Idx++) {
        AnomalyEvent CurrAE = AEV[Idx];

        if (CurrAE.first <= AE.second) {
            AE.second = CurrAE.second;
        } else {
            AEV[NumAnomalyEvents] = AE;
            AE = CurrAE;
            NumAnomalyEvents += 1;
        }
    }

    AEV.resize(NumAnomalyEvents);
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
