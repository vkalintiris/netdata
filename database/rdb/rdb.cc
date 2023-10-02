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

#include <sys/types.h>
#include <sys/time.h>
#include <sys/resource.h>

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

    se = storage_engine_get(RRD_MEMORY_MODE_RDB);
    si = reinterpret_cast<STORAGE_INSTANCE *>(NULL);

    size_t num_threads = 24;
    size_t num_groups = 5000;
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
