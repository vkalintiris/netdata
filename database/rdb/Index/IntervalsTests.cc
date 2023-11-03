#include "../rdb-private.h"

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
