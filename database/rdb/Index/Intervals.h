#include <iostream>
#include <limits>
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

    [[nodiscard]] inline uint32_t duration() const
    {
        return UpdateEvery * slots();
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

    [[nodiscard]] inline bool merge(const CompressedInterval &Other)
    {
        if (before() == Other.after())
            return CD.merge(Other.CD);

        return false;
    }

private:
    uint32_t After;
    CompressedDuration<TierSlots> CD;
};

template<size_t TierSlots>
class IntervalManager
{
public:
    void readIntervalsFromStdin()
    {
        uint32_t After, Before, Slots;
        while (std::cin >> After >> Before >> Slots)
        {
            CompressedInterval<1024> I(After, Before, Slots);
            addInterval(I);
        }
    }

    inline void addInterval(const CompressedInterval<TierSlots>& NI)
    {
        if (Intervals.back().merge(NI))
            return;
        
        Intervals.push_back(NI);
    }

    void printMergedIntervals() const
    {
        // for (const Interval<TierSlots> &I : Intervals)
        // {
        //     std::cout << I.After << " " << I.UpdateEvery << " " << I.Slots << std::endl;
        // }
    }

private:
    absl::InlinedVector<CompressedInterval<TierSlots>, 2> Intervals;
};

} // namespace rdb
