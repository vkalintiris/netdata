#include "otel_utils.h"
#include "otel_chart.h"

#include <gtest/gtest.h>

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
    EXPECT_FALSE(D.empty());
    EXPECT_EQ(D.numSamples(), 1);

    D.pushSample(SV[1]);
    EXPECT_EQ(D.numSamples(), 2);

    Sample S = D.popSample();
    EXPECT_EQ(S.Value, SV[0].Value);
    EXPECT_EQ(S.TimePoint, SV[0].TimePoint);
    EXPECT_EQ(D.numSamples(), 1);

    D.popSample();
    EXPECT_TRUE(D.empty());

    ASSERT_DEATH(D.popSample(), "expected non-empty samples");
}

TEST(DimensionTest, StartTime)
{
    Dimension D;

    D.pushSample({100, 1000});
    D.pushSample({200, 2000});

    EXPECT_EQ(D.startTime(), 1000);
}

TEST(DimensionTest, UpdateEvery)
{
    Dimension D;

    D.pushSample({100, 1000});
    D.pushSample({200, 2000});
    D.pushSample({300, 3000});

    EXPECT_EQ(D.updateEvery(), 1000);
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
        EXPECT_EQ(D.compareCollectionTime(LCT, UpdateEvery), ExpectedValue);
    }
}

TEST(DimensionTest, UpdateEveryWithIrregularIntervals)
{
    Dimension D;

    D.pushSample({1, 10});
    D.pushSample({1, 20});
    EXPECT_EQ(D.updateEvery(), 10);

    D.pushSample({1, 25});
    EXPECT_EQ(D.updateEvery(), 5);

    D.pushSample({1, 100});
    EXPECT_EQ(D.updateEvery(), 5);

    D.pushSample({200, 10});
    ASSERT_DEATH(D.updateEvery(), "expected unique timestamps");
}
