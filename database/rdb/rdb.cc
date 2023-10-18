#include "rdb-private.h"

#include <chrono>
#include <thread>

rdb::StorageInstance *SI = nullptr;

std::atomic<size_t> num_pages_written = 0;

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
            d.smh = se->api.metric_get_or_create(&d.rd, si);
            d.sch = storage_metric_store_init(STORAGE_ENGINE_BACKEND_RDB, d.smh, 1, smg);
            d.smg = smg;

            dimensions.push_back(d);
        }
    }
}

static void gen_random_data(std::vector<dimension_t> &dimensions, size_t num_points_per_dimension, usec_t point_in_time, const std::vector<uint32_t> &rand_vals)
{
    for (size_t i = 0; i != num_points_per_dimension; i++)
    {
        for (size_t j = 0; j != dimensions.size(); j++)
        {
            uint32_t val = rand_vals[(i + j) % rand_vals.size()];
            netdata_log_error("Adding value: %u", val);
            storage_engine_store_metric(dimensions[j].sch, point_in_time, val, 0, 0, 1, 0, SN_DEFAULT_FLAGS);
        }

        point_in_time += USEC_PER_SEC;
    }

    for (size_t i = 0; i != dimensions.size(); i++)
        storage_engine_store_flush(dimensions[i].sch);
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
}

static rocksdb::Options get_db_options()
{
    rocksdb::Options Opts;

    Opts.create_if_missing = true;
    Opts.statistics = rocksdb::CreateDBStatistics();
    Opts.compaction_style = rocksdb::kCompactionStyleFIFO;
    Opts.write_buffer_size = 512 * 1024 * 1024;
    Opts.target_file_size_base = 32 * 1024 * 1024;
    Opts.max_bytes_for_level_base = 10 * Opts.target_file_size_base; 
    Opts.manual_wal_flush = true;
    Opts.stats_dump_period_sec = 1;

    // Opts.enable_blob_files = true;
    // Opts.min_blob_size = 1024;

    rocksdb::BlockBasedTableOptions TableOpts = rocksdb::BlockBasedTableOptions();
    TableOpts.block_size = 64 * 1024;
    Opts.table_factory.reset(rocksdb::NewBlockBasedTableFactory(TableOpts));

    return Opts;
}

int rdb_main(int argc, char *argv[])
{
    (void) argc;
    (void) argv;

    SI = new rdb::StorageInstance(16);

    rocksdb::Options Opts = get_db_options();
    const char *Path = "/home/cm/opt/tmp";
    rocksdb::Status S = SI->open(Opts, Path);
    if (!S.ok())
        fatal("Could not open db at '%s': %s", Path, S.ToString().c_str());

    netdata_log_error("Program started...");

    se = storage_engine_get(RRD_MEMORY_MODE_RDB);
    si = reinterpret_cast<STORAGE_INSTANCE *>(NULL);

    size_t num_threads = 1;
    size_t num_groups = 1;
    size_t num_dims_per_group = 1;
    size_t num_points_per_dimension = 3600;

    netdata_log_error("Test simulating %zu agents: threads=%zu, groups=%zu, dims_per_group=%zu, points_per_dimension=%zu)",
                      (num_threads * num_groups * num_dims_per_group) / 2500,
                      num_threads, num_groups, num_dims_per_group, num_points_per_dimension);

    std::vector<uint32_t> rand_vals = genRandVector(1024 * 1024);

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
        size_t n = 60;

        auto start_time = std::chrono::high_resolution_clock::now();
        while (n--) {
            std::this_thread::sleep_for(std::chrono::seconds{1});
            // auto end_time = std::chrono::high_resolution_clock::now();

            // auto duration = std::chrono::duration_cast<std::chrono::milliseconds>(end_time - start_time);
            // double seconds = duration.count() / static_cast<double>(MSEC_PER_SEC);

            // double pages_per_second = static_cast<double>(num_pages_written) / seconds;
            // double points_per_sec = pages_per_second * 1024;
            // double mib_per_sec = (points_per_sec * sizeof(storage_number)) / (1024.0 * 1024.0);

            // double capacity = points_per_sec / 2500.0;

            // netdata_log_error("pages/sec: %.1lf, points/sec: %.1lf, mib/sec: %.1lf, capacity: %.1lf (RSS: %zu MiB)",
            //                   pages_per_second, points_per_sec, mib_per_sec, capacity, getRSS());

            // SI->RDB->Flush(rocksdb::FlushOptions());
        }
    }

    for (std::thread& thread : threads)
        thread.join();

    SI->close();
    exit(EXIT_SUCCESS);
}
