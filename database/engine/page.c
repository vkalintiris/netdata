// SPDX-License-Identifier: GPL-3.0-or-later

#include "page.h"
#include "libnetdata/aral/aral.h"
#include "libnetdata/gorilla/gorilla.h"

typedef enum __attribute__((packed)) {
    PAGE_OPTION_ALL_VALUES_EMPTY    = (1 << 0),
    PAGE_OPTION_READ_ONLY           = (1 << 1),
    PAGE_OPTION_ON_DISK             = (1 << 2),
} PAGE_OPTIONS;

typedef struct {
    uint8_t *data;
    uint32_t size;
} page_raw_t;


typedef struct {
    uint32_t *head_buffer;
    size_t num_buffers;
    gorilla_writer_t *writer;  
} page_gorilla_t;

struct pgd {
    uint8_t type;           // the page type
    PAGE_OPTIONS options;   // options related to the page

    uint32_t used;          // the uses number of slots in the page
    uint32_t slots;         // the total number of slots available in the page

    union {
        page_raw_t raw;
        page_gorilla_t gorilla;
    };
};

// ----------------------------------------------------------------------------
// memory management

struct {
    ARAL *aral_pgd;
    ARAL *aral_data[RRD_STORAGE_TIERS];
    ARAL *aral_gorilla;
} pgd_alloc_globals = {};

static ARAL *pgd_aral_data_lookup(size_t size)
{
    for (size_t tier = 0; tier < storage_tiers; tier++)
        if (size == tier_page_size[tier])
            return pgd_alloc_globals.aral_data[tier];

    return NULL;
}

void pgd_init_arals(void)
{
    // pgd aral
    {
        char buf[20 + 1];
        snprintfz(buf, 20, "pgd");

        // FIXME: add stats
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

    // gorilla aral
    {
        char buf[20 + 1];
        snprintfz(buf, 20, "gorilla");

        // FIXME: add stats
        size_t gorilla_page_size = 128 * sizeof(uint32_t);
        pgd_alloc_globals.aral_gorilla = aral_create(
                buf,
                gorilla_page_size,
                64,
                512 * gorilla_page_size,
                NULL,
                NULL, NULL, false, false);
    }
}

static void *pgd_data_aral_alloc(size_t size)
{
    ARAL *ar = pgd_aral_data_lookup(size);
    if (!ar)
        return mallocz(size);

    return aral_mallocz(ar);
}

static void pgd_data_aral_free(void *page, size_t size __maybe_unused)
{
    ARAL *ar = pgd_aral_data_lookup(size);
    if (!ar)
        freez(page);

    aral_freez(ar, page);
}

// ----------------------------------------------------------------------------
// management api

PGD *pgd_create(uint8_t type, uint32_t slots, gorilla_writer_t *gw)
{
    PGD *pg = aral_mallocz(pgd_alloc_globals.aral_pgd);
    pg->type = type;
    pg->used = 0;
    pg->slots = slots;
    pg->options = PAGE_OPTION_ALL_VALUES_EMPTY;

    switch (type) {
        case PAGE_METRICS:
        case PAGE_TIER: {
            uint32_t size = slots * page_type_size[type];

            internal_fatal(!size || slots == 1,
                      "DBENGINE: invalid number of slots (%u) or page type (%u)", slots, type);

            pg->raw.size = size;
            pg->raw.data = pgd_data_aral_alloc(size);
            break;
        }
        case PAGE_GORILLA_METRICS: {
            pg->gorilla.head_buffer = aral_mallocz(pgd_alloc_globals.aral_gorilla);
            pg->gorilla.num_buffers = 1;

            pg->gorilla.writer = gw;
            *pg->gorilla.writer = gorilla_writer_init(pg->gorilla.head_buffer, 128);
            break;
        }
        default:
            fatal("Unknown page type: %uc", type);
    }

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
    pg->options = PAGE_OPTION_READ_ONLY | PAGE_OPTION_ON_DISK;

    switch (type)
    {
        case PAGE_METRICS:
        case PAGE_TIER:
            pg->raw.data = pgd_data_aral_alloc(size);
            pg->raw.size = size;
            pg->used = size / page_type_size[type];
            pg->slots = pg->used;
            memcpy(pg->raw.data, base, size);
            break;
        case PAGE_GORILLA_METRICS:
            pg->gorilla.head_buffer = NULL;
            pg->gorilla.num_buffers = 0;
            pg->gorilla.writer = NULL;
            fatal("GVD: not implemented yet");
        default:
            fatal("Unknown page type: %uc", type);
    }

    return pg;
}

void pgd_free(PGD *pg)
{
    if (!pg)
        return;

    if (pg == PGD_EMPTY)
        return;

    switch (pg->type)
    {
        case PAGE_METRICS:
        case PAGE_TIER:
            pgd_data_aral_free(pg->raw.data, pg->raw.size);
            break;
        case PAGE_GORILLA_METRICS:
            fatal("pgd_free() not implemented for gorilla pages");
        default:
            fatal("Unknown page type: %uc", pg->type);
    }

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
        
    switch (pg->type) {
        case PAGE_METRICS:
        case PAGE_TIER:
            return sizeof(PGD) + pg->raw.size;
        case PAGE_GORILLA_METRICS:
            // FIXME: simplify the expression
            return sizeof(PGD) + pg->gorilla.num_buffers * (128 * sizeof(uint32_t));
        default:
            fatal("Unknown page type: %uc", pg->type);
    }
}

uint32_t pgd_disk_footprint(PGD *pg)
{
    if (!pgd_slots_used(pg))
        return 0;

    // TODO: understand this
    pg->options |= PAGE_OPTION_READ_ONLY;

    switch (pg->type) {
        case PAGE_METRICS:
        case PAGE_TIER: {
            uint32_t used_size = pg->used * page_type_size[pg->type];
            internal_fatal(used_size > pg->raw.size, "Wrong disk footprint page size");
            return used_size;
        }
        case PAGE_GORILLA_METRICS:
            fatal("pgd_disk_footprint() not implemented for gorilla pages");
        default:
            fatal("Unknown page type: %uc", pg->type);
    }
}

void pgd_copy_to_extent(PGD *pg, uint8_t *dst, uint32_t dst_size)
{
    internal_fatal(pgd_disk_footprint(pg) != dst_size, "Wrong disk footprint size requested (need %u, available %u)",
                   pgd_disk_footprint(pg), dst_size);

    // TODO: understand this
    pg->options |= PAGE_OPTION_ON_DISK;

    switch (pg->type) {
        case PAGE_METRICS:
        case PAGE_TIER:
            memcpy(dst, pg->raw.data, dst_size);
            break;
        case PAGE_GORILLA_METRICS:
            fatal("pgd_copy_to_extent() not implemented for gorilla pages");
        default:
            fatal("Unknown page type: %uc", pg->type);
    }
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
        fatal("DBENGINE: attempted to write beyond page size (page type %u, slots %u, used %u)",
              pg->type, pg->slots, pg->used /* FIXME:, pg->size */);

    if (unlikely(pg->used != expected_slot))
        fatal("DBENGINE: page is not aligned to expected slot (used %u, expected %u)",
              pg->used, expected_slot);

    internal_fatal(pg->options & (PAGE_OPTION_READ_ONLY | PAGE_OPTION_ON_DISK),
                   "Data collection on read-only page");

    switch (pg->type) {
        case PAGE_METRICS: {
            storage_number *tier0_metric_data = (storage_number *)pg->raw.data;
            storage_number t = pack_storage_number(n, flags);
            tier0_metric_data[pg->used++] = t;

            if ((pg->options & PAGE_OPTION_ALL_VALUES_EMPTY) && does_storage_number_exist(t))
                pg->options &= ~PAGE_OPTION_ALL_VALUES_EMPTY;

            break;
        }
        case PAGE_TIER: {
            storage_number_tier1_t *tier12_metric_data = (storage_number_tier1_t *)pg->raw.data;
            storage_number_tier1_t t;
            t.sum_value = (float) n;
            t.min_value = (float) min_value;
            t.max_value = (float) max_value;
            t.anomaly_count = anomaly_count;
            t.count = count;
            tier12_metric_data[pg->used++] = t;

            if ((pg->options & PAGE_OPTION_ALL_VALUES_EMPTY) && fpclassify(n) != FP_NAN)
                pg->options &= ~PAGE_OPTION_ALL_VALUES_EMPTY;

            break;
        }
        case PAGE_GORILLA_METRICS: {
            pg->used++;
            storage_number t = pack_storage_number(n, flags);

            bool ok = gorilla_writer_write(pg->gorilla.writer, t);
            if (!ok) {
                uint32_t *new_buffer = aral_mallocz(pgd_alloc_globals.aral_gorilla);
                pg->gorilla.num_buffers++;
                gorilla_writer_add_buffer(pg->gorilla.writer, new_buffer, 128);
                ok = gorilla_writer_write(pg->gorilla.writer, t);

                internal_fatal(ok == false, "Failed to writer value in newly allocated gorilla buffer.");
            }
            break;
        }
        default:
            fatal("DBENGINE: unknown page type id %d", pg->type);
            break;
    }
}

// ----------------------------------------------------------------------------
// querying with cursor

static void pgdc_seek(PGDC *pgdc, uint32_t position)
{
    uint8_t type = pgdc->pgd->type;

    switch (type) {
        case PAGE_METRICS:
        case PAGE_TIER:
            break;
        case PAGE_GORILLA_METRICS: {
            pgdc->gr = gorilla_reader_init(pgdc->pgd->gorilla.head_buffer);

            for (uint32_t i = 0; i != position; i++) {
                uint32_t value;
                bool ok = gorilla_reader_read(&pgdc->gr, &value);

                if (!ok)
                    fatal("Positioning cursor failed because gorilla buffer has less than %u values", position);
            }
        }
        break;
        default:
            fatal("DBENGINE: unknown page type id %d", type);
            break;
    }
}

void pgdc_reset(PGDC *pgdc, PGD *pgd, uint32_t position)
{
    // pgd might be null and position equal to UINT32_MAX

    pgdc->pgd = pgd;
    pgdc->position = position;

    if (!pgd)
        return;

    if (pgd == PGD_EMPTY)
        return;

    if (position == UINT32_MAX)
        return;

    pgdc_seek(pgdc, position);
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
            storage_number *array = (storage_number *) pgdc->pgd->raw.data;
            storage_number n = array[pgdc->position++];

            sp->min = sp->max = sp->sum = unpack_storage_number(n);
            sp->flags = (SN_FLAGS)(n & SN_USER_FLAGS);
            sp->count = 1;
            sp->anomaly_count = is_storage_number_anomalous(n) ? 1 : 0;

            return true;
        }
        case PAGE_TIER: {
            storage_number_tier1_t *array = (storage_number_tier1_t *) pgdc->pgd->raw.data;
            storage_number_tier1_t n = array[pgdc->position++];

            sp->flags = n.anomaly_count ? SN_FLAG_NONE : SN_FLAG_NOT_ANOMALOUS;
            sp->count = n.count;
            sp->anomaly_count = n.anomaly_count;
            sp->min = n.min_value;
            sp->max = n.max_value;
            sp->sum = n.sum_value;

            return true;
        }
        case PAGE_GORILLA_METRICS: {
            uint32_t value;
            bool ok = gorilla_reader_read(&pgdc->gr, &value);

            if (!ok)
                fatal("Could not get next point because gorilla buffer does not have enough values");

            return true;
        }
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
