#include "replication-private.h"

using namespace replication;

static pb::TimeRange timeRangeToProto(const TimeRange &TR) {
    pb::TimeRange PBTR;

    PBTR.set_after(TR.first);
    PBTR.set_before(TR.second);
    return PBTR;
}

static pb::TimeRanges timeRangesToProto(std::vector<TimeRange> TRs) {
    pb::TimeRanges PBTRs;

    for (const TimeRange &TR : TRs)
        PBTRs.mutable_trs()->Add(timeRangeToProto(TR));

    return PBTRs;
}

std::vector<TimeRange> replication::splitTimeRange(const TimeRange &TR, size_t Epoch) {
    size_t Duration = TR.second - TR.first + 1;
    size_t NumEpochs = (Duration / Epoch) + (Duration % Epoch != 0);

    std::vector<TimeRange> TRs;
    TRs.reserve(NumEpochs);

    for (time_t Offset = TR.first; TRs.size() != NumEpochs; Offset += Epoch)
        TRs.emplace_back(Offset, Offset + (Epoch - 1));
    TRs.back().second = TR.second;

    return TRs;
}

void replication::serializeTimeRanges(std::vector<TimeRange> TRs, char *Buf, size_t Len) {
    pb::TimeRanges PBTRs = timeRangesToProto(TRs);;

    size_t MsgSize = PROTO_COMPAT_MSG_SIZE(PBTRs);
    if (MsgSize > Len)
        return;

    PBTRs.SerializeToArray(Buf, Len);
}

std::vector<TimeRange> replication::deserializeTimeRanges(const char *Buf, size_t Len) {
    std::vector<TimeRange> TRs;

    pb::TimeRanges PBTRs;
    if (!PBTRs.ParseFromArray(Buf, Len)) {
        TRs.reserve(PBTRs.trs_size());
        for (int Idx = 0; Idx != PBTRs.trs_size(); Idx++) {
            pb::TimeRange PBTR = PBTRs.trs(Idx);
            TRs.emplace_back(PBTR.after(), PBTR.before());
        }
    }

    return TRs;
}

std::vector<TimeRange> replication::coalesceTimeRanges(std::vector<TimeRange> &TRs) {
    std::sort(TRs.rbegin(), TRs.rend());
    {
        while (TRs.size() > Cfg.MaxNumGapsToReplicate)
            TRs.pop_back();

        std::reverse(TRs.begin(), TRs.end());
    }

    std::vector<TimeRange> RetTRs;

    if (TRs.size() == 0)
        return RetTRs;

    // Pick the most recent connection time when the latest db time is the same
    RetTRs.push_back(TRs[0]);
    for (size_t Idx = 1; Idx != TRs.size(); Idx++) {
        if (RetTRs.back().first == TRs[Idx].first)
            RetTRs.back() = TRs[Idx];
        else
            RetTRs.push_back(TRs[Idx]);
    }

    /*
     * TODO: should we coalesce gaps that are than 1024 seconds apart???
     *       we would end up with more data transferred but fewer new DB
     *       engine pages created.
     */

    return RetTRs;
}
