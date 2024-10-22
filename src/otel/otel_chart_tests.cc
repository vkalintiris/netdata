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
    
    DC.add("user", { 1, 10});
    DC.add("system", { 2, 10});

    const auto& Dimensions = DC.dimensions();
    ASSERT_EQ(Dimensions.size(), 2);
    
    // // Check user dimension
    // auto userIt = Dimensions.find("user");
    // ASSERT_NE(userIt, Dimensions.end()) << "User dimension not found";
    // EXPECT_EQ(userIt->second.numSamples(), 2) << "Expected 2 samples in user dimension";
    
    // // Check system dimension
    // auto systemIt = Dimensions.find("system");
    // ASSERT_NE(systemIt, Dimensions.end()) << "System dimension not found";
    // EXPECT_EQ(systemIt->second.numSamples(), 1) << "Expected 1 sample in system dimension";
    
    // // Process the container
    // DC.process(2, 100);  // RampUpThreshold=2, GapThreshold=100
    
    // // Verify container is not committed initially
    // EXPECT_FALSE(DC.isCommitted()) << "Container should not be committed initially";
    
    // // Set committed state
    // DC.setCommitted(true);
    // EXPECT_TRUE(DC.isCommitted()) << "Container should be committed after setting";
}
