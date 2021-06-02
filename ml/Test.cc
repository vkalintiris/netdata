// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

#include "Unit.h"
#include "Host.h"

#include "gtest/gtest.h"

using namespace ml;

unsigned factorial(unsigned N) {
    unsigned Result = 1;

    if (!N)
        return 1;

    for (unsigned I = 1; I <= N; ++I)
        Result *= I;

    return Result;
}

TEST(FactorialTest, HandlesPositiveInput) {
  EXPECT_EQ(factorial(1), 1);
  EXPECT_EQ(factorial(2), 2);
  EXPECT_EQ(factorial(3), 6);
  EXPECT_EQ(factorial(8), 40320);
}

int ml_test(int argc, char *argv[]) {
    std::cout << "factorial(3): " << factorial(3) << std::endl;
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
