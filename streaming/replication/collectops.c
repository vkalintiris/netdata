#include "daemon/common.h"

#ifdef  ENABLE_DBENGINE
#define MEM_PAGE_BLOCK_SIZE RRDENG_BLOCK_SIZE
#else
#define MEM_PAGE_BLOCK_SIZE 4096
#endif

static void flush_collected_metric_past_data(RRDDIM_PAST_DATA *dim_past_data){
#ifdef ENABLE_DBENGINE
    if(rrdeng_store_past_metrics_realtime(dim_past_data->rd, dim_past_data)){
        if(rrdeng_store_past_metrics_page_init(dim_past_data)){
            error("Cannot initialize db engine page: Flushing collected past data skipped!");
            return;
        }
        rrdeng_store_past_metrics_page(dim_past_data);
        rrdeng_flush_past_metrics_page(dim_past_data);
        rrdeng_store_past_metrics_page_finalize(dim_past_data);
        debug(D_REPLICATION, "Flushed Collected Past Metric %s.%s", dim_past_data->rd->rrdset->id, dim_past_data->rd->id);
    }

#else
    UNUSED(dim_past_data);
    error("Flushed Collected Past Metric is not supported for host %s. Replication Rx thread needs `dbengine` memory mode.",
          dim_past_data->rd->rrdset->host->hostname);
#endif
}

RRDDIM_PAST_DATA *replication_collect_past_metric_init(RRDHOST *host, const char *rrdset_id, const char *rrddim_id) {
    RRDDIM_PAST_DATA *dim_past_data = callocz(1, sizeof(RRDDIM_PAST_DATA));

    dim_past_data->page = callocz(1, MEM_PAGE_BLOCK_SIZE);
    dim_past_data->host = host;

    rrdhost_rdlock(dim_past_data->host);

    dim_past_data->st = rrdset_find(host, rrdset_id);
    if(unlikely(!dim_past_data->st)) {
        error("Cannot find chart with name_id '%s' on host '%s'.", rrdset_id, host->hostname);

        rrdhost_unlock(host);

        freez(dim_past_data->page);
        freez(dim_past_data);
        return NULL;
    }

    rrdset_rdlock(dim_past_data->st);

    dim_past_data->rd = rrddim_find(dim_past_data->st, rrddim_id);
    if(unlikely(!dim_past_data->rd)) {
        error("Cannot find dimension with id '%s' in chart '%s' on host '%s'.", rrddim_id, rrdset_id, host->hostname);

        rrdset_unlock(dim_past_data->st);
        rrdhost_unlock(dim_past_data->host);

        freez(dim_past_data->page);
        freez(dim_past_data);
        return NULL;
    }

    debug(D_REPLICATION, "Initializaton for collecting past data of dimension \"%s\".\"%s\"\n", rrdset_id, rrddim_id);
    return dim_past_data;
}

void replication_collect_past_metric(RRDDIM_PAST_DATA *dim_past_data, time_t timestamp, storage_number number) {
    storage_number *page = dim_past_data->page;
    uint32_t page_length = dim_past_data->page_length;

    if(!dim_past_data->rd) {
        error("Collect past metric: Dimension not found in the host");
        return;
    }

    time_t update_every = dim_past_data->rd->update_every;

    if(!page_length)
        dim_past_data->start_time = timestamp * USEC_PER_SEC;
    if((page_length + sizeof(number)) < MEM_PAGE_BLOCK_SIZE){
        if(page_length && dim_past_data->rd){
            time_t current_end_time = dim_past_data->end_time / USEC_PER_SEC;
            time_t t_sample_diff  = (timestamp -  current_end_time);
            if(t_sample_diff > update_every){
                page_length += ((t_sample_diff - update_every)*sizeof(number));
#ifdef NETDATA_INTERNAL_CHECKS
                error("Hard gap [%ld, %ld] = %ld was detected. Need to fill it with zeros up to page index %u", current_end_time, timestamp, t_sample_diff, page_length);
#endif
                if(page_length > MEM_PAGE_BLOCK_SIZE){
                    error("Page size is not enough to fill the hard gap.");
                    return;
                }
            }
        }
        page[page_length / sizeof(number)] = number;
        page_length += sizeof(number);
        dim_past_data->page_length = page_length;
        dim_past_data->end_time = timestamp * USEC_PER_SEC;
    }
    debug(D_REPLICATION, "Collect past metric sample#%u@%ld: "CALCULATED_NUMBER_FORMAT" \n", page_length, timestamp, unpack_storage_number(number));
}

void replication_collect_past_metric_done(RRDDIM_PAST_DATA *dim_past_data) {
    if(!dim_past_data->rd){
        error("Collect past metric: Dimension not found in the host");
        return;
    }
    flush_collected_metric_past_data(dim_past_data);

    rrdset_unlock(dim_past_data->st);
    rrdhost_unlock(dim_past_data->host);
}
