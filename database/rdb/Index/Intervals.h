#include <iostream>
#include <limits>
#include <optional>
#include <type_traits>
#include <cassert>
#include <cstdint>
#include <climits>
#include "absl/container/inlined_vector.h"

namespace rdb
{

template<typename T, size_t N>
class BitSplitter
{
public:
    BitSplitter() = default;

    explicit BitSplitter(T Value) : Value(Value)
    {
        static_assert(std::is_integral<T>::value,
                      "T must be an integral type");
        static_assert(N < (sizeof(T) * CHAR_BIT),
                      "N must be less than or equal to the number of bits in T");
    }

    BitSplitter(T UV, T LV)
    {
        setUpper(UV);
        setLower(LV);
    }

    [[nodiscard]] inline T getLower() const
    {
        return Value & ((1 << N) - 1);
    }

    [[nodiscard]] T getUpper() const
    {
        return (Value >> N);
    }

    void setLower(T LV)
    {
        Value = (Value & ~((1 << N) - 1)) | (LV & ((1 << N) - 1));
    }

    void setUpper(T UV)
    {
        Value = (UV << N) | (Value & ((1 << N) - 1));
    }

private:
    T Value;
};

template<size_t TierSlots = 1024>
class CompressedSlots
{
public:
    static constexpr size_t PageSlots = TierSlots;

public:
    CompressedSlots() = default;

    explicit CompressedSlots(uint32_t Slots) : BS(Slots)
    {
        static_assert(sizeof(CompressedSlots<>) <= 2,
                      "Size of class exceeds 2 bytes threshold.");

        if ((Slots % TierSlots) == 0)
        {
            BS.setUpper(1);
            BS.setLower(Slots / TierSlots);
        }
        else
        {
            assert(Slots < TierSlots);
            BS.setUpper(0);
            BS.setLower(Slots);
        }
    }

    [[nodiscard]] inline bool merge(const CompressedSlots<TierSlots> &Other)
    {
        if (isPageCounter() && Other.isPageCounter())
        {
            // We need to check that:
            //    - the result can be stored in a uint16_t, and
            //    - the sum does not have the MSB set

            uint32_t Sum = pages() + Other.pages();
            bool canMerge = (Sum < std::numeric_limits<uint16_t>::max()) && ((Sum & 0x8000) == 0);

            if (canMerge)
            {
                BS.setLower(Sum);
                BS.setUpper(1);
                return true;
            }
        }

        return false;
    }

    [[nodiscard]] inline bool isPageCounter() const
    {
        return BS.getUpper() == 1;
    }

    [[nodiscard]] inline bool isSlotCounter() const
    {
        return !isPageCounter();
    }

    [[nodiscard]] inline uint32_t slots() const
    {
        if (!isPageCounter())
            return BS.getLower();

        return BS.getLower() * PageSlots;
    }

    [[nodiscard]] inline BitSplitter<uint16_t, 15> bitSplitter() const
    {
        return BS;
    }

private:
    [[nodiscard]] inline uint32_t pages() const
    {
        assert(isPageCounter());
        return BS.getLower();
    }

private:
    BitSplitter<uint16_t, 15> BS;
};

template<size_t TierSlots = 1024>
class CompressedDuration
{
public:
    static constexpr size_t PageSlots = TierSlots;

public:
    CompressedDuration() = default;

    explicit CompressedDuration(uint32_t Slots, uint16_t UpdateEvery)
        : CS(Slots), UpdateEvery(UpdateEvery)
    {
        static_assert(sizeof(CompressedDuration<>) <= 4,
                      "Size of class exceeds 4 bytes threshold.");
    } 
        
    [[nodiscard]] inline uint32_t slots() const
    {
        return CS.slots();
    }

    [[nodiscard]] inline uint32_t updateEvery() const
    {
        return UpdateEvery;    
    }

    [[nodiscard]] inline uint32_t duration() const
    {
        return UpdateEvery * slots();
    }

    [[nodiscard]] inline bool isPageDuration() const
    {
        return CS.isPageCounter();
    }

    [[nodiscard]] inline bool merge(const CompressedDuration<TierSlots> &Other)
    {
        if (UpdateEvery == Other.UpdateEvery)
        {
            return CS.merge(Other.CS);
        }
        
        return false;
    }

private:
    CompressedSlots<TierSlots> CS;
    uint16_t UpdateEvery;
};

template<size_t TierSlots = 1024>
class CompressedInterval
{
public:
    static constexpr size_t PageSlots = TierSlots;

public:
    CompressedInterval(uint32_t After, uint32_t Slots, uint16_t UpdateEvery)
        : After(After), CD(Slots, UpdateEvery)
    {
        static_assert(sizeof(CompressedInterval) == 8,
                      "Size of class exceeds 8 bytes threshold.");
    }

    [[nodiscard]] inline uint32_t after() const
    {
        return After;
    }
    
    [[nodiscard]] inline uint32_t before() const
    {
        return after() + CD.duration();
    }

    [[nodiscard]] inline uint32_t updateEvery() const
    {
        return CD.UpdateEvery;       
    }

    [[nodiscard]] inline uint32_t pageDuration() const
    {
        return TierSlots * updateEvery();
    }

    [[nodiscard]] inline uint32_t numPages() const
    {
        if (!CD.isPageDuration())
            return 1;

        return (before() - after()) / TierSlots;
    }

    [[nodiscard]] inline bool merge(const CompressedInterval &Other)
    {
        if (before() == Other.after())
            return CD.merge(Other.CD);

        return false;
    }

    [[nodiscard]] inline std::pair<std::optional<CompressedInterval>,
                                   std::optional<CompressedInterval>>
    drop(uint32_t PIT) const
    {
        if (!CD.isPageDuration())
        {
            if (PIT == after())
                return {};

            return { *this, std::nullopt };
        }

        if (PIT < after())
        {
            return { *this, std::nullopt };
        }
        else if (PIT > before())
        {
            return { *this, std::nullopt };
        }

        const uint32_t PageDuration = TierSlots * CD.updateEvery();

        // PIT should be already aligned. However, we want release builds to
        // handle any possible errors.
        assert((PIT % PageDuration) == 0);
        PIT -= (PIT % PageDuration);

        std::pair<std::optional<CompressedInterval>,
                  std::optional<CompressedInterval>> P;

        if (After < PIT)
        {
            uint32_t Slots = (PIT - After) / CD.updateEvery();
            P.first = CompressedInterval(After, Slots, CD.updateEvery());
        }

        if (PIT < (before() - PageDuration))
        {
            uint32_t Slots = (before() - (PIT + PageDuration)) / CD.updateEvery();
            P.second = CompressedInterval(PIT + PageDuration, Slots, CD.updateEvery());
        }

        return P;
    }

private:
    uint32_t After;
    CompressedDuration<TierSlots> CD;
};

template<size_t TierSlots>
class IntervalManager
{
    using CompInt = CompressedInterval<TierSlots>;
    using Iterator = typename absl::InlinedVector<CompInt, 2>::iterator;

public:
    static constexpr size_t PageSlots = TierSlots;

    [[nodiscard]] inline std::optional<uint32_t> after() const
    {
        if (!Intervals.size())
            return std::nullopt;

        return Intervals[0].after();
    }

    [[nodiscard]] inline std::optional<uint32_t> before() const
    {
        if (!Intervals.size())
            return std::nullopt;

        return Intervals.back().before();
    }

    inline bool addInterval(uint32_t After, uint32_t Slots, uint16_t UpdateEvery)
    {
        CompInt NCI(After, Slots, UpdateEvery);

        auto cmpFunc = [](const CompInt &LHS, const CompInt &RHS)
        {
            return LHS.after() < RHS.after();
        };
        auto It = std::lower_bound(Intervals.begin(), Intervals.end(), NCI, cmpFunc);

        auto validIntervals = [](const CompInt &LHS, const CompInt &RHS)
        {
            // NOTE: this check runs only when we can't merge the intervals.
            return LHS.before() < RHS.after();
        };

        if (It == Intervals.end())
        {
            if (!Intervals.size())
            {
                Intervals.push_back(NCI);
                return false;
            }
            else if (Intervals.back().merge(NCI))
            {
                return true;
            }
            else
            {
                if (validIntervals(Intervals.back(), NCI))
                    Intervals.push_back(NCI);
                return false;
            }
        }
        else
        {
            // Try to merge the RHS into NCI
            if (NCI.merge(*It))
            {
                if (It == Intervals.begin())
                {
                    *It = NCI;
                    return true;
                }
                else
                {
                    // Go to the LHS and try to merge the updated NCI
                    --It;
                    if (It->merge(NCI))
                    {
                        // 1. we managed to merge RHS into NCI and NCI into LHS
                        // 2. we can remove RHS from the vector
                        Intervals.erase(++It);
                        return true;
                    }
                    else
                    {
                        // 1. we managed to merge RHS into NCI.
                        // 2. update RHS with NCI.
                        ++It;
                        *It = NCI;
                        return true;
                    }
                }
            }
            else
            {
                if (It == Intervals.begin())
                {
                    // prepend NCI to the vector
                    if (validIntervals(NCI, *It))
                        Intervals.insert(It, NCI);
                    return false;
                }
                else
                {
                    // Go to the LHS and try to merge NCI
                    --It;
                    if (It->merge(NCI))
                    {
                        // Nothing else to do.
                        return true;
                    }
                    else
                    {
                        // We could not merge NCI into LHS. Add a new element after LHS.
                        ++It;
                        if (validIntervals(NCI, *It))
                            Intervals.insert(It, NCI);
                        return false;
                    }
                }
            }
        }

        fatal("Unhandled case in %s", __FUNCTION__);
    }

    [[nodiscard]] bool verify() const
    {
        if (Intervals.size() == 0)
            return true;

        if (Intervals.size() == 1)
            return Intervals[0].after() < Intervals[0].before();

        // Check that the intervals are in sorted order, they don't overlap
        // and they can't be merged.
        for (size_t Idx = 0; Idx != Intervals.size() - 1; Idx++)
        {
            CompInt CI = Intervals[Idx];
            const CompInt &RHS = Intervals[Idx + 1];

            bool Ok = (CI.after() < CI.before()) &&
                      (CI.after() <= RHS.after()) &&
                      (CI.before() <= RHS.after()) &&
                      !CI.merge(RHS);

            if (!Ok)
            {
                return false;
            }
        }

        return true;
    }

    [[nodiscard]] inline size_t size() const
    {
        return Intervals.size();   
    }

    void printMergedIntervals() const
    {
        for (size_t Idx = 0; Idx != Intervals.size(); Idx++)
        {
            printf("\tInterval[%zu/%zu]: [%u, %u)\n", Idx + 1, Intervals.size(),
                   Intervals[Idx].after(), Intervals[Idx].before());
        }

        printf("\n");
    }

private:
    absl::InlinedVector<CompressedInterval<TierSlots>, 2> Intervals;
};

} // namespace rdb
