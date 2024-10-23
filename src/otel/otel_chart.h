// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ND_OTEL_CHART_H
#define ND_OTEL_CHART_H

#include "absl/types/optional.h"
#include "otel_circular_buffer.h"
#include "otel_iterator.h"
#include "otel_utils.h"
#include "otel_hash.h"
#include <limits>

// Holds the value of a dimension at specific point in time.
struct Sample {
    // The 64-bit value we collected at this specific time point.
    uint64_t Value;

    // The time point at which we collected the value of the sample.
    uint32_t TimePoint;

    bool operator<(const Sample &RHS) const
    {
        return TimePoint < RHS.TimePoint;
    }
};

// Maintains a vector of sorted samples along with the name of the dimension.
struct Dimension {
    // The name of the dimension.
    std::string Name;

    // The samples of the dimension sorted by their time point.
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

    // The start time of the dimension is the time point of the first
    // sample in the sorted container
    uint32_t startTime() const
    {
        const Sample &S = Samples.peek();
        return S.TimePoint;
    }

    // A dimension should be collected at regular intervals. It is possible
    // to ingest OTEL data out-of-order (with respect to the collection time
    // point of the samples), whenever we push/pop samples to a dimension
    // the estimated collection interval might change.
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

    // While a dimension has its own colection interval (based on the time
    // points of its samples), a chart groups multiple dimensions together
    // and a separate logic is used to calculate a chart's collection
    // frequency.
    // We use this function to figure out if the start time of a dimension
    // is on the left-side (-1), in-side (0), right-side (+1) of
    // the expected collection interval.
    int compareCollectionTime(uint32_t LCT, uint32_t UpdateEvery) const
    {
        double StartTP = LCT + UpdateEvery - (UpdateEvery / 2.0);
        double EndTP = LCT + UpdateEvery + (UpdateEvery / 2.0);

        if (startTime() < StartTP) {
            return -1;
        } else if (startTime() >= EndTP) {
            return 1;
        } else {
            return 0;
        }
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

    absl::optional<uint64_t> startTime() const
    {
        return minStartTimeInDimensions();
    }

    absl::optional<uint32_t> updateEvery() const
    {
        return minUpdateEveryInDimensions();
    }

    absl::optional<uint32_t> lastCollectedTime(uint32_t UE) const {
        absl::optional<uint32_t> StartTime = startTime();
        if (StartTime.has_value()) {
            return absl::nullopt;
        }

        return StartTime.value() - UE;
    }

    void
    process(size_t RampUpThreshold, size_t GapThreshold, absl::InlinedVector<std::pair<std::string, Sample>, 4> &IV)
    {
        assert(RampUpThreshold >= 2);

        bool Processed = false;

        // If we already have an update every, then we have a last collection
        // time, which means that it might be possible to process the 
        // oldest samples of all dimensions if they have the expected start
        // time.
        if (UpdateEvery.has_value()) {
            Processed = processFastPath(IV);
        }

        // If we didn't manage to process any samples, we follow the slow
        // path that recalculates the update every and the last collected
        // time
        if (!Processed) {
            processSlowPath(RampUpThreshold, GapThreshold, IV);
        }
    }

private:
    bool processFastPath(absl::InlinedVector<std::pair<std::string, Sample>, 4> &IV)
    {
        assert(UpdateEvery.has_value());
        assert(LastCollectedTime.has_value());

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
            // We don't have an update every and we have less than
            // `RampUpThreshold` number of samples across all dimensions.
            // This is a newly created chart and we are still buffering
            // incoming data.
            if (maxDataPointsInDimensions() >= RampUpThreshold) {
                UpdateEvery = updateEvery();
                assert(UpdateEvery.has_value());

                if (UpdateEvery.has_value()) {
                    LastCollectedTime = lastCollectedTime(UpdateEvery.value());
                    assert(LastCollectedTime.has_value());
                }

                return;
            }
        } else {
            // We have an update every and the last collected time. Use these
            // to drop any samples from our dimensions that we might have
            // received and belong in the past.

            (void) dropPastCollectionTimes();

            // Keep buffering if we don't have at least `GapThreshold` samples
            // across all the dimensions of the container.
            if (maxDataPointsInDimensions() >= GapThreshold) {
                UpdateEvery = updateEvery();
                assert(UpdateEvery.has_value());

                if (UpdateEvery.has_value()) {
                    LastCollectedTime = lastCollectedTime(UpdateEvery.value());
                    assert(LastCollectedTime.has_value());
                }
            }
        }
    }

    // Find the number of maximum samples across all dimensions.
    size_t maxDataPointsInDimensions() const
    {
        size_t N = std::numeric_limits<size_t>::min();
        for (const auto &[Name, Dim] : Dimensions) {
            N = std::max(N, Dim.numSamples());
        }
        return N;
    }

    // Find the minimum update interval of all dimensions.
    absl::optional<uint32_t> minUpdateEveryInDimensions() const
    {
        assert(!Dimensions.empty());

        uint32_t UE = std::numeric_limits<uint32_t>::max();
        for (const auto &[Name, Dim] : Dimensions) {
            UE = std::min(UE, Dim.updateEvery());
        }

        if (UE == std::numeric_limits<uint32_t>::max()) {
            return absl::nullopt;
        }

        return UE;
    }

    // Find the minimum start time of all dimensions.
    absl::optional<uint32_t> minStartTimeInDimensions() const
    {
        assert(!Dimensions.empty());

        uint32_t TP = std::numeric_limits<uint32_t>::max();
        for (const auto &[Name, Dim] : Dimensions) {
            TP = std::min(TP, Dim.startTime());
        }

        if (TP == std::numeric_limits<uint32_t>::max()) {
            return absl::nullopt;
        }

        return TP;
    }

    // Drop the samples of all dimensions that have a start time that is
    // older than the minimum time of the next collection interval.
    bool dropPastCollectionTimes()
    {
        assert(
            UpdateEvery.has_value() && UpdateEvery.value() && LastCollectedTime.has_value() &&
            LastCollectedTime.value());

        double UE = UpdateEvery.value();
        double LCT = LastCollectedTime.value();
        double StartTP = LCT + UE / 2.0;

        bool Dropped = false;
        for (auto &[Name, Dim] : Dimensions) {
            while (Dim.startTime() < StartTP) {
                Dim.popSample();
                Dropped = true;
            }
        }

        return Dropped;
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
