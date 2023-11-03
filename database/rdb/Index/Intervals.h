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

    BitSplitter(T Value) : Value(Value)
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

    CompressedSlots(uint32_t Slots) : BS(Slots)
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
    CompressedDuration() = default;

    CompressedDuration(uint16_t UpdateEvery, uint16_t Slots)
        : BS(((uint32_t) UpdateEvery << 16) | Slots)
    { }

    [[nodiscard]] inline CompressedSlots<TierSlots> compressedSlots() const
    {
        return CompressedSlots<TierSlots>(BS.getLower());
    }

    [[nodiscard]] inline uint32_t updateEvery() const
    {
        return BS.getUpper();    
    }

    [[nodiscard]] inline uint32_t slots() const
    {
        CompressedSlots<TierSlots> CS = slots();
        return CS.slots();
    }

    [[nodiscard]] inline uint32_t duration() const
    {
        return updateEvery() * slots();
    }

    [[nodiscard]] inline bool Merge(const CompressedDuration<TierSlots> &Other)
    {
        if (updateEvery() == Other.updateEvery())
        {
            return compressedSlots().merge(Other.compressedSlots());
        }
        
        return false;
    }

private:
    BitSplitter<uint32_t, 16> BS;    
};

template<size_t TierSlots>
class Interval
{
public:
    Interval(uint32_t After, uint16_t UpdateEvery, uint16_t Slots) :
        After(After), CD(UpdateEvery, Slots)
    { }

    [[nodiscard]] inline uint32_t after() const
    {
        return After;
    }
    
    [[nodiscard]] inline uint32_t before() const
    {
        return after() + CD.duration();
    }

    [[nodiscard]] inline bool merge(const Interval &Other)
    {
        if (before() == Other.after())
            return CD.Merge(Other.CD);

        return false;
    }

    inline void mergeWith(const Interval &Other)
    {
        // UpdateEvery = Other.UpdateEvery;
    }

public:
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
            Interval<1024> I(After, Before, Slots);
            addInterval(I);
        }
    }

    inline void addInterval(const Interval<TierSlots>& NI)
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
    absl::InlinedVector<Interval<TierSlots>, 2> Intervals;
};

} // namespace rdb
