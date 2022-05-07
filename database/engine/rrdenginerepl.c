// SPDX-License-Identifier: GPL-3.0-or-later
#include "rrdengine.h"

//It updates the dim_past_data according to new start and end times
static void modify_dim_past_data(RRDDIM_PAST_DATA *dim_past_data, usec_t start_time, usec_t end_time)
{
    uint64_t start, end, new_start, new_end, new_entries;
    start = dim_past_data->start_time / USEC_PER_SEC; //gap time start
    end = dim_past_data->end_time / USEC_PER_SEC;     //gap time end
    new_start = start_time / USEC_PER_SEC;
    new_end = end_time / USEC_PER_SEC;
    new_entries = (uint64_t)(new_end - new_start) / dim_past_data->rd->update_every + 1;
#ifndef NETDATA_INTERNAL_CHECKS
    UNUSED(end);
#endif

    dim_past_data->page_length = new_entries * sizeof(storage_number);
    if(new_start != start) {
        storage_number *sn = dim_past_data->page;
        dim_past_data->page = &sn[((new_start - start) / dim_past_data->rd->update_every) * sizeof(storage_number)];
    }
    dim_past_data->start_time = new_start * USEC_PER_SEC;
    dim_past_data->end_time = new_end * USEC_PER_SEC;

    error(
        "Divided page %p - [%llu, %llu, %lu, %lu, %u, %lu]",
        dim_past_data->page,
        dim_past_data->start_time,
        dim_past_data->end_time,
        start,
        end,
        dim_past_data->page_length,
        ((new_start - start) / dim_past_data->rd->update_every) * sizeof(storage_number));
}

int rrdeng_store_past_metrics_page_init(RRDDIM_PAST_DATA *dim_past_data) {
    /* Create a page */
    // struct rrdeng_collect_handle *rep_handle;
    struct pg_cache_page_index *page_index;
    struct rrdengine_instance *ctx;
    RRDDIM *rd = dim_past_data->rd;
    RRDHOST *host = rd->rrdset->rrdhost;

    // rep_handle = &rd->state->handle.rrdeng;
    ctx = host->rrdeng_ctx;
    dim_past_data->ctx = ctx;
    page_index = rd->state->page_index;

    // rep_handle->prev_descr = NULL;
    // rep_handle->unaligned_page = 0;
    uv_rwlock_wrlock(&page_index->lock);
    ++page_index->writers;
    uv_rwlock_wrunlock(&page_index->lock);

    // create a dbengine page
    void *page = rrdeng_create_page(ctx, &page_index->id, &dim_past_data->descr);
    fatal_assert(page);
    error("A dbengine page is created to store past data for dimension \"%s\".\"%s\".", rd->rrdset->id, rd->id);
    return 0;
}

void rrdeng_store_past_metrics_page(RRDDIM_PAST_DATA *dim_past_data) {
    struct page_cache *pg_cache;
    struct rrdengine_instance *ctx;
    struct rrdeng_page_descr *descr;
    RRDDIM *rd = dim_past_data->rd;
#ifndef NETDATA_INTERNAL_CHECKS
    UNUSED(rd);
#endif

    descr = dim_past_data->descr;
    ctx = dim_past_data->ctx;
    pg_cache = &ctx->pg_cache;

    // copy the dim past dataq in this page
    rrdeng_page_descr_mutex_lock(ctx, descr);
    memcpy(descr->pg_cache_descr->page, dim_past_data->page, (size_t)dim_past_data->page_length);
    descr->page_length  = dim_past_data->page_length;
    descr->end_time = dim_past_data->end_time;
    descr->start_time = dim_past_data->start_time;
    // Page alignment can be handled with zero values.
    // Every new past data page can reach the aligment with zeros if the values are not enough.
    // for (page_length - rd->rrdset->rrddim_page_alignment) fill with zeros OR
    // simply increase the length since zeros are already there
    rrdeng_page_descr_mutex_unlock(ctx, descr);

    error("REP: Page correlation ID and page info updates....");
    // prepare the pg descr to insert and commit the dbengine page
    dim_past_data->page_correlation_id = rrd_atomic_fetch_add(&pg_cache->committed_page_index.latest_corr_id, 1);
    pg_cache_atomic_set_pg_info(descr, descr->end_time, descr->page_length);
    error("Past \"%s\".\"%s\" metrics page is ready for commit in memory.", rd->rrdset->id, rd->id);
}

void rrdeng_flush_past_metrics_page(RRDDIM_PAST_DATA *dim_past_data) {
    struct rrdengine_instance *ctx;
    struct rrdeng_page_descr *descr;
    struct pg_cache_page_index *page_index;
    RRDDIM *rd;
#ifndef NETDATA_INTERNAL_CHECKS
    UNUSED(rd);
#endif

    descr = dim_past_data->descr;
    ctx = dim_past_data->ctx;
    page_index = dim_past_data->rd->state->page_index;
    rd = dim_past_data->rd;

    error("Inserting page in dbengine....");
    unsigned long new_metric_API_producers, old_metric_API_max_producers, ret_metric_API_max_producers;
    new_metric_API_producers = rrd_atomic_add_fetch(&ctx->stats.metric_API_producers, 1);
    while (unlikely(new_metric_API_producers > (old_metric_API_max_producers = ctx->metric_API_max_producers))) {
        ret_metric_API_max_producers = ulong_compare_and_swap(&ctx->metric_API_max_producers,
                                                                old_metric_API_max_producers,
                                                                new_metric_API_producers);
        if (old_metric_API_max_producers == ret_metric_API_max_producers) {
            break;
        }
    }
    // page flags check. Need to enable the pg_cache_descr_state flags
    pg_cache_insert(ctx, page_index, descr);
    // Try to update the time start and end of the metric.
    pg_cache_add_new_metric_time(page_index, descr);

    error("Commiting page in dbengine....");
    if (likely(descr->page_length)) {
        int page_is_empty;

        rrd_stat_atomic_add(&ctx->stats.metric_API_producers, -1);

        page_is_empty = rrdeng_page_has_only_empty_metrics(descr);
        if (page_is_empty) {
            error("Past metrics page has empty metrics only, deleting:");
            pg_cache_put(ctx, descr);
            pg_cache_punch_hole(ctx, descr, 1, 0, NULL);
        } else {
            rrdeng_commit_page(ctx, descr, dim_past_data->page_correlation_id);
        }
    } else {
        freez(descr->pg_cache_descr->page);
        rrdeng_destroy_pg_cache_descr(ctx, descr->pg_cache_descr);
        freez(descr);
    }
    error("Page Commited -  Dimension \"%s\".\"%s\" metrics page commited in memory.", rd->rrdset->id, rd->id);
    // TBR: Only for debug
    error("REP: OBSERVE dimension (%s.%s) in time_interval[%ld, %ld] #samples(%lu)....END", rd->rrdset->id, rd->id, (time_t)(descr->start_time/USEC_PER_SEC), (time_t)(descr->end_time/USEC_PER_SEC), (descr->page_length/sizeof(storage_number)));
}

void rrdeng_store_past_metrics_page_finalize(RRDDIM_PAST_DATA *dim_past_data){
    struct pg_cache_page_index* page_index;
    RRDDIM *rd = dim_past_data->rd;
    page_index = dim_past_data->rd->state->page_index;
#ifndef NETDATA_INTERNAL_CHECKS
    UNUSED(rd);
#endif

    uv_rwlock_wrlock(&page_index->lock);
    --page_index->writers;
    uv_rwlock_wrunlock(&page_index->lock);
    error("Finalize operation -  Dimension \"%s\".\"%s\" metrics page completed.", rd->rrdset->id, rd->id);
}

// It saves the GAP past metrics in the active page in real-time
int rrdeng_store_past_metrics_realtime(RRDDIM *rd, RRDDIM_PAST_DATA *dim_past_data)
{
    struct rrdeng_collect_handle *handle = (struct rrdeng_collect_handle *)rd->state->handle;
    struct rrdeng_page_descr *descr;
    storage_number *page, *page_gap;
    int return_value = 0;

    descr = handle->descr;
    if(!descr || !descr->pg_cache_descr) {
        infoerr("No active descr or page for dimension %s.%s", rd->rrdset->id, rd->id);
        return 1;
    }

    page = (storage_number *)descr->pg_cache_descr->page;
    page_gap = (storage_number *)dim_past_data->page;

    uint64_t start, end, page_start, page_end;
    start = dim_past_data->start_time / USEC_PER_SEC; //gap time start
    end = dim_past_data->end_time / USEC_PER_SEC;     //gap time end
    page_start = descr->start_time / USEC_PER_SEC;    //active page time start
    page_end = descr->end_time / USEC_PER_SEC;        //active page time start

    if (!page || !page_gap || start > end) {
        info(
            "Active page %p - [%lu, %lu] and GAP page %p - [%lu, %lu] problems",
            page,
            page_start,
            page_end,
            page_gap,
            start,
            end);
        return 0;
    }

    if(page_end < end)
        modify_dim_past_data(dim_past_data, start * USEC_PER_SEC, (page_end) * USEC_PER_SEC);

    uint64_t entries_gap = (dim_past_data->page_length / sizeof(storage_number)); // num of samples
    uint64_t entries_page = (descr->page_length / sizeof(storage_number));    // num of samples
    uint64_t ue_page = rd->update_every;
    uint64_t gap_start_offset = 0;
    uint64_t page_start_offset = 0;

    if (!ue_page) {
        info(
            "Active page %p - [%lu, %lu] has no samples(%lu) for %s.%s",
            page,
            page_start,
            page_end,
            entries_page,
            rd->rrdset->id,
            rd->id);
        return 0;
    }

    if(end < page_start){
        return 1;
    }

    if (page_start > start) {
        gap_start_offset = (uint64_t)(page_start - start) / ue_page - 1;
        return_value = 1;
    }
    if (page_start < start) {
        return_value = 0;
        page_start_offset = (uint64_t)(start - page_start) / ue_page - 1;
    }
    if (page_start == start) {
        return_value = 0;
        page_start_offset = 0;
    }

    error("Just before memcpy");
    void *dest = (void *)(page + page_start_offset);
    void *src = (void *)(page_gap + gap_start_offset);
    size_t size = ((entries_gap - gap_start_offset) * sizeof(storage_number));
    error("page[%lu]=%p, page_gap[%lu]=%p, size: %lu", page_start_offset, dest, gap_start_offset, src, size);
    memcpy(dest, src, size);
    error("Successfully updated the active page for %s.%s", rd->rrdset->id, rd->id);

    if(return_value)
        modify_dim_past_data(dim_past_data, start * USEC_PER_SEC, (page_start - 1) * USEC_PER_SEC);

    return return_value;
}
