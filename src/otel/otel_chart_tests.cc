#include "otel_utils.h"
#include "otel_chart.h"

#include <gtest/gtest.h>
#include <limits>

TEST(DimensionTest, DefaultConstructor)
{
    Dimension D;
    EXPECT_TRUE(D.empty());
    EXPECT_EQ(D.numSamples(), 0);
}

TEST(DimensionTest, PushAndPopSample)
{
    Dimension D;

    std::vector<Sample> SV = {
        Sample{100, 1000},
        Sample{200, 2000},
    };

    D.pushSample(SV[0]);
    ASSERT_FALSE(D.empty());
    ASSERT_EQ(D.numSamples(), 1);

    D.pushSample(SV[1]);
    ASSERT_EQ(D.numSamples(), 2);

    Sample S = D.popSample();
    ASSERT_EQ(S.Value, SV[0].Value);
    ASSERT_EQ(S.TimePoint, SV[0].TimePoint);
    ASSERT_EQ(D.numSamples(), 1);

    D.popSample();
    ASSERT_TRUE(D.empty());

    ASSERT_DEATH(D.popSample(), "expected non-empty samples");
}

TEST(DimensionTest, StartTime)
{
    Dimension D;

    D.pushSample({100, 1000});
    D.pushSample({200, 2000});

    ASSERT_EQ(D.startTime(), 1000);
}

TEST(DimensionTest, UpdateEvery)
{
    Dimension D;

    D.pushSample({100, 1000});
    D.pushSample({200, 2000});
    D.pushSample({300, 3000});

    ASSERT_EQ(D.updateEvery(), 1000);
}

TEST(DimensionTest, CompareCollectionTime)
{
    // For LCT=14000 and UE=1000 the valid samples are in the
    // half-open range [14500, 15500)
    uint32_t LCT = 14000;
    uint32_t UpdateEvery = 1000;

    for (uint32_t TP = LCT; TP < (LCT + 2 * UpdateEvery); TP++) {
        uint32_t ExpectedValue;

        if (TP < 14500)
            ExpectedValue = -1;
        else if (TP >= 15500)
            ExpectedValue = 1;
        else
            ExpectedValue = 0;

        Dimension D;
        D.pushSample({1, TP});
        ASSERT_EQ(D.compareCollectionTime(LCT, UpdateEvery), ExpectedValue);
    }
}

TEST(DimensionTest, UpdateEveryWithIrregularIntervals)
{
    Dimension D;

    D.pushSample({1, 10});
    D.pushSample({1, 20});
    ASSERT_EQ(D.updateEvery(), 10);

    D.pushSample({1, 25});
    ASSERT_EQ(D.updateEvery(), 5);

    D.pushSample({1, 100});
    ASSERT_EQ(D.updateEvery(), 5);

    D.pushSample({200, 10});
    ASSERT_DEATH(D.updateEvery(), "expected unique timestamps");
}

TEST(DimensionContainer, BasicOperations) {
    DimensionContainer DC;
    
    DC.add("user", { 1, 1 });
    DC.add("user", { 10, 2 });

    DC.add("system", { 2, 1 });
    DC.add("system", { 20, 2 });
    DC.add("system", { 20, 5 });

    DC.add("nice", { 3, 2 });
    DC.add("nice", { 30, 4 });

    const auto& Dims = DC.dimensions();
    ASSERT_EQ(Dims.size(), 3);

    ASSERT_TRUE(Dims.contains("user"));
    const Dimension &User = Dims.at("user");
    ASSERT_FALSE(User.empty());
    ASSERT_EQ(User.numSamples(), 2);
    ASSERT_EQ(User.startTime(), 1);
    ASSERT_EQ(User.updateEvery(), 1);
    
    ASSERT_TRUE(Dims.contains("system"));
    const Dimension &System = Dims.at("system");
    ASSERT_FALSE(System.empty());
    ASSERT_EQ(System.numSamples(), 3);
    ASSERT_EQ(System.startTime(), 1);
    ASSERT_EQ(System.updateEvery(), 1);

    ASSERT_TRUE(Dims.contains("nice"));
    const Dimension &Nice = Dims.at("nice");
    ASSERT_FALSE(Nice.empty());
    ASSERT_EQ(Nice.startTime(), 2);
    ASSERT_EQ(Nice.updateEvery(), 2);

    DC.add("nice", { 3, 1 });
    ASSERT_EQ(Nice.startTime(), 1);
    ASSERT_EQ(Nice.updateEvery(), 1);

    ASSERT_EQ(DC.startTime(), 1);
    ASSERT_EQ(DC.updateEvery(), 1);

    ASSERT_FALSE(Dims.contains("foo"));
}

TEST(DimensionContainer, StartTimeAndUpdateEvery) {
    DimensionContainer DC;
    
    DC.add("user", { 1, 50 });
    DC.add("system", { 1, 100 });
    ASSERT_EQ(DC.startTime(), 50);
    ASSERT_EQ(DC.updateEvery(), std::numeric_limits<std::uint32_t>::max());

    DC.add("user", { 1, 25 });
    ASSERT_EQ(DC.startTime(), 25);
    ASSERT_EQ(DC.updateEvery(), 25);

    DC.add("system", { 1, 90 });
    ASSERT_EQ(DC.startTime(), 25);
    ASSERT_EQ(DC.updateEvery(), 10);

    DC.add("system", { 1, 95 });
    ASSERT_EQ(DC.startTime(), 25);
    ASSERT_EQ(DC.updateEvery(), 5);

    DC.add("user", { 1, 49 });
    ASSERT_EQ(DC.startTime(), 25);
    ASSERT_EQ(DC.updateEvery(), 1);
}
