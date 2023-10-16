#include <gtest/gtest.h>
#include <rocksdb/db.h>
#include <rocksdb/statistics.h>
#include <rocksdb/options.h>
#include <rocksdb/advanced_options.h>
#include <rocksdb/table.h>

#include "rdb.h"
#include "rdb-private.h"
#include "si.h"

static const char *temp_dir_new()
{
    char tmpl[] = "/tmp/mydirXXXXXX";
    const char *tmp = mkdtemp(tmpl);
    EXPECT_TRUE(tmp);
    return strdupz(tmp);
}

static void temp_dir_delete(const char *path) {
    unlink(path);
    freez((void *) path);
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

    rocksdb::BlockBasedTableOptions TableOpts = rocksdb::BlockBasedTableOptions();
    TableOpts.block_size = 64 * 1024;
    Opts.table_factory.reset(rocksdb::NewBlockBasedTableFactory(TableOpts));

    return Opts;
}

static STORAGE_INSTANCE *storage_instance_new(const char *Path)
{
    SI = new StorageInstance(16);
    rocksdb::Status S = SI->open(get_db_options(), Path);
    EXPECT_TRUE(S.ok());
    return reinterpret_cast<STORAGE_INSTANCE *>(SI);
}

static void storage_instance_delete()
{
    SI->close();
    delete SI;
    SI = nullptr;
}

TEST(rdb, SomeTest) {
    const char *TmpDir = temp_dir_new();
    STORAGE_INSTANCE *si = storage_instance_new(TmpDir);
    EXPECT_NE(si, nullptr);

    RRDDIM rd;
    uuid_generate(rd.metric_uuid);
    STORAGE_METRIC_HANDLE *smh = rdb_metric_get_or_create(&rd, si);
    EXPECT_NE(smh, nullptr);

    uuid_t group_uuid;
    uuid_generate(group_uuid);
    STORAGE_METRICS_GROUP *smg = rdb_metrics_group_get(si, &group_uuid);
    EXPECT_NE(smg, nullptr);

    usec_t update_every = 5 * USEC_PER_SEC;
    STORAGE_COLLECT_HANDLE *sch = rdb_store_metric_init(smh, update_every / USEC_PER_SEC, smg);
    EXPECT_NE(sch, nullptr);

    usec_t N = 10;
    usec_t after = 3600 * USEC_PER_SEC;
    usec_t before = after + N * update_every;

    netdata_log_error("Filling %zu elements in page with ue=%zu, after=%zu and before=%zu",
                      N,
                      update_every / USEC_PER_SEC,
                      after / USEC_PER_SEC,
                      before / USEC_PER_SEC);
    netdata_log_error("");

    for (usec_t i = 0; i != N; i++)
    {
        usec_t pit = after + i * update_every;
        rdb_store_metric_next(sch, pit, i, 0, 0, 1, 0, SN_DEFAULT_FLAGS);

        netdata_log_error("page size: %zu",
                          reinterpret_cast<rdb_collect_handle *>(sch)->collection.value.size());
        netdata_log_error("sch[%zu] = %zu", pit / USEC_PER_SEC, i);
        netdata_log_error("");
    }

    struct storage_engine_query_handle seqh;
    rdb_load_metric_init(smh, &seqh, after / USEC_PER_SEC, before / USEC_PER_SEC, STORAGE_PRIORITY_NORMAL);

    storage_engine_query_next_metric(&seqh);

    storage_instance_delete();
    temp_dir_delete(TmpDir);
}

int rdb_tests_main(int argc, char *argv[])
{
    // skip the `-W rdb-tests` args
    for (int i = 2; i < argc; ++i) {
        argv[i - 1] = argv[i];
    }
    argc -= 2;

    for (size_t i = 0; i != argc; i++)
    {
        netdata_log_error("CLI arg[%d]: %s", i, argv[i]);
    }

    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
