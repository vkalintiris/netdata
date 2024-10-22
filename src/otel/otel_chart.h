// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ND_OTEL_CHART_H
#define ND_OTEL_CHART_H

#include "otel_circular_buffer.h"
#include "otel_iterator.h"
#include "otel_utils.h"
#include "otel_hash.h"
#include <limits>

struct Sample {
    uint64_t Value;
    uint32_t TimePoint;

    bool operator<(const Sample &RHS) const
    {
        return TimePoint < RHS.TimePoint;
    }
};

struct Dimension {
    std::string Name;
    SortedContainer<Sample> Samples;

    bool empty() const
    {
        return Samples.empty();
    }

    size_t numSamples() const
    {
        return Samples.size();
    }

    void pushSample(const Sample &S)
    {
        Samples.push(S);
    }

    Sample popSample()
    {
        assert(!Samples.empty() && "expected non-empty samples");
        return Samples.pop();
    }

    uint32_t startTime() const
    {
        const Sample &S = Samples.peek();
        return S.TimePoint;
    }

    uint32_t updateEvery() const
    {
        assert(std::is_sorted(Samples.begin(), Samples.end()) && "expected sorted samples");
        uint32_t UE = std::numeric_limits<uint32_t>::max();

        for (size_t Idx = 1; Idx < Samples.size(); Idx++) {
            uint32_t Delta = Samples[Idx].TimePoint - Samples[Idx - 1].TimePoint;
            assert(Delta != 0 && "expected unique timestamps");

            UE = std::min(UE, Delta);
        }

        return UE;
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

class DimensionContainer {
public:
    void add(const std::string &Name, const Sample &S)
    {
        auto [It, Emplaced] = Dimensions.try_emplace(Name);
        Dimension &D = It->second;

        if (Emplaced) {
            D.Name = Name;
            Committed = false;
        }

        D.pushSample(S);
    }

    void
    process(size_t RampUpThreshold, size_t GapThreshold, absl::InlinedVector<std::pair<std::string, Sample>, 4> &IV)
    {
        assert(RampUpThreshold >= 2);

        bool Processed = false;

        if (UpdateEvery.has_value()) {
            Processed = processFastPath(IV);
        }

        if (!Processed) {
            processSlowPath(RampUpThreshold, GapThreshold, IV);
        }
    }

    const absl::flat_hash_map<std::string, Dimension> &dimensions() const
    {
        return Dimensions;
    }

    bool isCommitted() const
    {
        return Committed;
    }

    void setCommitted(bool committed)
    {
        Committed = committed;
    }

    uint64_t startTime() const
    {
        return minStartTimeInDimensions();
    }

    uint64_t updateEvery() const
    {
        return minUpdateEveryInDimensions();
    }

private:
    bool processFastPath(absl::InlinedVector<std::pair<std::string, Sample>, 4> &IV)
    {
        assert(
            UpdateEvery.has_value() && UpdateEvery.value() &&
            UpdateEvery.value() != std::numeric_limits<uint32_t>::max());
        assert(
            LastCollectedTime.has_value() && LastCollectedTime.value() &&
            LastCollectedTime.value() != std::numeric_limits<uint32_t>::max());

        bool Ok = false;
        while (true) {
            for (auto &[Name, D] : Dimensions) {
                if (D.empty()) {
                    return Ok;
                }

                if (D.compareCollectionTime(LastCollectedTime.value(), UpdateEvery.value())) {
                    return Ok;
                }

                IV.emplace_back(Name, D.popSample());

                IV.clear();
                Ok = true;
            }
        };
    }

    void processSlowPath(
        size_t RampUpThreshold,
        size_t GapThreshold,
        absl::InlinedVector<std::pair<std::string, Sample>, 4> &IV)
    {
        UNUSED(IV);

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
        assert(!Dimensions.empty());

        uint32_t UE = std::numeric_limits<uint32_t>::max();
        for (const auto &[Name, Dim] : Dimensions) {
            UE = std::min(UE, Dim.updateEvery());
        }

        return UE;
    }

    uint32_t minStartTimeInDimensions() const
    {
        assert(!Dimensions.empty());

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
    absl::flat_hash_map<std::string, Dimension> Dimensions;
    std::optional<uint32_t> UpdateEvery;
    std::optional<uint32_t> LastCollectedTime;
    bool Committed = false;
};

class Chart {
public:
    void initialize(BlakeId &Id, const pb::ResourceMetrics *RM, const pb::ScopeMetrics *SM, const pb::Metric *M)
    {
        UNUSED(RM);
        UNUSED(SM);

        this->BID = Id;
        Name = M->name();
    }

    void initialize(BlakeId &Id, const std::string &Name)
    {
        this->BID = Id;
        this->Name = Name;
    }

    void add(const OtelElement &OE)
    {
        /* TODO */
        UNUSED(OE);
    }

    void process(size_t RampUpThreshold, size_t GapThreshold)
    {
        absl::InlinedVector<std::pair<std::string, Sample>, 4> IV;
        DimContainer.process(RampUpThreshold, GapThreshold, IV);
    }

    const std::string &name() const
    {
        return Name;
    }

    const absl::flat_hash_map<std::string, Dimension> &dimensions() const
    {
        return DimContainer.dimensions();
    }

private:
    BlakeId BID;
    std::string Name;
    DimensionContainer DimContainer;
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
        return fmt::format_to(Ctx.out(), "{}", D.Samples);
    }
};

template <> struct fmt::formatter<DimensionContainer> {
    constexpr auto parse(format_parse_context &Ctx) -> decltype(Ctx.begin())
    {
        return Ctx.end();
    }

    template <typename FormatContext>
    auto format(const DimensionContainer &DC, FormatContext &Ctx) const -> decltype(Ctx.out())
    {
        return fmt::format_to(Ctx.out(), "{}", DC.dimensions());
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
