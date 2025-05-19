#include "netdata_provider.h"

int32_t nsd_id128_from_string(const char *s, NsdId128 *ret)
{
#ifdef RUST_PROVIDER
    return rsd_id128_from_string(s, (struct RsdId128 *) ret);
#else
    return sd_id128_from_string(s, (sd_id128_t *) ret);
#endif
}

int32_t nsd_id128_equal(NsdId128 a, NsdId128 b)
{
#ifdef RUST_PROVIDER
    return rsd_id128_equal(a, b);
#else
    return sd_id128_equal(a, b);
#endif
}

int nsd_journal_open_files(NsdJournal **ret, const char *const *paths, int flags)
{
#ifdef RUST_PROVIDER
    return rsd_journal_open_files(ret, paths, flags);
#else
    return sd_journal_open_files(ret, paths, flags);
#endif
}

void nsd_journal_close(NsdJournal *j)
{
#ifdef RUST_PROVIDER
    rsd_journal_close(j);
#else
    sd_journal_close(j);
#endif
}

int nsd_journal_seek_head(NsdJournal *j)
{
#ifdef RUST_PROVIDER
    return rsd_journal_seek_head(j);
#else
    return sd_journal_seek_head(j);
#endif
}

int nsd_journal_seek_tail(NsdJournal *j)
{
#ifdef RUST_PROVIDER
    return rsd_journal_seek_tail(j);
#else
    return sd_journal_seek_tail(j);
#endif
}

int nsd_journal_seek_realtime_usec(NsdJournal *j, uint64_t usec)
{
#ifdef RUST_PROVIDER
    return rsd_journal_seek_realtime_usec(j, usec);
#else
    return sd_journal_seek_realtime_usec(j, usec);
#endif
}

int nsd_journal_next(NsdJournal *j)
{
#ifdef RUST_PROVIDER
    return rsd_journal_next(j);
#else
    return sd_journal_next(j);
#endif
}

int nsd_journal_previous(NsdJournal *j)
{
#ifdef RUST_PROVIDER
    return rsd_journal_previous(j);
#else
    return sd_journal_previous(j);
#endif
}

int nsd_journal_get_seqnum(NsdJournal *j, uint64_t *ret_seqnum, NsdId128 *ret_seqnum_id)
{
#ifdef RUST_PROVIDER
    return rsd_journal_get_seqnum(j, ret_seqnum, ret_seqnum_id);
#else
    return sd_journal_get_seqnum(j, ret_seqnum, ret_seqnum_id);
#endif
}

int nsd_journal_get_realtime_usec(NsdJournal *j, uint64_t *ret)
{
#ifdef RUST_PROVIDER
    return rsd_journal_get_realtime_usec(j, ret);
#else
    return sd_journal_get_realtime_usec(j, ret);
#endif
}

void nsd_journal_restart_data(NsdJournal *j)
{
#ifdef RUST_PROVIDER
    rsd_journal_restart_data(j);
#else
    sd_journal_restart_data(j);
#endif
}

int nsd_journal_enumerate_available_data(NsdJournal *j, const void **data, uintptr_t *l)
{
#ifdef RUST_PROVIDER
    return rsd_journal_enumerate_available_data(j, data, l);
#else
    return sd_journal_enumerate_available_data(j, data, l);
#endif
}

void nsd_journal_restart_fields(NsdJournal *j)
{
#ifdef RUST_PROVIDER
    rsd_journal_restart_fields(j);
#else
    sd_journal_restart_fields(j);
#endif
}

int nsd_journal_enumerate_fields(NsdJournal *j, const char **field)
{
#ifdef RUST_PROVIDER
    return rsd_journal_enumerate_fields(j, field);
#else
    return sd_journal_enumerate_fields(j, field);
#endif
}

int nsd_journal_query_unique(NsdJournal *j, const char *field)
{
#ifdef RUST_PROVIDER
    return rsd_journal_query_unique(j, field);
#else
    return sd_journal_query_unique(j, field);
#endif
}

void nsd_journal_restart_unique(NsdJournal *j)
{
#ifdef RUST_PROVIDER
    rsd_journal_restart_unique(j);
#else
    sd_journal_restart_unique(j);
#endif
}

int nsd_journal_enumerate_available_unique(NsdJournal *j, const void **data, uintptr_t *l)
{
#ifdef RUST_PROVIDER
    return rsd_journal_enumerate_available_unique(j, data, l);
#else
    return sd_journal_enumerate_available_unique(j, data, l);
#endif
}

int nsd_journal_add_match(NsdJournal *j, const void *data, uintptr_t size)
{
#ifdef RUST_PROVIDER
    return rsd_journal_add_match(j, data, size);
#else
    return sd_journal_add_match(j, data, size);
#endif
}

int nsd_journal_add_conjunction(NsdJournal *j)
{
#ifdef RUST_PROVIDER
    return rsd_journal_add_conjunction(j);
#else
    return sd_journal_add_conjunction(j);
#endif
}

int nsd_journal_add_disjunction(NsdJournal *j)
{
#ifdef RUST_PROVIDER
    return rsd_journal_add_disjunction(j);
#else
    return sd_journal_add_disjunction(j);
#endif
}

void nsd_journal_flush_matches(NsdJournal *j)
{
#ifdef RUST_PROVIDER
    return rsd_journal_flush_matches(j);
#else
    return sd_journal_flush_matches(j);
#endif
}
