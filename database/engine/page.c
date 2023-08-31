// SPDX-License-Identifier: GPL-3.0-or-later

#include "page.h"
#include "libnetdata/aral/aral.h"

typedef enum __attribute__((packed)) {
    PAGE_OPTION_ALL_VALUES_EMPTY    = (1 << 0),
    PAGE_OPTION_READ_ONLY           = (1 << 1),
    PAGE_OPTION_ON_DISK             = (1 << 2),
} PAGE_OPTIONS;

struct pgd {
    uint8_t type;           // the page type
    PAGE_OPTIONS options;   // options related to the page

    uint32_t used;          // the uses number of slots in the page
    uint32_t slots;         // the total number of slots available in the page

    uint32_t size;          // the size of data in bytes
    uint8_t *data;         // the data of the page
};

// ----------------------------------------------------------------------------
// memory management

struct {
    ARAL *aral_pgd;
    ARAL *aral_data[RRD_STORAGE_TIERS];
} pgd_alloc_globals = {};

static ARAL *pgd_aral_data_lookup(size_t size)
{
    for (size_t tier = 0; tier < storage_tiers; tier++)
        if (size == tier_page_size[tier] + sizeof(PGD))
            return pgd_alloc_globals.aral_data[tier];

    return NULL;
}

void pgd_init_arals(void)
{
    // pgd aral
    {
        char buf[20 + 1];
        snprintfz(buf, 20, "pgd");

        // TODO: add stats
        pgd_alloc_globals.aral_pgd = aral_create(
                buf,
                sizeof(struct pgd),
                64,
                512 * (sizeof(struct pgd)),
                NULL,
                NULL, NULL, false, false);
    }

    // tier page aral
    {
        for (size_t i = storage_tiers; i > 0 ;i--)
        {
            size_t tier = storage_tiers - i;

            char buf[20 + 1];
            snprintfz(buf, 20, "tier%zu-pages", tier);

            pgd_alloc_globals.aral_data[tier] = aral_create(
                    buf,
                    tier_page_size[tier],
                    64,
                    512 * (tier_page_size[tier]),
                    pgc_aral_statistics(),
                    NULL, NULL, false, false);
        }
    }
}

static void *pgd_data_aral_alloc(size_t size)
{
    ARAL *ar = pgd_aral_data_lookup(size);
    if(ar) return aral_mallocz(ar);

    return mallocz(size);
}

static void pgd_data_aral_free(void *page, size_t size __maybe_unused)
{
    ARAL *ar = pgd_aral_data_lookup(size);
    if(ar)
        aral_freez(ar, page);
    else
        freez(page);
}

// ----------------------------------------------------------------------------
// management api

PGD *pgd_create(uint8_t type, uint32_t slots)
{
    uint32_t size = slots * page_type_size[type];

    internal_fatal(!size || slots == 1, "DBENGINE: invalid number of slots (%u) or page type (%u)",
                   slots, type);

    PGD *pg = aral_mallocz(pgd_alloc_globals.aral_pgd);
    pg->type = type;
    pg->size = size;
    pg->used = 0;
    pg->slots = slots;
    pg->options = PAGE_OPTION_ALL_VALUES_EMPTY;
    pg->data = pgd_data_aral_alloc(size);

    return pg;
}

PGD *pgd_create_from_disk_data(uint8_t type, void *base, uint32_t size)
{
    if (!size)
        return PGD_EMPTY;

    if (size < page_type_size[type])
        return PGD_EMPTY;

    PGD *pg = aral_mallocz(pgd_alloc_globals.aral_pgd);
    pg->type = type;
    pg->size = size;
    pg->used = size / page_type_size[type];
    pg->slots = pg->used;
    pg->options = PAGE_OPTION_READ_ONLY | PAGE_OPTION_ON_DISK;
    pg->data = pgd_data_aral_alloc(size);

    memcpy(pg->data, base, size);

    return pg;
}

void pgd_free(PGD *pg)
{
    if (!pg)
        return;

    if (pg == PGD_EMPTY)
        return;
        
    pgd_data_aral_free(pg->data, pg->size);
    aral_freez(pgd_alloc_globals.aral_pgd, pg);
}

// ----------------------------------------------------------------------------
// utility functions

uint32_t pgd_type(PGD *pg)
{
    return pg->type;
}

bool pgd_is_empty(PGD *pg)
{
    if (pg == PGD_EMPTY)
        return true;

    if (pg->used == 0)
        return true;

    if (pg->options & PAGE_OPTION_ALL_VALUES_EMPTY)
        return true;

    return false;
}

uint32_t pgd_slots_used(PGD *pg)
{
    if (!pg)
        return 0;

    if (pg == PGD_EMPTY)
        return 0;

    return pg->used;
}

uint32_t pgd_memory_footprint(PGD *pg)
{
    if (!pg)
        return 0;

    if (pg == PGD_EMPTY)
        return 0;
        
    return sizeof(PGD) + pg->size;
}

uint32_t pgd_disk_footprint(PGD *pg)
{
    uint32_t used_size = 0;

    if(pg && pg != PGD_EMPTY && pg->used) {
        used_size = pg->used * page_type_size[pg->type];

        internal_fatal(used_size > pg->size, "Wrong disk footprint page size");

        pg->options |= PAGE_OPTION_READ_ONLY;
    }

    return used_size;
}

void pgd_copy_to_extent(PGD *pg, uint8_t *dst, uint32_t dst_size)
{
    internal_fatal(pgd_disk_footprint(pg) != dst_size, "Wrong disk footprint size requested (need %u, available %u)",
                   pgd_disk_footprint(pg), dst_size);

    memcpy(dst, pg->data, dst_size);
    pg->options |= PAGE_OPTION_ON_DISK;
}


// ----------------------------------------------------------------------------
// data collection

void pgd_append_point(PGD *pg,
                      usec_t point_in_time_ut __maybe_unused,
                      NETDATA_DOUBLE n,
                      NETDATA_DOUBLE min_value,
                      NETDATA_DOUBLE max_value,
                      uint16_t count,
                      uint16_t anomaly_count,
                      SN_FLAGS flags,
                      uint32_t expected_slot)
{
    if (unlikely(pg->used >= pg->slots))
        fatal("DBENGINE: attempted to write beyond page size (page type %u, slots %u, used %u, size %u)",
              pg->type, pg->slots, pg->used, pg->size);

    if (unlikely(pg->used != expected_slot))
        fatal("DBENGINE: page is not aligned to expected slot (used %u, expected %u)",
              pg->used, expected_slot);

    internal_fatal(pg->options & (PAGE_OPTION_READ_ONLY|PAGE_OPTION_ON_DISK), "Data collection on read-only page");

    switch (pg->type) {
        case PAGE_METRICS: {
            storage_number *tier0_metric_data = (storage_number *)pg->data;
            storage_number t = pack_storage_number(n, flags);
            tier0_metric_data[pg->used++] = t;

            if ((pg->options & PAGE_OPTION_ALL_VALUES_EMPTY) && does_storage_number_exist(t))
                pg->options &= ~PAGE_OPTION_ALL_VALUES_EMPTY;
        }
        break;

        case PAGE_TIER: {
            storage_number_tier1_t *tier12_metric_data = (storage_number_tier1_t *)pg->data;
            storage_number_tier1_t t;
            t.sum_value = (float) n;
            t.min_value = (float) min_value;
            t.max_value = (float) max_value;
            t.anomaly_count = anomaly_count;
            t.count = count;
            tier12_metric_data[pg->used++] = t;

            if ((pg->options & PAGE_OPTION_ALL_VALUES_EMPTY) && fpclassify(n) != FP_NAN)
                pg->options &= ~PAGE_OPTION_ALL_VALUES_EMPTY;
        }
        break;

        default:
            fatal("DBENGINE: unknown page type id %d", pg->type);
            break;
    }
}

// ----------------------------------------------------------------------------
// querying with cursor

static void pgdc_seek(PGDC *pgdc)
{
    UNUSED(pgdc);
}

void pgdc_reset(PGDC *pgdc, PGD *pgd, uint32_t position)
{
    pgdc->pgd = pgd;
    pgdc->position = position;

    if (pgd && pgd != PGD_EMPTY)
        pgdc_seek(pgdc);
}

bool pgdc_get_next_point(PGDC *pgdc, uint32_t expected_position, STORAGE_POINT *sp)
{
    if (!pgdc->pgd || pgdc->pgd == PGD_EMPTY || pgdc->position >= pgdc->pgd->slots)
    {
        storage_point_empty(*sp, sp->start_time_s, sp->end_time_s);
        return false;
    }

    internal_fatal(pgdc->position != expected_position, "Wrong expected cursor position");

    switch (pgdc->pgd->type)
    {
        case PAGE_METRICS: {
            storage_number *array = (storage_number *)pgdc->pgd->data;
            storage_number n = array[pgdc->position++];
            sp->min = sp->max = sp->sum = unpack_storage_number(n);
            sp->flags = (SN_FLAGS)(n & SN_USER_FLAGS);
            sp->count = 1;
            sp->anomaly_count = is_storage_number_anomalous(n) ? 1 : 0;
            return true;
        }
        case PAGE_TIER: {
            storage_number_tier1_t *array = (storage_number_tier1_t *)pgdc->pgd->data;
            storage_number_tier1_t n = array[pgdc->position++];
            sp->flags = n.anomaly_count ? SN_FLAG_NONE : SN_FLAG_NOT_ANOMALOUS;
            sp->count = n.count;
            sp->anomaly_count = n.anomaly_count;
            sp->min = n.min_value;
            sp->max = n.max_value;
            sp->sum = n.sum_value;
            return true;
        }
        // we don't know this page type
        default: {
            static bool logged = false;
            if (!logged)
            {
                netdata_log_error("DBENGINE: unknown page type %d found. Cannot decode it. Ignoring its metrics.", pgd_type(pgdc->pgd));
                logged = true;
            }

            storage_point_empty(*sp, sp->start_time_s, sp->end_time_s);
            return false;
        }
    }
}
