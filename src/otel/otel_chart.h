// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ND_OTEL_CHART_H
#define ND_OTEL_CHART_H

#include "circular_buffer.h"
#include "otel_iterator.h"
#include "otel_utils.h"
#include "otel_hash.h"

struct Sample {
    uint64_t Value;
    uint32_t TimePoint;
};

struct Dimension {
    std::string Name;
    CircularBuffer<Sample> CB;

    bool empty() const
    {
        return CB.empty();
    }

    size_t numSamples() const
    {
        return CB.size();
    }

    void pushSample(const Sample &S)
    {
        CB.push(S);
    }

    Sample popSample()
    {
        return CB.pop();
    }

    uint32_t startTime() const
    {
        return CB.head().TimePoint;
    }

    uint32_t updateEvery() const
    {
        uint32_t MinDelta = std::numeric_limits<uint32_t>::max();

        for (size_t Idx = 1; Idx < CB.size(); Idx++) {
            assert(CB[Idx - 1].TimePoint < CB[Idx].TimePoint &&
                   "expected samples sorted by time");

            uint32_t Delta = CB[Idx].TimePoint - CB[Idx - 1].TimePoint;
            MinDelta = std::min(MinDelta, Delta);
        }

        return MinDelta;
    }

    int compareCollectionTime(uint32_t LCT, uint32_t UpdateEvery) const
    {
        double StartTP = LCT + UpdateEvery - (UpdateEvery / 2.0);
        double EndTP = LCT + UpdateEvery + (UpdateEvery / 2.0);

        if (startTime() < StartTP)
            return -1;
        else if (startTime() >= EndTP)
            return 1;
        else
            return 0;
    }
};

class Chart {
public:
    void initialize(BlakeId &Id, const pb::ResourceMetrics *RM, const pb::ScopeMetrics *SM, const pb::Metric *M)
    {
        UNUSED(RM);
        UNUSED(SM);

        this->BID = Id;
        Name = M->name();
        Committed = false;
    }

    void initialize(BlakeId &Id, const std::string &Name)
    {
        this->BID = Id;
        this->Name = Name;
        Committed = false;
    }

    void add(const OtelElement &OE)
    {
        absl::string_view DimName = "value";
        if (auto Result = OE.name(); Result.ok()) {
            DimName = *Result.value();
        }

        auto [It, Emplaced] = Dimensions.try_emplace(DimName.data());
        auto &ND = It->second;

        if (Emplaced) {
            ND.Name = DimName;
            Committed = false;
        }

        uint32_t TP = static_cast<uint32_t>(OE.time() / std::chrono::nanoseconds::period::den);
        Sample S{OE.value(1000), TP};
        ND.pushSample(S);
    }

    void process(size_t RampUpThreshold, size_t GapThreshold)
    {
        bool Processed = false;

        if (UpdateEvery.has_value()) {
            Processed = processFastPath();
        }

        if (!Processed)
            processSlowPath(RampUpThreshold, GapThreshold);
    }

    bool processFastPath()
    {
        assert(
            UpdateEvery.has_value() && UpdateEvery.value() && LastCollectedTime.has_value() &&
            LastCollectedTime.value());

        absl::InlinedVector<std::pair<std::string, Sample>, 4> IV;

        bool Ok = false;
        while (true) {
            for (auto &[Name, Dim] : Dimensions) {
                if (Dim.empty()) {
                    return Ok;
                }

                if (Dim.compareCollectionTime(LastCollectedTime.value(), UpdateEvery.value())) {
                    return Ok;
                }

                IV.emplace_back(Name, Dim.popSample());

                IV.clear();
                Ok = true;
            }
        };
    }

    void processSlowPath(size_t RampUpThreshold, size_t GapThreshold)
    {
        UNUSED(GapThreshold);

        assert(RampUpThreshold >= 2);
        assert(!Dimensions.empty());
        assert(!Dimensions.begin()->second.empty());

        if (!UpdateEvery.has_value()) {
            if (maxDataPointsInDimensions() < RampUpThreshold) {
                return;
            }

            UpdateEvery = minUpdateEveryInDimensions();
            assert(UpdateEvery != 0);
            LastCollectedTime = minStartTimeInDimensions() - UpdateEvery.value();
            assert(LastCollectedTime != 0);
        }

        dropPastCollectionTimes();

        if (maxDataPointsInDimensions() < GapThreshold) {
            return;
        }

        UpdateEvery = minUpdateEveryInDimensions();
        assert(UpdateEvery != 0);
        LastCollectedTime = minStartTimeInDimensions() - UpdateEvery.value();
        assert(LastCollectedTime != 0);
    }

    const std::string &name() const
    {
        return Name;
    }

    const absl::flat_hash_map<std::string, Dimension> &dimensions() const
    {
        return Dimensions;
    }

private:
    size_t maxDataPointsInDimensions() const
    {
        size_t N = std::numeric_limits<size_t>::min();

        for (const auto &[Name, Dim] : Dimensions) {
            N = std::max(N, Dim.numSamples());
        }

        return N;
    }

    uint32_t minUpdateEveryInDimensions() const
    {
        uint32_t UE = std::numeric_limits<uint32_t>::max();

        for (const auto &[Name, Dim] : Dimensions) {
            UE = std::min(UE, Dim.updateEvery());
        }

        return UE;
    }

    uint32_t minStartTimeInDimensions() const
    {
        uint32_t TP = std::numeric_limits<uint32_t>::max();

        for (const auto &[Name, Dim] : Dimensions) {
            TP = std::min(TP, Dim.startTime());
        }

        return TP;
    }

    void dropPastCollectionTimes()
    {
        assert(
            UpdateEvery.has_value() && UpdateEvery.value() && LastCollectedTime.has_value() &&
            LastCollectedTime.value());

        double UE = UpdateEvery.value();
        double LCT = LastCollectedTime.value();

        double StartTP = LCT + UE / 2.0;

        for (auto &[Name, Dim] : Dimensions) {
            while (Dim.startTime() < StartTP) {
                Dim.popSample();
            }
        }
    }

private:
    BlakeId BID;
    std::string Name;

    absl::flat_hash_map<std::string, Dimension> Dimensions;
    std::optional<uint32_t> UpdateEvery;
    std::optional<uint32_t> LastCollectedTime;
    bool Committed = false;
};

template <> struct fmt::formatter<Sample> {
    constexpr auto parse(format_parse_context &Ctx) -> decltype(Ctx.begin())
    {
        return Ctx.end();
    }

    template <typename FormatContext> auto format(const Sample &S, FormatContext &Ctx) const -> decltype(Ctx.out())
    {
        return fmt::format_to(Ctx.out(), "[{}]={}", S.TimePoint, static_cast<double>(S.Value) / 1000);
    }
};

template <> struct fmt::formatter<Dimension> {
    constexpr auto parse(format_parse_context &Ctx) -> decltype(Ctx.begin())
    {
        return Ctx.end();
    }

    template <typename FormatContext> auto format(const Dimension &D, FormatContext &Ctx) const -> decltype(Ctx.out())
    {
        return fmt::format_to(Ctx.out(), "{}", D.CB);
    }
};

template <> struct fmt::formatter<Chart> {
    constexpr auto parse(format_parse_context &Ctx) -> decltype(Ctx.begin())
    {
        return Ctx.end();
    }

    template <typename FormatContext> auto format(const Chart &C, FormatContext &Ctx) const -> decltype(Ctx.out())
    {
        return fmt::format_to(Ctx.out(), "{}: {}", C.name(), C.dimensions());
    }
};

#endif /* ND_OTEL_CHART_H */
