#include "database/rdb/Intervals.h"
#include "rdb-private.h"
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
    for (size_t TierSlots = 1024, Idx = 0; Idx != TierSlots; Idx++)
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

    for (size_t Idx = 0; Idx != TierSlots; Idx++)
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

    {
        const size_t PageSlots = 8;
        const size_t UpdateEvery = 3;
        const size_t PageDuration = PageSlots * UpdateEvery;
        const size_t NumPages = 10;

        const CompressedInterval<PageSlots> CI(3600, NumPages * PageSlots, UpdateEvery);

        EXPECT_EQ(CI.after(), 3600);
        EXPECT_EQ(CI.before(), CI.after() + PageDuration * NumPages);

        {
            // Test dropping always the 1st page
            
            CompressedInterval<PageSlots> TmpCI = CI;

            for (size_t Idx = 0; Idx != PageSlots; Idx++)
            {
                std::pair<std::optional<CompressedInterval<PageSlots>>,
                          std::optional<CompressedInterval<PageSlots>>> P = TmpCI.drop(TmpCI.after());

                EXPECT_FALSE(P.first.has_value());
                EXPECT_TRUE(P.second.has_value());

                TmpCI = P.second.value();
                EXPECT_EQ(TmpCI.after(), CI.after() + PageDuration * (Idx + 1));
                EXPECT_EQ(TmpCI.before(), CI.before());
            }
        }
        
        {
            // Test dropping always the last page

            CompressedInterval<PageSlots> TmpCI = CI;

            for (size_t Idx = 0; Idx != PageSlots; Idx++)
            {
                std::pair<std::optional<CompressedInterval<PageSlots>>,
                          std::optional<CompressedInterval<PageSlots>>> P =
                    TmpCI.drop(TmpCI.before() - PageDuration);

                EXPECT_TRUE(P.first.has_value());
                EXPECT_FALSE(P.second.has_value());

                TmpCI = P.first.value();
                EXPECT_EQ(TmpCI.after(), CI.after());
                EXPECT_EQ(TmpCI.before(), CI.before() - PageDuration * (Idx + 1));
            }
        }
        
        {
            // Test dropping the 2nd page

            CompressedInterval<PageSlots> TmpCI = CI;

            std::pair<std::optional<CompressedInterval<PageSlots>>,
                      std::optional<CompressedInterval<PageSlots>>> P =
                TmpCI.drop(CI.after() + PageDuration);

            EXPECT_TRUE(P.first.has_value());
            EXPECT_TRUE(P.second.has_value());

            const auto &LHS = P.first.value();
            EXPECT_EQ(LHS.after(), CI.after());
            EXPECT_EQ(LHS.before(), CI.after() + PageDuration);

            const auto &RHS = P.second.value();
            EXPECT_EQ(RHS.after(), LHS.after() + 2 * PageDuration);
            EXPECT_EQ(RHS.before(), CI.before());
        }

        {
            // Test dropping the 2nd-to-last page

            CompressedInterval<PageSlots> TmpCI = CI;

            std::pair<std::optional<CompressedInterval<PageSlots>>,
                      std::optional<CompressedInterval<PageSlots>>> P =
                TmpCI.drop(CI.before() - 2 * PageDuration);

            EXPECT_TRUE(P.first.has_value());
            EXPECT_TRUE(P.second.has_value());

            const auto &LHS = P.first.value();
            EXPECT_EQ(LHS.after(), CI.after());
            EXPECT_EQ(LHS.before(), CI.before() - 2 * PageDuration);

            const auto &RHS = P.second.value();
            EXPECT_EQ(RHS.after(), CI.before() - PageDuration);
            EXPECT_EQ(RHS.before(), CI.before());
        }

        {
            // Test dropping before after()

            CompressedInterval<PageSlots> TmpCI = CI;

            std::pair<std::optional<CompressedInterval<PageSlots>>,
                      std::optional<CompressedInterval<PageSlots>>> P =
                TmpCI.drop(TmpCI.after() - PageDuration);

            EXPECT_TRUE(P.first.has_value());
            EXPECT_FALSE(P.second.has_value());

            const auto &LHS = P.first.value();
            EXPECT_EQ(LHS.after(), TmpCI.after());
            EXPECT_EQ(LHS.before(), TmpCI.before());
        }

        {
            // Test dropping after before()

            CompressedInterval<PageSlots> TmpCI = CI;

            std::pair<std::optional<CompressedInterval<PageSlots>>,
                      std::optional<CompressedInterval<PageSlots>>> P =
                TmpCI.drop(TmpCI.before() + 1);

            EXPECT_TRUE(P.first.has_value());
            EXPECT_FALSE(P.second.has_value());

            const auto &LHS = P.first.value();
            EXPECT_EQ(LHS.after(), TmpCI.after());
            EXPECT_EQ(LHS.before(), TmpCI.before());
        }

        {
            // Test drop()-ing on a non-page interval
            const size_t PageSlots = 1024;
            size_t Slots = 100;
            size_t UpdateEvery = 3;

            const CompressedInterval<PageSlots> CI(3600, Slots, UpdateEvery);
            
            std::pair<std::optional<CompressedInterval<PageSlots>>,
                      std::optional<CompressedInterval<PageSlots>>> P;
            
            P = CI.drop(CI.before() + 1);
            EXPECT_TRUE(P.first.has_value());
            EXPECT_FALSE(P.second.has_value());

            EXPECT_EQ(P.first.value().after(), CI.after());
            EXPECT_EQ(P.first.value().before(), CI.before());

            P = CI.drop(CI.before() - 1);
            EXPECT_TRUE(P.first.has_value());
            EXPECT_FALSE(P.second.has_value());

            EXPECT_EQ(P.first.value().after(), CI.after());
            EXPECT_EQ(P.first.value().before(), CI.before());

            P = CI.drop(CI.after() + 1);
            EXPECT_TRUE(P.first.has_value());
            EXPECT_FALSE(P.second.has_value());

            EXPECT_EQ(P.first.value().after(), CI.after());
            EXPECT_EQ(P.first.value().before(), CI.before());

            P = CI.drop(CI.after() - 1);
            EXPECT_TRUE(P.first.has_value());
            EXPECT_FALSE(P.second.has_value());

            EXPECT_EQ(P.first.value().after(), CI.after());
            EXPECT_EQ(P.first.value().before(), CI.before());

            // For intervals that are not a multiple of a page's duration,
            // we only support droping the entire interval iff the PIT == after()
            P = CI.drop(CI.after());
            EXPECT_FALSE(P.first.has_value());
            EXPECT_FALSE(P.second.has_value());
        }
    }
}

TEST(Intervals, IntervalsManager)
{
    fflush(stdout);
    fflush(stderr);

    {
        IntervalManager<1024> IM;
        EXPECT_FALSE(IM.after().has_value());
        EXPECT_FALSE(IM.before().has_value());
        EXPECT_TRUE(IM.verify());

        size_t Epoch = 5555;
        size_t UpdateEvery = 3;
        size_t PageDuration = IM.PageSlots * UpdateEvery;

        for (size_t Idx = 0; Idx < 1024; Idx += 2) {
            bool Merged = IM.addInterval(Epoch + Idx * PageDuration, IM.PageSlots, UpdateEvery);
            EXPECT_TRUE(!Merged && IM.verify());
        }
        EXPECT_EQ(IM.size(), 512);

        {
            constexpr size_t SerializedSize = sizeof(uint32_t) + 512 * sizeof(CompressedInterval<IM.PageSlots>);
            std::array<char, SerializedSize> Buffer;

            auto OS = IM.serialize(Buffer);
            EXPECT_TRUE(OS);

            const Slice S(Buffer.data(), Buffer.size());
            std::optional<IntervalManager<IM.PageSlots>> OIM = IM.deserialize(S);
            EXPECT_TRUE(OIM.has_value());

            IM = OIM.value();
        }

        EXPECT_TRUE(IM.after().has_value());
        EXPECT_EQ(IM.after(), Epoch);
        EXPECT_TRUE(IM.before().has_value());
        EXPECT_EQ(IM.before(), Epoch + 1022 * PageDuration + PageDuration);

        std::vector<size_t> Indexes;
        for (size_t Idx = 1; Idx < 1024; Idx += 2) {
            Indexes.push_back(Idx);
        }

        unsigned Seed = std::chrono::system_clock::now().time_since_epoch().count();
        std::default_random_engine Eng(Seed);
        std::shuffle(Indexes.begin(), Indexes.end(), Eng);
    
        for (size_t Idx : Indexes)
        {
            bool Merged = IM.addInterval(Epoch + Idx * PageDuration, IM.PageSlots, UpdateEvery);
            EXPECT_TRUE(Merged && IM.verify());
        }
        EXPECT_EQ(IM.size(), 1);
    
        EXPECT_TRUE(IM.after().has_value());
        EXPECT_EQ(IM.after(), Epoch);
        EXPECT_TRUE(IM.before().has_value());
        EXPECT_EQ(IM.before(), Epoch + 1024 * PageDuration);
    }

    {
        // Test that we can't add overlapping intervals
        
        IntervalManager<60> IM;

        size_t Epoch = 3600;
        size_t UpdateEvery = 1;
        size_t PageDuration = IM.PageSlots * UpdateEvery;

        IM.addInterval(Epoch, IM.PageSlots, UpdateEvery);

        IM.addInterval(Epoch, 10, UpdateEvery);
        EXPECT_EQ(IM.after(), Epoch);
        EXPECT_EQ(IM.before(), Epoch + PageDuration);

        IM.addInterval(Epoch, IM.PageSlots, UpdateEvery);
        EXPECT_EQ(IM.after(), Epoch);
        EXPECT_EQ(IM.before(), Epoch + PageDuration);

        IM.addInterval(Epoch, 2 * IM.PageSlots, UpdateEvery);
        EXPECT_EQ(IM.after(), Epoch);
        EXPECT_EQ(IM.before(), Epoch + PageDuration);
    }
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
    ::testing::GTEST_FLAG(filter) = "Intervals.*";

    int rc = RUN_ALL_TESTS();
    google::protobuf::ShutdownProtobufLibrary();
    return rc;
}
