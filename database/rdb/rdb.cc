#include "database/rrd.h"
#include "libnetdata/locks/locks.h"
#include "libnetdata/storage_number/storage_number.h"
#include "rdb-private.h"
#include <google/protobuf/repeated_field.h>

#include <thread>
#include <chrono>

#include <iostream>
#include <condition_variable>
#include <mutex>
#include <thread>
#include <chrono>

#include <lz4.h>

#include "uuid_shard.h"

class Barrier
{
public:
    Barrier(int count) : thread_count(count), counter(0), waiting(0) { }

    void wait() {
        //fence mechanism
        std::unique_lock<std::mutex> lk(m);
        ++counter;
        ++waiting;
        cv.wait(lk, [&]{return counter >= thread_count;});
        cv.notify_one();

        --waiting;

        if(waiting == 0) {
           //reset barrier
           counter = 0;
        }

        lk.unlock();
    }

 private:
      std::mutex m;
      std::condition_variable cv;
      int thread_count;
      int counter;
      int waiting;
};

/*
 * buffer
*/

#include <google/protobuf/arena.h>

using namespace google::protobuf;

struct tier0_event {
    uint32_t id;
    uint32_t ts;
    storage_number sn;
    uint16_t update_every;
};

struct tier0_inflight_buffer {
    SPINLOCK spinlock;
    // std::mutex mutex;
    google::protobuf::Arena arena;
    RepeatedField<tier0_event> events;
};

static const size_t flush_every = 2500 * 240;

struct tier0_compaction_data {
    SPINLOCK spinlock;
    // std::mutex mutex;
    google::protobuf::Arena arena;
    RepeatedField<tier0_event> events;
};

tier0_compaction_data *t0_compaction_data;

void tier0_compaction_data_add(tier0_compaction_data &t0cd, const RepeatedField<tier0_event> &events) {
    // std::lock_guard<std::mutex> L(t0cd.mutex);
    spinlock_lock(&t0cd.spinlock);

    t0cd.events.MergeFrom(events);

    if (t0cd.events.size() >= (512 * flush_every)) {
        size_t input_size = t0cd.events.size() * sizeof(tier0_event);
        size_t max_compressed_size = LZ4_compressBound(input_size);
        char *compressed_buffer = Arena::CreateArray<char>(&t0cd.arena, max_compressed_size);

        int compressed_size = LZ4_compress_default(
            (const char *) t0cd.events.data(), compressed_buffer,
            input_size, max_compressed_size);

        netdata_log_error("Flusing events!");

        t0cd.events.Clear();
        t0_compaction_data->arena.Reset();
    }

    spinlock_unlock(&t0cd.spinlock);
}

void tier0_inflight_buffer_add(tier0_inflight_buffer &ib, tier0_event &event)
{
    // std::lock_guard<std::mutex> L(ib.mutex);

    spinlock_lock(&ib.spinlock);

    tier0_event *t0e = ib.events.Add();
    *t0e = event;

    if (ib.events.size() == flush_every) {
        size_t input_size = ib.events.size() * sizeof(tier0_event);
        size_t max_compressed_size = LZ4_compressBound(input_size);
        char *compressed_buffer = Arena::CreateArray<char>(&ib.arena, max_compressed_size);

        int compressed_size = LZ4_compress_default(
            (const char *) ib.events.data(), compressed_buffer,
            input_size, max_compressed_size);

        ib.events.Clear();
        ib.arena.Reset();
    }

    spinlock_unlock(&ib.spinlock);
}

std::vector<tier0_inflight_buffer> t0_inflight_buffers(8192);

/*
 * STORAGE_METRIC_HANDLE
*/

static class UuidShard<rdb_metric_handle> pmetrics(10);

STORAGE_METRIC_HANDLE *rdb_metric_get(STORAGE_INSTANCE *si, uuid_t *uuid)
{
    UNUSED(si);

    rdb_metric_handle *rmh = pmetrics.acquire(*uuid);
    return (STORAGE_METRIC_HANDLE *) rmh;
}

// FIXME:
STORAGE_METRIC_HANDLE *rdb_metric_get_or_create(RRDDIM *rd, STORAGE_INSTANCE *si)
{
    UNUSED(si);

    rdb_metric_handle *rmh = pmetrics.add_or_create(rd->metric_uuid);
    return (STORAGE_METRIC_HANDLE *) rmh;
}

STORAGE_METRIC_HANDLE *rdb_metric_dup(STORAGE_METRIC_HANDLE *smh)
{
    rdb_metric_handle *rmh = (rdb_metric_handle *) smh;
    pmetrics.acquire(rmh);
    return smh;
}

void rdb_metric_release(STORAGE_METRIC_HANDLE *smh)
{
    rdb_metric_handle *rmh = (rdb_metric_handle *) smh;
    pmetrics.release(rmh);
}

bool rdb_metric_retention_by_uuid(STORAGE_INSTANCE *si, uuid_t *uuid, time_t *first_entry_s, time_t *last_entry_s) {
    UNUSED(si);
    UNUSED(uuid);
    UNUSED(first_entry_s);
    UNUSED(last_entry_s);

    fatal("Not implemented yet.");

    return false;
}

/*
 * STORAGE_METRICS_GROUP
*/

STORAGE_METRICS_GROUP *rdb_metrics_group_get(STORAGE_INSTANCE *si, uuid_t *uuid) {
    UNUSED(si);
    UNUSED(uuid);

    rdb_metrics_group *rmg = new rdb_metrics_group();
    rmg->rc = 0;
    return (STORAGE_METRICS_GROUP *) rmg;
}

void rdb_metrics_group_release(STORAGE_INSTANCE *si, STORAGE_METRICS_GROUP *smg) {
    UNUSED(si);

    rdb_metrics_group *rmg = (rdb_metrics_group *) smg;
    if(__atomic_sub_fetch(&rmg->rc, 1, __ATOMIC_SEQ_CST) == 0)
        delete rmg;
}

// /*
//  * STORAGE_COLLECT_HANDLE
// */

struct rdb_collect_handle {
    struct storage_collect_handle common; // has to be first item
    rdb_metric_handle *rmh;
    tier0_inflight_buffer *ib;
    RepeatedField<storage_number> sns;
};

STORAGE_COLLECT_HANDLE *rdb_store_metric_init(STORAGE_METRIC_HANDLE *smh, uint32_t update_every, STORAGE_METRICS_GROUP *smg)
{
    rdb_collect_handle *rch = new rdb_collect_handle();

    rch->common.backend = STORAGE_ENGINE_BACKEND_RDB;
    rch->rmh = (rdb_metric_handle *) rdb_metric_dup(smh);
    rch->ib = &t0_inflight_buffers[gettid() % t0_inflight_buffers.size()];

    rch->sns.Reserve(1024);

    UNUSED(update_every);
    UNUSED(smg);

    return (STORAGE_COLLECT_HANDLE *) rch;
}

void rdb_store_metric_next(STORAGE_COLLECT_HANDLE *sch, usec_t point_in_time,
                           NETDATA_DOUBLE n, NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                           uint16_t count, uint16_t anomaly_count, SN_FLAGS flags)
{
    UNUSED(sch);
    UNUSED(point_in_time);
    UNUSED(n);
    UNUSED(min_value);
    UNUSED(max_value);
    UNUSED(count);
    UNUSED(anomaly_count);
    UNUSED(flags);

    rdb_collect_handle *rch = (rdb_collect_handle *) sch;

    spinlock_lock(&rch->ib->spinlock);

    storage_number *ptr = rch->sns.Add();
    *ptr = pack_storage_number(n, flags);

    if (rch->sns.size() == 1024) {
        rch->sns.Clear();
    }

    spinlock_unlock(&rch->ib->spinlock);

    // tier0_event t0e = {
    //     .id = rch->rmh->id,
    //     .ts = static_cast<uint32_t>(point_in_time / USEC_PER_SEC),
    //     .sn = pack_storage_number(n, flags),
    //     .update_every = 1,
    // };

    // tier0_inflight_buffer_add(*rch->ib, t0e);
}

void rdb_store_metric_flush(STORAGE_COLLECT_HANDLE *sch) {
    UNUSED(sch);
}

int rdb_store_metric_finalize(STORAGE_COLLECT_HANDLE *sch) {
    rdb_collect_handle *rch = (rdb_collect_handle *) sch;
    delete rch;

    return 0;
}

/* benchmark

*/

static STORAGE_ENGINE *se = nullptr;
static STORAGE_INSTANCE *si = nullptr;

typedef struct {
    STORAGE_METRICS_GROUP *smg;
    STORAGE_METRIC_HANDLE *smh;
    STORAGE_COLLECT_HANDLE *sch;
    RRDDIM rd;
} dimension_t;


static void gen_random_dimensions(std::vector<dimension_t> &dimensions,
                                  size_t num_groups,
                                  size_t num_dims_per_group)
{
    dimensions.reserve(num_groups * num_dims_per_group);

    for (size_t i = 0; i != num_groups; i++) {
        uuid_t smg_uuid;
        uuid_generate(smg_uuid);

        STORAGE_METRICS_GROUP *smg = storage_engine_metrics_group_get(STORAGE_ENGINE_BACKEND_RDB, si, &smg_uuid);

        for (size_t j = 0; j != num_dims_per_group; j++) {
            dimension_t d;

            uuid_generate(d.rd.metric_uuid);
            d.smh = se->api.metric_get_or_create(&d.rd, si);
            d.sch = storage_metric_store_init(STORAGE_ENGINE_BACKEND_RDB, d.smh, 1, smg);
            d.smg = smg;

            dimensions.push_back(d);
        }
    }
}

static void gen_random_data(std::vector<dimension_t> &dimensions, size_t num_points_per_dimension, usec_t point_in_time)
{

    for (size_t i = 0; i != num_points_per_dimension; i++) {
        for (size_t j = 0; j != dimensions.size(); j++) {
            storage_engine_store_metric(dimensions[j].sch, point_in_time, i, 0, 0, 1, 0, SN_DEFAULT_FLAGS);
        }
        point_in_time += USEC_PER_SEC;
    }

    for (size_t i = 0; i != dimensions.size(); i++) {
        storage_engine_store_flush(dimensions[i].sch);
    }
}

static Barrier *B = nullptr;

static void gen_thread(size_t thread_id, size_t num_threads, size_t num_groups, size_t num_dims_per_group, size_t num_points_per_dimension) {
    UNUSED(num_threads);
    
    char thread_name[128];
    snprintfz(thread_name, 1024, "genthread-%04zu", thread_id);
    pthread_setname_np(pthread_self(), thread_name);

    std::vector<dimension_t> dimensions;
    gen_random_dimensions(dimensions, num_groups, num_dims_per_group);

    // shift each thread's entries so that we can avoid compressing all threads
    // at the same point in time
    usec_t point_in_time = (now_realtime_sec() - (365 * 24 * 3600)) * USEC_PER_SEC;
    for (size_t i = 0; i != thread_id; i++) {
        for (size_t j = 0; j != dimensions.size(); j++) {
            storage_engine_store_metric(dimensions[j].sch, point_in_time, i, 0, 0, 1, 0, SN_DEFAULT_FLAGS);
        }
        point_in_time += USEC_PER_SEC;
    }

    B->wait();
    
    gen_random_data(dimensions, num_points_per_dimension, point_in_time);
}

int rdb_main(int argc, char *argv[]) {
    (void) argc;
    (void) argv;

    netdata_log_error("Program started...");

    t0_compaction_data = new tier0_compaction_data();
    spinlock_init(&t0_compaction_data->spinlock);
    
    for (tier0_inflight_buffer &ib : t0_inflight_buffers) {
        spinlock_init(&ib.spinlock);
        ib.events.Reserve(flush_every);
    }

    se = storage_engine_get(RRD_MEMORY_MODE_RDB);
    si = reinterpret_cast<STORAGE_INSTANCE *>(NULL);

    size_t num_threads = 512;
    size_t num_groups = 500;
    size_t num_dims_per_group = 5;
    size_t num_points_per_dimension = 4 * 3600;

    std::vector<std::thread> threads;

    {
        Barrier Bar(num_threads + 1);
        B = &Bar;

        auto start_time = std::chrono::high_resolution_clock::now();
        for (size_t i = 0; i < num_threads; ++i)
            threads.emplace_back(gen_thread, i, num_threads, num_groups, num_dims_per_group, num_points_per_dimension);

        B->wait();
    
        auto end_time = std::chrono::high_resolution_clock::now();
        auto duration = std::chrono::duration_cast<std::chrono::milliseconds>(end_time - start_time);
        double seconds = duration.count() / static_cast<double>(MSEC_PER_SEC);
        netdata_log_error("Time to setup metrics: %.2lf seconds", seconds);
    }

    auto start_time = std::chrono::high_resolution_clock::now();

    for (std::thread& thread : threads)
        thread.join();

    auto end_time = std::chrono::high_resolution_clock::now();
    auto duration = std::chrono::duration_cast<std::chrono::milliseconds>(end_time - start_time);
    double seconds = duration.count() / static_cast<double>(MSEC_PER_SEC);
    netdata_log_error("Overall execution time: %.2lf seconds", seconds);

    netdata_log_error("Test config: threads=%zu, groups=%zu, dims_per_group=%zu, points_per_dimension=%zu)",
                      num_threads, num_groups, num_dims_per_group, num_points_per_dimension);

    size_t total_points = num_threads * num_groups * num_dims_per_group * num_points_per_dimension;
    netdata_log_error("Points written: %zu", total_points);

    size_t total_bytes = total_points * sizeof(storage_number) ;
    double total_mib = static_cast<double>(total_bytes) / (1024 * 1024);
    netdata_log_error("MiB written: %.2lf", total_mib);

    double points_per_sec = static_cast<double>(total_points) / seconds;
    netdata_log_error("Points per second: %.2lf", points_per_sec);

    double bytes_per_sec =  static_cast<double>(total_bytes) / seconds;
    double mib_per_sec = static_cast<double>(bytes_per_sec) / (1024.0 * 1024.0);
    netdata_log_error("MiB per second: %.2lf", mib_per_sec);

    double pages_per_sec = points_per_sec / 1024.0;
    netdata_log_error("pages per second: %.2lf", pages_per_sec);

    exit(EXIT_SUCCESS);
}
