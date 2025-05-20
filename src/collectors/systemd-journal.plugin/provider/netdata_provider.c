#include <stdio.h>
#include "netdata_provider.h"

static int format_bytes_to_hex(const unsigned char *src, size_t src_len,
                               char *dst, size_t dst_size) {
    // Calculate required destination size: 3 chars per byte (2 hex + 1 space)
    // minus 1 space after the last byte, plus 1 for null terminator
    size_t required_size = src_len * 3 - 1 + 1;

    // Check if destination buffer is large enough
    if (dst_size < required_size) {
        return -1;
    }

    // Reset destination buffer
    dst[0] = '\0';

    // Current position in the destination buffer
    size_t pos = 0;

    // Format each byte
    for (size_t i = 0; i < src_len; i++) {
        // Format current byte as hex
        pos += sprintf(dst + pos, "%02X", src[i]);

        // Add space after each byte except the last one
        if (i < src_len - 1) {
            dst[pos++] = ' ';
            dst[pos] = '\0';
        }
    }

    return pos;
}

int32_t nsd_id128_from_string(const char *s, NsdId128 *ret)
{
#if defined(HAVE_RUST_PROVIDER)
    return rsd_id128_from_string(s, (struct RsdId128 *) ret);
#else
    return sd_id128_from_string(s, (sd_id128_t *) ret);
#endif
}

int32_t nsd_id128_equal(NsdId128 a, NsdId128 b)
{
#if defined(HAVE_RUST_PROVIDER)
    return rsd_id128_equal(a, b);
#else
    return sd_id128_equal(a, b);
#endif
}

int nsd_journal_open_files(NsdJournal **ret, const char *const *paths, int flags)
{
#if defined(HAVE_BOTH_PROVIDERS)
    *ret = calloc(1, sizeof(NsdJournal));
    if (!ret) {
        fprintf(stderr, "[1] nsd_journal_open_files\n");
        abort();
    }

    int rc = sd_journal_open_files(&(*ret)->sdj, paths, flags);
    int rsd_rc = rsd_journal_open_files(&(*ret)->rsdj, paths, flags);

    if (rc != rsd_rc) {
        fprintf(stderr, "[2] nsd_journal_open_files\n");
        abort();
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_open_files(ret, paths, flags);
#else
    return sd_journal_open_files(ret, paths, flags);
#endif
}

void nsd_journal_close(NsdJournal *j)
{
#if defined(HAVE_BOTH_PROVIDERS)
    sd_journal_close(j->sdj);
    rsd_journal_close(j->rsdj);
#elif defined(HAVE_RUST_PROVIDER)
    rsd_journal_close(j);
#else
    sd_journal_close(j);
#endif
}

int nsd_journal_seek_head(NsdJournal *j)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_seek_head(j->sdj);
    int rsd_rc = rsd_journal_seek_head(j->rsdj);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_seek_head\n");
        abort();
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_seek_head(j);
#else
    return sd_journal_seek_head(j);
#endif
}

int nsd_journal_seek_tail(NsdJournal *j)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_seek_tail(j->sdj);
    int rsd_rc = rsd_journal_seek_tail(j->rsdj);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_seek_tail\n");
        abort();
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_seek_tail(j);
#else
    return sd_journal_seek_tail(j);
#endif
}

int nsd_journal_seek_realtime_usec(NsdJournal *j, uint64_t usec)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_seek_realtime_usec(j->sdj, usec);
    int rsd_rc = rsd_journal_seek_realtime_usec(j->rsdj, usec);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_seek_realtime_usec\n");
        abort();
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_seek_realtime_usec(j, usec);
#else
    return sd_journal_seek_realtime_usec(j, usec);
#endif
}

int nsd_journal_next(NsdJournal *j)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_next(j->sdj);
    int rsd_rc = rsd_journal_next(j->rsdj);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_next\n");
        abort();
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_next(j);
#else
    return sd_journal_next(j);
#endif
}

int nsd_journal_previous(NsdJournal *j)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_previous(j->sdj);
    int rsd_rc = rsd_journal_previous(j->rsdj);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_previous\n");
        abort();
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_previous(j);
#else
    return sd_journal_previous(j);
#endif
}

int nsd_journal_get_seqnum(NsdJournal *j, uint64_t *ret_seqnum, NsdId128 *ret_seqnum_id)
{
#if defined(HAVE_BOTH_PROVIDERS)
    uint64_t sd_ret_seqnum;
    sd_id128_t sd_ret_seqnum_id;
    int rc = sd_journal_get_seqnum(j->sdj, &sd_ret_seqnum, &sd_ret_seqnum_id);
    if (rc == 0) {
        *ret_seqnum = sd_ret_seqnum;
        memcpy(ret_seqnum_id->bytes, sd_ret_seqnum_id.bytes, 16);
    }

    uint64_t rsd_ret_seqnum;
    RsdId128 rsd_ret_seqnum_id;
    int rsd_rc = rsd_journal_get_seqnum(j->rsdj, &rsd_ret_seqnum, &rsd_ret_seqnum_id);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_get_seqnum\n");
        abort();
    }

    if (rc == 0) {
        if (sd_ret_seqnum != rsd_ret_seqnum) {
            fprintf(stderr, "[2] nsd_journal_get_seqnum\n");
            abort();
        }

        if (memcmp(ret_seqnum_id->bytes, rsd_ret_seqnum_id.bytes, 16) != 0) {
            char sd_bytes[128];
            {
                format_bytes_to_hex(ret_seqnum_id->bytes, 16, sd_bytes, 100);
            }

            char rsd_bytes[128];
            {
                format_bytes_to_hex(rsd_ret_seqnum_id.bytes, 16, rsd_bytes, 100);
            }

            fprintf(stderr, "[3] nsd_journal_get_seqnum: sd=%lu>>>%s<<<, rsd=%lu>>>%s<<<\n", sd_ret_seqnum, sd_bytes, rsd_ret_seqnum, rsd_bytes);
            abort();
        }
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_get_seqnum(j, ret_seqnum, ret_seqnum_id);
#else
    return sd_journal_get_seqnum(j, ret_seqnum, ret_seqnum_id);
#endif
}

int nsd_journal_get_realtime_usec(NsdJournal *j, uint64_t *ret)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_get_realtime_usec(j->sdj, ret);

    uint64_t rsd_ret = 0;
    int rsd_rc = rsd_journal_get_realtime_usec(j->rsdj, &rsd_ret);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_get_realtime_usec: rc=%d, rsd_rc=%d, ret=%zu, rsd_ret=%zu\n", rc, rsd_rc, *ret, rsd_ret);
        abort();
    }

    if (rc == 0) {
        if (*ret != rsd_ret) {
            fprintf(stderr, "[2] nsd_journal_get_realtime_usec\n");
            abort();
        }
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_get_realtime_usec(j, ret);
#else
    return sd_journal_get_realtime_usec(j, ret);
#endif
}

void nsd_journal_restart_data(NsdJournal *j)
{
#if defined(HAVE_BOTH_PROVIDERS)
    sd_journal_restart_data(j->sdj);
    rsd_journal_restart_data(j->rsdj);
#elif defined(HAVE_RUST_PROVIDER)
    rsd_journal_restart_data(j);
#else
    sd_journal_restart_data(j);
#endif
}

int nsd_journal_enumerate_available_data(NsdJournal *j, const void **data, uintptr_t *l)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_enumerate_available_data(j->sdj, data, l);

    const void *rsd_data = NULL;
    uintptr_t rsd_l = 0;
    int rsd_rc = rsd_journal_enumerate_available_data(j->rsdj, &rsd_data, &rsd_l);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_enumerate_available_data\n");
        abort();
    }

    if (rc > 0) {
        if (*l != rsd_l) {
            fprintf(stderr, "[2] nsd_journal_enumerate_available_data\n");
            abort();
        }

        if (memcmp(*data, rsd_data, rsd_l)) {
            fprintf(stderr, "[3] nsd_journal_enumerate_available_data\n");
            abort();
        }
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_enumerate_available_data(j, data, l);
#else
    return sd_journal_enumerate_available_data(j, data, l);
#endif
}

void nsd_journal_restart_fields(NsdJournal *j)
{
#if defined(HAVE_BOTH_PROVIDERS)
    sd_journal_restart_fields(j->sdj);
    rsd_journal_restart_fields(j->rsdj);
#elif defined(HAVE_RUST_PROVIDER)
    rsd_journal_restart_fields(j);
#else
    sd_journal_restart_fields(j);
#endif
}

int nsd_journal_enumerate_fields(NsdJournal *j, const char **field)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_enumerate_fields(j->sdj, field);

    const char *rsd_field = NULL;
    int rsd_rc = rsd_journal_enumerate_fields(j->rsdj, &rsd_field);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_enumerate_fields\n");
        abort();
    }

    if (rc > 0) {
        if (strcmp(*field, rsd_field)) {
            fprintf(stderr, "[2] nsd_journal_enumerate_fields\n");
            abort();
        }
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_enumerate_fields(j, field);
#else
    return sd_journal_enumerate_fields(j, field);
#endif
}

int nsd_journal_query_unique(NsdJournal *j, const char *field)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_query_unique(j->sdj, field);
    int sd_rc = rsd_journal_query_unique(j->rsdj, field);

    if (rc != sd_rc) {
        fprintf(stderr, "[1] nsd_journal_query_unique\n");
        abort();
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_query_unique(j, field);
#else
    return sd_journal_query_unique(j, field);
#endif
}

void nsd_journal_restart_unique(NsdJournal *j)
{
#if defined(HAVE_BOTH_PROVIDERS)
    sd_journal_restart_unique(j->sdj);
    rsd_journal_restart_unique(j->rsdj);
#elif defined(HAVE_RUST_PROVIDER)
    rsd_journal_restart_unique(j);
#else
    sd_journal_restart_unique(j);
#endif
}

int nsd_journal_enumerate_available_unique(NsdJournal *j, const void **data, uintptr_t *l)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_enumerate_available_unique(j->sdj, data, l);

    const void *rsd_data = NULL;
    uintptr_t rsd_l = 0;
    int rsd_rc = rsd_journal_enumerate_available_unique(j->rsdj, &rsd_data, &rsd_l);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_enumerate_available_unique\n");
        abort();
    }

    if (rc > 0) {
        if (*l != rsd_l) {
            fprintf(stderr, "[2] nsd_journal_enumerate_available_unique\n");
            abort();
        }

        if (memcmp(*data, rsd_data, *l)) {
            fprintf(stderr, "[3] nsd_journal_enumerate_available_unique\n");
            abort();
        }
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_enumerate_available_unique(j, data, l);
#else
    return sd_journal_enumerate_available_unique(j, data, l);
#endif
}

int nsd_journal_add_match(NsdJournal *j, const void *data, uintptr_t size)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_add_match(j->sdj, data, size);
    int rsd_rc = rsd_journal_add_match(j->rsdj, data, size);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_add_match\n");
        abort();
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_add_match(j, data, size);
#else
    return sd_journal_add_match(j, data, size);
#endif
}

int nsd_journal_add_conjunction(NsdJournal *j)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_add_conjunction(j->sdj);
    int rsd_rc = rsd_journal_add_conjunction(j->rsdj);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_add_conjunction\n");
        abort();
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_add_conjunction(j);
#else
    return sd_journal_add_conjunction(j);
#endif
}

int nsd_journal_add_disjunction(NsdJournal *j)
{
#if defined(HAVE_BOTH_PROVIDERS)
    int rc = sd_journal_add_disjunction(j->sdj);
    int rsd_rc = rsd_journal_add_disjunction(j->rsdj);

    if (rc != rsd_rc) {
        fprintf(stderr, "[1] nsd_journal_add_disjunction\n");
        abort();
    }

    return rc;
#elif defined(HAVE_RUST_PROVIDER)
    return rsd_journal_add_disjunction(j);
#else
    return sd_journal_add_disjunction(j);
#endif
}

void nsd_journal_flush_matches(NsdJournal *j)
{
#if defined(HAVE_BOTH_PROVIDERS)
    sd_journal_flush_matches(j->sdj);
    rsd_journal_flush_matches(j->rsdj);
#elif defined(HAVE_RUST_PROVIDER)
    rsd_journal_flush_matches(j);
#else
    sd_journal_flush_matches(j);
#endif
}
