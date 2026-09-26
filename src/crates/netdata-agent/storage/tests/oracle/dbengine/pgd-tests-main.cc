// SPDX-License-Identifier: GPL-3.0-or-later
//
// Entry point for running src/database/engine/page_test.cc (compiled verbatim with -DHAVE_GTEST against
// fake-gtest/gtest/gtest.h): the same call the daemon makes for `-W pgd-tests` (src/daemon/main.c:432-433).

#include "database/engine/page_test.h"

int main(int argc, char **argv) {
    return pgd_test(argc, argv);
}
