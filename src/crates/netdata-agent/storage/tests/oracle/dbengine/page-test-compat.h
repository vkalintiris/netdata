// SPDX-License-Identifier: GPL-3.0-or-later
//
// Force-included (-include) when compiling src/database/engine/page_test.cc verbatim. It bridges the two places
// where that file no longer matches the reference tree:
//   1. PAGE_METRICS / PAGE_GORILLA_METRICS were renamed to RRDENG_PAGE_TYPE_ARRAY_32BIT / _GORILLA_32BIT
//      (rrddiskprotocol.h, commit 00f897a883 "Code cleanup (#17237)"); the old names are defined nowhere now.
//   2. page_test.cc:462 calls pgc_destroy(cache) but cache.h:178 declares pgc_destroy(PGC *cache, bool flush).
#ifndef PAGE_TEST_COMPAT_H
#define PAGE_TEST_COMPAT_H

#include "database/engine/page.h"

#define PAGE_METRICS RRDENG_PAGE_TYPE_ARRAY_32BIT
#define PAGE_GORILLA_METRICS RRDENG_PAGE_TYPE_GORILLA_32BIT

static inline void pgc_destroy(PGC *cache) { pgc_destroy(cache, false); }

#endif
