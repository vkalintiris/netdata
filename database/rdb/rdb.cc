#include "rocksdb/compression_type.h"
#include "rocksdb/options.h"
#include <chrono>
#include <condition_variable>
#include <iostream>
#include <mutex>
#include <thread>

#include <sys/types.h>
#include <sys/time.h>
#include <sys/resource.h>

#include <google/protobuf/repeated_field.h>
#include <lz4.h>

#include "rdb-private.h"
#include "uuid_shard.h"
#include "si.h"

#include <rocksdb/db.h>
#include <rocksdb/statistics.h>

StorageInstance *SI = nullptr;
rocksdb::DB *RDB = nullptr;



// Function to get the RSS in bytes
std::size_t getRSSBytes() {
    struct rusage rusage;
    getrusage(RUSAGE_SELF, &rusage);
    return rusage.ru_maxrss * 1024;  // ru_maxrss is in kilobytes
}

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
            storage_engine_store_metric(dimensions[j].sch, point_in_time, i % 1111, 0, 0, 1, 0, SN_DEFAULT_FLAGS);
        }
        point_in_time += USEC_PER_SEC;

        RDB->Flush(rocksdb::FlushOptions());
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

#include <rocksdb/options.h>
#include <rocksdb/advanced_options.h>

rocksdb::DB *open_kv_db(const char *path) {
    rocksdb::Options options;

    options.create_if_missing = true;
    // options.enable_blob_files = true;
    // options.min_blob_size = 1024;
    options.target_file_size_base = 1024 * 1024 * 1024;
    options.max_bytes_for_level_base = 10 * options.target_file_size_base; 

    options.write_buffer_size = 512 * 1024 * 1024;
    options.max_write_buffer_number = 5;

    options.writable_file_max_buffer_size = 1024 * 1024 * 1024;
    options.min_write_buffer_number_to_merge = 2;
    
    options.max_background_flushes = 32;
    options.max_background_compactions = 32;
    options.statistics = rocksdb::CreateDBStatistics();
    options.stats_dump_period_sec = 1;
    options.manual_wal_flush = true;

    options.allow_concurrent_memtable_write = true;
    options.enable_write_thread_adaptive_yield = true;

    rocksdb::DB* db;
    rocksdb::Status S = rocksdb::DB::Open(options, path, &db);
    if (!S.ok())
        fatal("Failed to open db: %s", S.ToString().c_str());

    // dbopts.manual_wal_flush


    return db;
}

int rdb_main(int argc, char *argv[]) {
    (void) argc;
    (void) argv;

    SI = new StorageInstance(16);
    RDB = open_kv_db("/home/cm/opt/tmp");

    netdata_log_error("Program started...");

    se = storage_engine_get(RRD_MEMORY_MODE_RDB);
    si = reinterpret_cast<STORAGE_INSTANCE *>(NULL);

    size_t num_threads = 16;
    size_t num_groups = 500;
    size_t num_dims_per_group = 5;
    size_t num_points_per_dimension = 24 * 3600;

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

    rocksdb::FlushOptions FO;
    FO.allow_write_stall = true;
    FO.wait = true;
    
    RDB->Flush(FO);
    RDB->SyncWAL();
    RDB->Close();
    delete RDB;

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
