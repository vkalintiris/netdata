// SPDX-License-Identifier: GPL-3.0-or-later
//
// Link stubs so the real src/database/engine/page.c (compiled from the reference tree) runs outside the netdata
// daemon. None of them influences the encoded bytes:
//   - tier_page_size / page_type_size: copied from src/database/engine/rrdengineapi.c:28-42 (64-bit values);
//   - nd_profile_detect_and_configure / netdata_conf_cpus: only size the ARAL pools in pgd_init_arals (page.c:243);
//   - pulse_*: statistics counters (page.c:358, 492, 845, 970);
//   - pgc_create / pgc_destroy: the dummy cache page_test.cc:455 creates; page.c never uses it.

#include "database/engine/page.h"

size_t tier_page_size[RRD_STORAGE_TIERS] = {4096, 2048, 384, 384, 384};

size_t page_type_size[256] = {
    [RRDENG_PAGE_TYPE_ARRAY_32BIT] = sizeof(storage_number),
    [RRDENG_PAGE_TYPE_ARRAY_TIER1] = sizeof(storage_number_tier1_t),
    [RRDENG_PAGE_TYPE_GORILLA_32BIT] = sizeof(storage_number),
};

ND_PROFILE nd_profile_detect_and_configure(bool recheck) {
    (void)recheck;
    return ND_PROFILE_STANDALONE;
}

size_t netdata_conf_cpus(void) {
    return 4;
}

void pulse_aral_register_statistics(struct aral_statistics *stats, const char *name) {
    (void)stats;
    (void)name;
}

void pulse_gorilla_hot_buffer_added(void) {
}

void pulse_gorilla_tier0_page_flush(uint32_t actual, uint32_t optimal, uint32_t original) {
    (void)actual;
    (void)optimal;
    (void)original;
}

PGC *pgc_create(const char *name, size_t clean_size_bytes, free_clean_page_callback pgc_free_clean_cb,
                size_t max_dirty_pages_per_flush, save_dirty_init_callback pgc_save_init_cb,
                save_dirty_page_callback pgc_save_dirty_cb, size_t max_pages_per_inline_eviction,
                size_t max_inline_evictors, size_t max_skip_pages_per_inline_eviction, size_t max_flushes_inline,
                PGC_OPTIONS options, size_t partitions, size_t additional_bytes_per_page) {
    (void)name; (void)clean_size_bytes; (void)pgc_free_clean_cb; (void)max_dirty_pages_per_flush;
    (void)pgc_save_init_cb; (void)pgc_save_dirty_cb; (void)max_pages_per_inline_eviction; (void)max_inline_evictors;
    (void)max_skip_pages_per_inline_eviction; (void)max_flushes_inline; (void)options; (void)partitions;
    (void)additional_bytes_per_page;
    return NULL;
}

void pgc_destroy(PGC *cache, bool flush) {
    (void)cache;
    (void)flush;
}
