#include <random>
#include <gtest/gtest.h>
#include <rocksdb/db.h>
#include <rocksdb/statistics.h>
#include <rocksdb/options.h>
#include <rocksdb/advanced_options.h>
#include <rocksdb/table.h>

#include "rdb.h"
#include "rdb-private.h"
#include "si.h"

static std::random_device RandDev;
static std::mt19937 Gen(RandDev());
static std::uniform_int_distribution<uint32_t> Dist(std::numeric_limits<uint32_t>::min(),
                                                    std::numeric_limits<uint32_t>::max());

TEST(rdb, Key)
{
    for (size_t i = 0; i != 128; i++)
    {
        uint32_t gid = Dist(Gen);
        uint32_t mid = Dist(Gen);
        uint32_t pit = Dist(Gen);

        rdb::Key k1{gid, mid, pit};
        Slice s1 = k1.slice();

        rdb::Key k2{s1};

        EXPECT_EQ(k1.gid(), k2.gid());
        EXPECT_EQ(k1.mid(), k2.mid());
        EXPECT_EQ(k1.pit(), k2.pit());
    }
}

TEST(rdb, ImmutablePage) {
    std::vector<uint32_t> random_numbers(128);
    std::generate(random_numbers.begin(), random_numbers.end(),
                  [](){ return Dist(Gen); });

    rdbv::RdbValue V;
    rdbv::StorageNumbersPage *SNP = V.mutable_storage_numbers_page();
    SNP->set_update_every(2);

    google::protobuf::RepeatedField<storage_number> *SNs = SNP->mutable_storage_numbers();
    for (uint32_t i : random_numbers)
    {
        storage_number sn = pack_storage_number(i, SN_DEFAULT_FLAGS);
        storage_number *snp = SNs->Add();
        *snp = sn;
    }

    rdb::ImmutablePage IP(&V);

    size_t i = 0;
    for (auto It = IP.begin(3600); It != IP.end(); It++)
    {
        const STORAGE_POINT &SP = *It;

        netdata_log_error("Orig It[%zu]: %lf, tr: [%u, %u)", i, SP.sum, SP.start_time_s, SP.end_time_s);
        NETDATA_DOUBLE exp = unpack_storage_number(pack_storage_number(random_numbers[i++], SN_DEFAULT_FLAGS));
        EXPECT_EQ(SP.sum, exp);
    }
}

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
    return;

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

    usec_t update_every = 1 * USEC_PER_SEC;
    STORAGE_COLLECT_HANDLE *sch = rdb_store_metric_init(smh, update_every / USEC_PER_SEC, smg);
    EXPECT_NE(sch, nullptr);

    usec_t N = 6 * 3600;
    usec_t after = 3600 * USEC_PER_SEC;
    usec_t before = after + N * update_every;

    netdata_log_error("Filling %zu elements in page with ue=%zu, after=%zu and before=%zu",
                      N,
                      update_every / USEC_PER_SEC,
                      after / USEC_PER_SEC,
                      before / USEC_PER_SEC);

    for (usec_t i = 0; i != N; i++)
    {
        usec_t pit = after + i * update_every;
        rdb_store_metric_next(sch, pit, i, 0, 0, 1, 0, SN_DEFAULT_FLAGS);
    }
    rdb_store_metric_flush(sch);

    struct storage_engine_query_handle seqh;
    rdb_load_metric_init(smh, &seqh, 2 * 3600, 3 * 3600, STORAGE_PRIORITY_NORMAL);

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
