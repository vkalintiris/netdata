// SPDX-License-Identifier: GPL-3.0-or-later

#include "dbengine_test.h"

#include "common.h"
#include <limits>
#include <vector>

// Save dimensions' values under cache dir
// #define LOG_VALUES_TO_FILES 1

static const int CHARTS = 64;
static const int DIMS = 16;

#define REGIONS 3

static const int REGION_UPDATE_EVERY[REGIONS] = {2, 3, 1};
static const int REGION_POINTS[REGIONS] = {
    16384,
    16384,
    16384,
};
static const int QUERY_BATCH = 4096;

static void rrddim_set_by_pointer_fake_time(RRDDIM *rd, collected_number value, time_t now)
{
    rd->collector.last_collected_time.tv_sec = now;
    rd->collector.last_collected_time.tv_usec = 0;
    rd->collector.collected_value = value;
    rd->collector.options =
        static_cast<RRDDIM_OPTIONS>(static_cast<int>(rd->collector.options) | RRDDIM_OPTION_UPDATED);

    rd->collector.counter++;

#ifdef LOG_VALUES_TO_FILES
    fprintf(rd->fp, "[%ld] = %lld\n", now, value);
#endif

    collected_number v = (value >= 0) ? value : -value;
    if (unlikely(v > rd->collector.collected_value_max))
        rd->collector.collected_value_max = v;
}

static void create_charts(RRDHOST *host, RRDSET *st[CHARTS], RRDDIM *rd[CHARTS][DIMS], int update_every)
{
    char name[101];

    for (int i = 0; i < CHARTS; ++i) {
        snprintfz(name, sizeof(name) - 1, "dbengine-chart-%02d", i);

        st[i] = rrdset_create(
            host,
            "netdata",
            name,
            name,
            "netdata",
            NULL,
            "Unit Testing",
            "a value",
            "unittest",
            NULL,
            1,
            update_every,
            RRDSET_TYPE_LINE);
        rrdset_flag_set(st[i], RRDSET_FLAG_DEBUG);
        rrdset_flag_set(st[i], RRDSET_FLAG_STORE_FIRST);

        for (int j = 0; j < DIMS; ++j) {
            snprintfz(name, sizeof(name) - 1, "dim-%02d", j);

            rd[i][j] = rrddim_add(st[i], name, NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);

#if LOG_VALUES_TO_FILES
            char path[1024];
            snprintfz(path, 1024 - 1, "%s/%s.%s.log", netdata_configured_cache_dir, rrdset_id(st[i]), rrddim_id(rd[i][j]));

            rd[i][j]->fp = fopen(path, "w");
            if (!rd[i][j]->fp)
            {
                fprintf(stderr, "Could not open log file >>> %s <<<", path);
                abort();
            }
#endif
        }
    }

    for (int i = 0; i < CHARTS; ++i) {
        for (int j = 0; j < DIMS; ++j) {
            rd[i][j]->collector.last_collected_time.tv_sec = st[i]->last_collected_time.tv_sec =
                st[i]->last_updated.tv_sec = 2 * API_RELATIVE_TIME_MAX - 1;
            rd[i][j]->collector.last_collected_time.tv_usec = st[i]->last_collected_time.tv_usec =
                st[i]->last_updated.tv_usec = 0;
        }
    }

    for (int i = 0; i < CHARTS; ++i) {
        st[i]->usec_since_last_update = USEC_PER_SEC;

        for (int j = 0; j < DIMS; ++j) {
            rrddim_set_by_pointer_fake_time(rd[i][j], 69, 2 * API_RELATIVE_TIME_MAX); // set first value to 69
        }

        struct timeval now;
        now_realtime_timeval(&now);
        rrdset_timed_done(st[i], now, false);
    }

    for (int i = 0; i < CHARTS; ++i) {
        for (int j = 0; j < DIMS; ++j) {
            rrdeng_store_metric_flush_current_page((rd[i][j])->tiers[0].db_collection_handle);
        }
    }
}

static time_t
create_metrics(RRDSET *st[CHARTS], RRDDIM *rd[CHARTS][DIMS], int current_region, time_t time_start)
{
    int update_every = REGION_UPDATE_EVERY[current_region];
    time_t time_now = time_start;

    for (int i = 0; i < CHARTS; ++i)
    {
        for (int j = 0; j < DIMS; ++j)
        {
            storage_engine_store_change_collection_frequency(rd[i][j]->tiers[0].db_collection_handle, update_every);

            rd[i][j]->collector.last_collected_time.tv_sec = st[i]->last_collected_time.tv_sec =
                st[i]->last_updated.tv_sec = time_now;
            rd[i][j]->collector.last_collected_time.tv_usec = st[i]->last_collected_time.tv_usec =
                st[i]->last_updated.tv_usec = 0;
        }
    }

    for (int c = 0; c < REGION_POINTS[current_region]; ++c) {
        time_now += update_every; // time_now = start + (c + 1) * update_every

        for (int i = 0; i < CHARTS; ++i)
        {
            st[i]->usec_since_last_update = USEC_PER_SEC * update_every;

            for (int j = 0; j < DIMS; ++j)
            {
                collected_number next = ((collected_number)i * DIMS) * REGION_POINTS[current_region] + j * REGION_POINTS[current_region] + c;
                rrddim_set_by_pointer_fake_time(rd[i][j], next, time_now);
            }

            struct timeval now;
            now.tv_sec = time_now;
            now.tv_usec = 0;

            rrdset_timed_done(st[i], now, false);
        }
    }

    return time_now;
}

static void process_rd(int chart_index, RRDSET *st, int dimension_index, RRDDIM *rd, int region, time_t after, time_t before, long points)
{
    std::vector<collected_number> CNs;
    std::vector<NETDATA_DOUBLE> NDs;

    fprintf(stderr, "Found error in dimension: %s.%s\n", rrdset_id(st), rrddim_id(rd));
    fprintf(stderr, "region %d: [%ld, %ld)\n", region, after, before);

    // Gather the collected_numbers and the NETDATA_DOUBLEs the dimension
    // should have reported.
    for (long r = 0; r != REGIONS; r++) {
        for (long c = 0; c != REGION_POINTS[region]; c++)
        {
            collected_number cn = chart_index * DIMS * REGION_POINTS[r] + dimension_index * REGION_POINTS[r] + c;
            NETDATA_DOUBLE expected = unpack_storage_number(pack_storage_number((NETDATA_DOUBLE) cn, SN_DEFAULT_FLAGS));

            CNs.push_back(cn);
            NDs.push_back(cn);
        }
    }

    // Find the timestamps/values reported by dbengine for the problematic region
    struct storage_engine_query_handle seqh;
    storage_engine_query_init(rd->tiers[0].backend, rd->tiers[0].db_metric_handle, &seqh, after, before, STORAGE_PRIORITY_NORMAL);

    std::vector<std::pair<time_t, NETDATA_DOUBLE>> FoundNDs;
    while (!storage_engine_query_is_finished(&seqh)) {
        STORAGE_POINT SP = storage_engine_query_next_metric(&seqh);
        FoundNDs.push_back({ SP.start_time_s, SP.sum });
    }
    storage_engine_query_finalize(&seqh);

    // Print them
    for (long c = 0; c != REGION_POINTS[region]; c++)
    {
        collected_number cn = CNs[c];
        NETDATA_DOUBLE nd = NDs[c];

        if (c < FoundNDs.size())
        {
            fprintf(stderr, "@[%ld] CNs[%ld] = %lld, NDs[%ld] = %f, FoundNDs[%ld] = %f\n",
                    FoundNDs[c].first, c, CNs[c], c, NDs[c], c, FoundNDs[c].second);
        }
        else
        {
            fprintf(stderr, "CNs[%ld] = %lld, NDs[%ld] = %f, FoundNDs[%ld] = >>> MISSING <<<\n",
                    c, CNs[c], c, NDs[c], c);
        }
    }

    abort();
}

static int check_rrdr(
    RRDSET *st[CHARTS],
    RRDDIM *rd[CHARTS][DIMS],
    int current_region,
    time_t time_start,
    time_t time_end)
{
    int update_every = REGION_UPDATE_EVERY[current_region];
    fprintf(
        stderr,
        "%s() running on region %d, start time %lld, end time %lld, update every %d, on %d dimensions...\n",
        __FUNCTION__,
        current_region,
        (long long)time_start,
        (long long)time_end,
        update_every,
        CHARTS * DIMS);

    int value_errors = 0, time_errors = 0, value_right = 0, time_right = 0;

    int errors = 0;
    long points = (time_end - time_start) / update_every;

    for (int i = 0; i < CHARTS; ++i) {
        ONEWAYALLOC *owa = onewayalloc_create(0);
        RRDR *r = rrd2rrdr_legacy(
            owa,
            st[i],
            points,
            time_start,
            time_end,
            RRDR_GROUPING_AVERAGE,
            0,
            RRDR_OPTION_NATURAL_POINTS,
            NULL,
            NULL,
            0,
            0,
            QUERY_SOURCE_UNITTEST,
            STORAGE_PRIORITY_NORMAL);
        if (!r) {
            fprintf(
                stderr,
                "    DB-engine unittest %s: empty RRDR on region %d ### E R R O R ###\n",
                rrdset_name(st[i]),
                current_region);
            return ++errors;
        } else {
            assert(r->internal.qt->request.st == st[i]);
            for (long c = 0; c != (long)rrdr_rows(r); ++c) {
                time_t time_now = time_start + (c + 1) * update_every;
                time_t time_retrieved = r->t[c];

                // for each dimension
                void *dp = NULL;
                rrddim_foreach_read(dp, r->internal.qt->request.st)
                {
                    RRDDIM *d = static_cast<RRDDIM *>(dp);

                    if (unlikely(dp_dfe.counter >= r->d))
                        break; // d_counter is provided by the dictionary dfe

                    int j = (int)dp_dfe.counter;

                    NETDATA_DOUBLE *cn = &r->v[c * r->d];
                    NETDATA_DOUBLE value = cn[j];
                    assert(rd[i][j] == dp);

#if 1
                    collected_number last = i * DIMS * REGION_POINTS[current_region] + j * REGION_POINTS[current_region] + c;
#else
                    collected_number last = 0xAA;
#endif
                    NETDATA_DOUBLE expected = unpack_storage_number(pack_storage_number((NETDATA_DOUBLE)last, SN_DEFAULT_FLAGS));

                    uint8_t same = (roundndd(value) == roundndd(expected)) ? 1 : 0;

                    if (!same)
                    {
                        int chart_index = i;
                        int dimension_index = j;
                        process_rd(chart_index, st[i], dimension_index, d, current_region, time_start, time_end, points);
                    } else {
                        if (!same) {

                            if (value_errors < 20) {
                                fprintf(
                                    stderr,
                                    "[A] ### ERROR ### DB-engine unittest %s/%s: point #%ld, at %lu secs, expecting value %0.1f, RRDR found %0.1f\n",
                                    rrdset_name(st[i]),
                                    rrddim_name(rd[i][j]),
                                    (long)c + 1,
                                    (unsigned long)time_now,
                                    expected,
                                    value);
                            }
                            value_errors++;
                        } else
                            value_right++;

                        if (time_retrieved != time_now) {
                            if (time_errors < 20)
                                fprintf(
                                    stderr,
                                    "[B]  ### ERROR ### DB-engine unittest %s/%s: point #%ld at %lu secs, found RRDR timestamp %lu\n",
                                    rrdset_name(st[i]),
                                    rrddim_name(rd[i][j]),
                                    (long)c + 1,
                                    (unsigned long)time_now,
                                    (unsigned long)time_retrieved);
                            time_errors++;
                        } else
                            time_right++;
                    }
                }
                rrddim_foreach_done(dp);
            }
            rrdr_free(owa, r);
        }
        onewayalloc_destroy(owa);
    }

    if (value_errors)
        fprintf(stderr, "%d value errors encountered (%d were ok)\n", value_errors, value_right);

    if (time_errors)
        fprintf(stderr, "%d time errors encountered (%d were ok)\n", time_errors, value_right);

    return errors + value_errors + time_errors;
}

void check_charts_and_dims_are_not_collected(RRDSET *st[CHARTS], RRDDIM *rd[CHARTS][DIMS])
{
    for (int c = 0; c < CHARTS; c++) {
        st[c]->rrdcontexts.collected = false;
        for (int d = 0; d < DIMS; d++)
            rd[c][d]->rrdcontexts.collected = false;
    }
}

int test_dbengine(void)
{
    nd_log_limits_unlimited();

    default_rrd_memory_mode = RRD_MEMORY_MODE_DBENGINE;

    const char *host_name = "unittest-dbengine";
    RRDHOST *host = rrdhost_find_or_create(
        host_name,
        host_name,
        host_name,
        os_type,
        netdata_configured_timezone,
        netdata_configured_abbrev_timezone,
        netdata_configured_utc_offset,
        "",
        program_name,
        program_version,
        default_rrd_update_every,
        default_rrd_history_entries,
        RRD_MEMORY_MODE_DBENGINE,
        default_health_enabled,
        default_rrdpush_enabled,
        default_rrdpush_destination,
        default_rrdpush_api_key,
        default_rrdpush_send_charts_matching,
        default_rrdpush_enable_replication,
        default_rrdpush_seconds_to_replicate,
        default_rrdpush_replication_step,
        NULL,
        0);

    if (!host)
        return 1;

    int errors = 0;
    int value_errors = 0;
    int time_errors = 0;

    int current_region = 0;
    int update_every = REGION_UPDATE_EVERY[current_region];

    RRDSET *st[CHARTS];
    RRDDIM *rd[CHARTS][DIMS];
    create_charts(host, st, rd, update_every);

    time_t time_start[REGIONS];
    time_start[current_region] = 2 * API_RELATIVE_TIME_MAX;

    time_t time_end[REGIONS];
    time_end[current_region] = create_metrics(st, rd, current_region, time_start[current_region]);

    check_charts_and_dims_are_not_collected(st, rd);

    {
        current_region = 1;
        update_every = REGION_UPDATE_EVERY[current_region];

        // Align pages for frequency change
        for (int i = 0; i < CHARTS; ++i) {
            st[i]->update_every = update_every;
            for (int j = 0; j < DIMS; ++j) {
                rrdeng_store_metric_flush_current_page((rd[i][j])->tiers[0].db_collection_handle);
            }
        }

        time_start[current_region] = time_end[current_region - 1] + update_every;
        if (0 != time_start[current_region] % update_every) // align to update_every
            time_start[current_region] += update_every - time_start[current_region] % update_every;
        time_end[current_region] = create_metrics(st, rd, current_region, time_start[current_region]);

        check_charts_and_dims_are_not_collected(st, rd);
    }

    {
        current_region = 2;
        update_every = REGION_UPDATE_EVERY[current_region];

        // Align pages for frequency change
        for (int i = 0; i < CHARTS; ++i)
        {
            st[i]->update_every = update_every;
            for (int j = 0; j < DIMS; ++j) {
                rrdeng_store_metric_flush_current_page((rd[i][j])->tiers[0].db_collection_handle);
            }
        }

        time_start[current_region] = time_end[current_region - 1] + update_every;
        if (0 != time_start[current_region] % update_every) // align to update_every
            time_start[current_region] += update_every - time_start[current_region] % update_every;
        time_end[current_region] = create_metrics(st, rd, current_region, time_start[current_region]);

        check_charts_and_dims_are_not_collected(st, rd);
    }

    for (current_region = 0; current_region < REGIONS; ++current_region) {
        errors += check_rrdr(st, rd, current_region, time_start[current_region], time_end[current_region]);
    }

    if (errors)
        return 1;

    rrd_wrlock();
    rrdeng_prepare_exit((struct rrdengine_instance *)host->db[0].instance);
    rrdhost_delete_charts(host);
    rrdeng_exit((struct rrdengine_instance *)host->db[0].instance);
    rrd_unlock();

#ifdef LOG_VALUES_TO_FILES
    for (int i = 0; i != CHARTS; i++) {
        for (int j = 0; j != DIMS; j++) {
            fflush(rd[i][j]->fp);
            fclose(rd[i][j]->fp);
        }
    }
#endif

    return errors + value_errors + time_errors;
}
