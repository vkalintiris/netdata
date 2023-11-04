#include "../rdb-private.h"
#include "libnetdata/log/log.h"
#include <gtest/gtest.h>

using namespace rdb;

TEST(Intervals, BitSplitter)
{
    {
        BitSplitter<uint16_t, 0> BS(0xDEAD);
        EXPECT_EQ(BS.getUpper(), 0xDEAD);
        EXPECT_EQ(BS.getLower(), 0x0);

        {
            BitSplitter<uint16_t, 0> BS;   
            BS.setUpper(0xDEAD);
            BS.setLower(0);
            EXPECT_EQ(BS.getUpper(), 0xDEAD);
            EXPECT_EQ(BS.getLower(), 0x0);
        }
    }

    {
        BitSplitter<uint16_t, 1> BS(0xDEAD);
        EXPECT_EQ(BS.getUpper(), 0xDEAD >> 1);
        EXPECT_EQ(BS.getLower(), 0x1);

        {
            BitSplitter<uint16_t, 1> BS;   
            BS.setUpper(0xDEAD);
            BS.setLower(1);
            EXPECT_EQ(BS.getUpper(), 0xDEAD & 0x7FFF);
            EXPECT_EQ(BS.getLower(), 0x1);
        }
    }

    {
        BitSplitter<uint16_t, 2> BS(0xDEAD);
        EXPECT_EQ(BS.getUpper(), 0xDEAD >> 2);
        EXPECT_EQ(BS.getLower(), 0x1);
    }

    {
        BitSplitter<uint16_t, 4> BS(0xDEAD);
        EXPECT_EQ(BS.getUpper(), 0xDEAD >> 4);
        EXPECT_EQ(BS.getLower(), 0xD);
    }

    {
        BitSplitter<uint16_t, 6> BS(0xDEAD);
        EXPECT_EQ(BS.getUpper(), 0xDEAD >> 6);
        EXPECT_EQ(BS.getLower(), 0b101101);
    }

    {
        BitSplitter<uint16_t, 8> BS(0xDEAD);
        EXPECT_EQ(BS.getUpper(), 0xDE);
        EXPECT_EQ(BS.getLower(), 0xAD);
    }

    {
        BitSplitter<uint16_t, 15> BS(0xFFFF);
        EXPECT_EQ(BS.getUpper(), 0x1);
        EXPECT_EQ(BS.getLower(), 0x7FFF);

        {
            BitSplitter<uint16_t, 15> BS;   
            BS.setUpper(0xDEAD);
            BS.setLower(0xDEAD);
            EXPECT_EQ(BS.getUpper(), 0xDEAD & 0x1);
            EXPECT_EQ(BS.getLower(), 0x5EAD);
        }
    }

    {
        BitSplitter<uint32_t, 0> BS(0xDEADBEEF);
        EXPECT_EQ(BS.getUpper(), 0xDEADBEEF);
        EXPECT_EQ(BS.getLower(), 0x0);
    }

    {
        BitSplitter<uint32_t, 1> BS(0xDEADBEEF);
        EXPECT_EQ(BS.getUpper(), 0xDEADBEEF >> 1);
        EXPECT_EQ(BS.getLower(), 0x1);
    }
    
    {
        BitSplitter<uint32_t, 28> BS(0xDEADBEEF);
        EXPECT_EQ(BS.getUpper(), 0xD);
        EXPECT_EQ(BS.getLower(), 0xEADBEEF);
    }
}

TEST(Intervals, CompressedSlots_TierSlots_1024)
{
    for (size_t TierSlots = 1024, Idx = 0; Idx != 4 * TierSlots; Idx++)
    {
        CompressedSlots CS(Idx);
        EXPECT_EQ(CS.slots(), Idx);
        EXPECT_EQ(CS.isPageCounter(), (Idx % TierSlots) == 0);

        auto BS = CS.bitSplitter();
        if (CS.isPageCounter())
        {
            EXPECT_EQ(BS.getUpper(), 0x1);
            EXPECT_EQ(BS.getLower(), Idx / TierSlots);
        }
        else
        {
            EXPECT_EQ(BS.getUpper(), 0x0);
        }
    }

    {
        CompressedSlots LHS(512);
        CompressedSlots RHS(512);
        EXPECT_FALSE(LHS.merge(RHS));
    }

    {
        CompressedSlots LHS(0);

        for (size_t Page = 0; Page != 0x7FFF; Page++)
        {
            CompressedSlots RHS(LHS.PageSlots);
            EXPECT_TRUE(LHS.merge(RHS));
            EXPECT_EQ(LHS.slots(), (Page + 1) * LHS.PageSlots);

            auto BS = LHS.bitSplitter();
            EXPECT_EQ(BS.getUpper(), 0x1);
            EXPECT_EQ(BS.getLower(), Page + 1);
        }

        CompressedSlots RHS(LHS.PageSlots);
        EXPECT_FALSE(LHS.merge(RHS));
        EXPECT_EQ(LHS.slots(), 0x7FFF * LHS.PageSlots);
    }

    {
        CompressedSlots LHS(0);
        CompressedSlots RHS(0);

        EXPECT_EQ(LHS.PageSlots, RHS.PageSlots);

        size_t NumPagesLHS = 100;
        for (size_t Page = 0; Page != NumPagesLHS; Page++)
        {
            CompressedSlots Tmp(LHS.PageSlots);
            EXPECT_TRUE(LHS.merge(Tmp));
        }
        EXPECT_EQ(LHS.slots(), NumPagesLHS * LHS.PageSlots);

        size_t NumPagesRHS = 500;
        for (size_t Page = 0; Page != NumPagesRHS; Page++)
        {
            CompressedSlots Tmp(RHS.PageSlots);
            EXPECT_TRUE(RHS.merge(Tmp));
        }
        EXPECT_EQ(RHS.slots(), NumPagesRHS * RHS.PageSlots);

        EXPECT_TRUE(LHS.merge(RHS));
        EXPECT_EQ(LHS.slots(), (NumPagesLHS + NumPagesRHS) * LHS.PageSlots);

        CompressedSlots NonPageCS(111);
        EXPECT_FALSE(LHS.merge(NonPageCS));
    }

    {
        CompressedSlots CS(100 * CompressedSlots<>::PageSlots);
        EXPECT_EQ(CS.slots(), 100 * CompressedSlots<>::PageSlots);
    }
}

TEST(Intervals, CompressedSlots_TierSlots_641)
{
    constexpr size_t TierSlots = 641;

    for (size_t Idx = 0; Idx != 4 * TierSlots; Idx++)
    {
        CompressedSlots<TierSlots> CS(Idx);
        EXPECT_EQ(CS.slots(), Idx);
        EXPECT_EQ(CS.isPageCounter(), (Idx % TierSlots) == 0);

        auto BS = CS.bitSplitter();
        if (CS.isPageCounter())
        {
            EXPECT_EQ(BS.getUpper(), 0x1);
            EXPECT_EQ(BS.getLower(), Idx / TierSlots);
        }
        else
        {
            EXPECT_EQ(BS.getUpper(), 0x0);
        }
    }

    {
        CompressedSlots<TierSlots> LHS(512);
        CompressedSlots<TierSlots> RHS(512);
        EXPECT_FALSE(LHS.merge(RHS));
    }

    {
        CompressedSlots<TierSlots> LHS(0);

        for (size_t Page = 0; Page != 0x7FFF; Page++)
        {
            CompressedSlots<TierSlots> RHS(LHS.PageSlots);
            EXPECT_TRUE(LHS.merge(RHS));
            EXPECT_EQ(LHS.slots(), (Page + 1) * LHS.PageSlots);

            auto BS = LHS.bitSplitter();
            EXPECT_EQ(BS.getUpper(), 0x1);
            EXPECT_EQ(BS.getLower(), Page + 1);
        }

        CompressedSlots<TierSlots> RHS(LHS.PageSlots);
        EXPECT_FALSE(LHS.merge(RHS));
        EXPECT_EQ(LHS.slots(), 0x7FFF * LHS.PageSlots);
    }

    {
        CompressedSlots<TierSlots> LHS(0);
        CompressedSlots<TierSlots> RHS(0);

        EXPECT_EQ(LHS.PageSlots, RHS.PageSlots);

        size_t NumPagesLHS = 100;
        for (size_t Page = 0; Page != NumPagesLHS; Page++)
        {
            CompressedSlots<TierSlots> Tmp(LHS.PageSlots);
            EXPECT_TRUE(LHS.merge(Tmp));
        }
        EXPECT_EQ(LHS.slots(), NumPagesLHS * LHS.PageSlots);

        size_t NumPagesRHS = 500;
        for (size_t Page = 0; Page != NumPagesRHS; Page++)
        {
            CompressedSlots<TierSlots> Tmp(RHS.PageSlots);
            EXPECT_TRUE(RHS.merge(Tmp));
        }
        EXPECT_EQ(RHS.slots(), NumPagesRHS * RHS.PageSlots);

        EXPECT_TRUE(LHS.merge(RHS));
        EXPECT_EQ(LHS.slots(), (NumPagesLHS + NumPagesRHS) * LHS.PageSlots);

        CompressedSlots<TierSlots> NonPageCS(111);
        EXPECT_FALSE(LHS.merge(NonPageCS));
    }

    {
        CompressedSlots<TierSlots> CS(100 * TierSlots);
        EXPECT_EQ(CS.slots(), 100 * TierSlots);
    }
}

TEST(Intervals, CompressedDuration)
{
    constexpr size_t TierSlots = 1024;

    {
        CompressedDuration CD(100 * CompressedDuration<>::PageSlots, 5);
        EXPECT_EQ(CD.slots(), 100 * CD.PageSlots);
        EXPECT_EQ(CD.duration(), CD.slots() * 5);
    }

    {
        CompressedDuration LHS(100 * CompressedDuration<>::PageSlots, 5);
        CompressedDuration RHS(200 * CompressedDuration<>::PageSlots, 5);

        EXPECT_TRUE(LHS.merge(RHS));
        
        EXPECT_EQ(LHS.slots(), 300 * LHS.PageSlots);
        EXPECT_EQ(LHS.duration(), LHS.slots() * 5);
    }

    {
        CompressedDuration LHS(100 * CompressedDuration<>::PageSlots, 5);
        CompressedDuration RHS(200 * CompressedDuration<>::PageSlots, 15);

        EXPECT_FALSE(LHS.merge(RHS));
    }

    {
        CompressedDuration LHS(100 * CompressedDuration<>::PageSlots, 5);
        CompressedDuration RHS(51, 15);

        EXPECT_FALSE(LHS.merge(RHS));
    }

    {
        CompressedDuration LHS(51, 15);
        CompressedDuration RHS(51, 15);

        EXPECT_FALSE(LHS.merge(RHS));
    }
}

TEST(Intervals, CompressedInterval)
{
    constexpr size_t TierSlots = 1024;

    {
        CompressedInterval CI(333, 66, 3600);
        EXPECT_EQ(CI.after(), 333);
        EXPECT_EQ(CI.before(), 333 + 66 * 3600);
    }

    {
        CompressedInterval CI(100, 0, 1);
        EXPECT_EQ(CI.after(), 100);
        EXPECT_EQ(CI.before(), 100);
    }

    {
        CompressedInterval CI(100, 1, 5);
        EXPECT_EQ(CI.after(), 100);
        EXPECT_EQ(CI.before(), 105);
    }

    {
        CompressedInterval CI(333, 100 * TierSlots, 5);
        EXPECT_EQ(CI.after(), 333);
        EXPECT_EQ(CI.before(), 333 + (100 * TierSlots) * 5);
    }

    {
        CompressedInterval LHS(333, 100 * TierSlots, 5);
        CompressedInterval RHS(333, 100 * TierSlots, 5);

        EXPECT_FALSE(LHS.merge(RHS));
    }

    {
        CompressedInterval LHS(333, 100 * TierSlots, 5);
        CompressedInterval RHS(333, 100 * TierSlots, 15);

        EXPECT_FALSE(LHS.merge(RHS));
    }
    
    {
        CompressedInterval LHS(100, 50, 10);
        CompressedInterval RHS(LHS.before(), 50, 10);

        EXPECT_FALSE(LHS.merge(RHS));
    }

    {
        CompressedInterval LHS(100, 100 * TierSlots, 10);
        CompressedInterval RHS(LHS.before(), 100 * TierSlots, 10);

        EXPECT_EQ(LHS.after(), 100);
        EXPECT_EQ(RHS.before(), LHS.before() + (100 * TierSlots) * 10);

        EXPECT_TRUE(LHS.merge(RHS));
        EXPECT_EQ(LHS.after(), 100);
        EXPECT_EQ(LHS.before(), RHS.before());
    }
}

TEST(Intervals, IntervalsManager)
{
    IntervalManager<1024> IM;

    fflush(stdout);
    fflush(stderr);

    IM.addInterval(5 * 1024, IM.PageSlots, 1);
    IM.printMergedIntervals();

    IM.addInterval(4 * 1024, IM.PageSlots, 1);
    IM.printMergedIntervals();

    EXPECT_TRUE(IM.verify());
    
    // IM.addInterval(2 * 1024, IM.PageSlots, 1);
    // printf("[2048, 3072) Intervals:");
    // IM.printMergedIntervals();

    // IM.addInterval(3 * 1024, IM.PageSlots, 1);
    // printf("Should have just 1 interval now:");
    // IM.printMergedIntervals();
}

int rdb_intervals_tests_main(int argc, char *argv[])
{
    // skip the `-W intervals-tests` args
    for (int i = 2; i < argc; ++i)
    {
        argv[i - 1] = argv[i];
    }
    argc -= 2;

    ::testing::InitGoogleTest(&argc, argv);
    ::testing::GTEST_FLAG(filter) = "Intervals.IntervalsManager";

    int rc = RUN_ALL_TESTS();
    google::protobuf::ShutdownProtobufLibrary();
    return rc;
}
