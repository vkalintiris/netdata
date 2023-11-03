#include "../rdb-private.h"

TEST(Intervals, foo) {
    EXPECT_TRUE(true);
}

int rdb_intervals_tests_main(int argc, char *argv[])
{
    // skip the `-W intervals-tests` args
    for (int i = 2; i < argc; ++i)
    {
        argv[i - 1] = argv[i];
    }
    argc -= 2;

    for (size_t i = 0; i != argc; i++)
    {
        netdata_log_error("CLI arg[%d]: %s", i, argv[i]);
    }

    ::testing::InitGoogleTest(&argc, argv);
    ::testing::GTEST_FLAG(filter) = "Intervals.*";

    int rc = RUN_ALL_TESTS();
    google::protobuf::ShutdownProtobufLibrary();
    return rc;
}
