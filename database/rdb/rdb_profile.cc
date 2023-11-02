#include "rdb-private.h"
#include "Barrier.h"

#include "rocksdb/advanced_options.h"
#include <chrono>
#include <thread>

#include "rocksdb/cache.h"
#include "rocksdb/table.h"

rdb::StorageInstance *SI = nullptr;
STORAGE_INSTANCE *RDB_StorageInstance = nullptr;

std::atomic<size_t> NumFlushedPages = 0;

// Function to get the RSS in bytes
std::size_t getRSS() {
    struct rusage rusage;
    getrusage(RUSAGE_SELF, &rusage);
    return rusage.ru_maxrss / 1024;  // ru_maxrss is in kilobytes
}

static std::vector<uint32_t> genRandVector(size_t n) {
    std::random_device rd;
    std::mt19937 gen(rd());
    std::uniform_int_distribution<uint32_t> dis(0, std::numeric_limits<uint32_t>::max());

    std::vector<uint32_t> v;
    v.reserve(n);

    for (int i = 0; i < n; ++i)
        v.push_back(dis(gen));

    return v;
}

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
            d.smh = se->api.metric_get_or_create(si, smg, &d.rd);
            d.sch = storage_metric_store_init(STORAGE_ENGINE_BACKEND_RDB, d.smh, 1, smg);
            d.smg = smg;

            dimensions.push_back(d);
        }
    }
}

static void free_random_dimensions(std::vector<dimension_t> &dimensions)
{
    for (size_t i = 0; i != dimensions.size(); i++)
    {
        storage_engine_store_finalize(dimensions[i].smh, dimensions[i].sch);
        se->api.metric_release(dimensions[i].smh);
    }
}

static void gen_random_data(std::vector<dimension_t> &dimensions, size_t num_points_per_dimension, usec_t point_in_time, const std::vector<uint32_t> &rand_vals)
{
    for (size_t i = 0; i != num_points_per_dimension; i++)
    {
        for (size_t j = 0; j != dimensions.size(); j++)
        {
            // uint32_t val = rand_vals[(i + j) % 256];
            storage_engine_store_metric(dimensions[j].smh, dimensions[j].sch, point_in_time, 0, 0, 0, 1, 0, SN_DEFAULT_FLAGS);
        }

        point_in_time += USEC_PER_SEC;
    }

    for (size_t i = 0; i != dimensions.size(); i++)
        storage_engine_store_flush(dimensions[i].smh, dimensions[i].sch);
}

static Barrier *B = nullptr;


static void gen_thread(size_t thread_id,
                       size_t num_threads,
                       size_t num_groups,
                       size_t num_dims_per_group,
                       size_t num_points_per_dimension,
                       const std::vector<uint32_t> &rand_vals)
{
    UNUSED(num_threads);
    
    char thread_name[128];
    snprintfz(thread_name, 1024, "genthread-%04zu", thread_id);
    pthread_setname_np(pthread_self(), thread_name);

    std::vector<dimension_t> dimensions;
    gen_random_dimensions(dimensions, num_groups, num_dims_per_group);
    B->wait();

    usec_t point_in_time = 0x000000FF * USEC_PER_SEC;
    gen_random_data(dimensions, num_points_per_dimension, point_in_time, rand_vals);
    
    std::this_thread::sleep_for(std::chrono::seconds{1});
    netdata_log_error("Will free dimensions");

    free_random_dimensions(dimensions);
}

static rocksdb::Options get_level_db_options()
{
    rocksdb::Options Opts;

    Opts.create_if_missing = true;
    
    Opts.compaction_style = rocksdb::kCompactionStyleLevel;
    
    Opts.write_buffer_size = 128 * 1024 * 1024;
    Opts.target_file_size_base = 256 * 1024 * 1024;
    Opts.target_file_size_multiplier = 10;

    Opts.enable_blob_files = true;
    Opts.min_blob_size = 64;
    
    Opts.manual_wal_flush = true;
    return Opts;
}

void rdb_init() {
    SI = new rdb::StorageInstance();
    RDB_StorageInstance = reinterpret_cast<STORAGE_INSTANCE *>(SI);
    
    rocksdb::Options Opts = get_level_db_options();

    #if 0
    char Path[4096] = { };
    snprintf(Path, 4096 - 1, "%s/rdb", netdata_configured_cache_dir);
    #else
    const char *Path = "/mnt/tmpfs";
    #endif

    rocksdb::Status S = SI->open(Opts, Path);
    if (!S.ok())
        fatal("Could not open db at '%s': %s", Path, S.ToString().c_str());
}

void rdb_flush()
{
    SI->RDB->Flush(rocksdb::FlushOptions());
}

void oldestKey()
{
    rocksdb::Iterator *It = SI->getIteratorMD(rocksdb::ReadOptions());

    It->SeekToFirst();
    if (!It->Valid()) {
        fatal("Could not seek to first key!");
    }
    
    rdb::Key K = It->key();
    netdata_log_error("Oldest key: %s", K.toString(true).c_str());

    delete It;
}

int rdb_profile_main(int argc, char *argv[])
{
    (void) argc;
    (void) argv;

    rdb_init();

    se = storage_engine_get(RRD_MEMORY_MODE_RDB);
    si = reinterpret_cast<STORAGE_INSTANCE *>(NULL);

    size_t num_threads = 16;
    size_t num_groups = 500;
    size_t num_dims_per_group = 5;
    size_t num_points_per_dimension = 6 * 3600;

    netdata_log_error("Test simulating %zu agents: threads=%zu, groups=%zu, dims_per_group=%zu, points_per_dimension=%zu)",
                      (num_threads * num_groups * num_dims_per_group) / 2500,
                      num_threads, num_groups, num_dims_per_group, num_points_per_dimension);

    // std::vector<uint32_t> rand_vals = genRandVector(1024 * 1024);
    std::vector<uint32_t> rand_vals;

    std::vector<std::thread> threads;
    {
        netdata_log_error("Setting up metrics... (RSS: %zu MiB)", getRSS());

        Barrier Bar(num_threads + 1);
        B = &Bar;

        auto start_time = std::chrono::high_resolution_clock::now();

        for (size_t i = 0; i < num_threads; ++i)
            threads.emplace_back(gen_thread, i, num_threads, num_groups, num_dims_per_group, num_points_per_dimension, rand_vals);

        B->wait();
    
        auto end_time = std::chrono::high_resolution_clock::now();
        auto duration = std::chrono::duration_cast<std::chrono::milliseconds>(end_time - start_time);
        double seconds = duration.count() / static_cast<double>(MSEC_PER_SEC);
        netdata_log_error("Time to setup metrics: %.2lf seconds (RSS: %zu MiB)", seconds, getRSS());
    }

    {
        auto start_time = std::chrono::high_resolution_clock::now();
        while (true)
        {
            size_t PrevNumFlushedPages = NumFlushedPages;
            std::this_thread::sleep_for(std::chrono::seconds{1});
            auto end_time = std::chrono::high_resolution_clock::now();

            auto duration = std::chrono::duration_cast<std::chrono::milliseconds>(end_time - start_time);
            double seconds = duration.count() / static_cast<double>(MSEC_PER_SEC);
            
            double pages_per_second = static_cast<double>(NumFlushedPages) / seconds;
            double points_per_sec = pages_per_second * 1024;
            double mib_per_sec = (points_per_sec * sizeof(storage_number)) / (1024.0 * 1024.0);

            double capacity = points_per_sec / 2500.0;

            netdata_log_error("pages/sec: %.1lf, points/sec: %.1lf, mib/sec: %.1lf, capacity: %.1lf (RSS: %zu MiB)",
                              pages_per_second, points_per_sec, mib_per_sec, capacity, getRSS());

            SI->RDB->Flush(rocksdb::FlushOptions());
            oldestKey();

            if (PrevNumFlushedPages == NumFlushedPages)
            {
                break;
            }
        }
    }

    netdata_log_error("Collection threads finished!");

    for (std::thread& thread : threads)
        thread.join();

    SI->RDB->Flush(rocksdb::FlushOptions());
    SI->close();
    exit(EXIT_SUCCESS);
}
