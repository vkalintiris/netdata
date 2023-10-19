#include "rdb-private.h"
#include <google/protobuf/arena.h>

static std::random_device RandDev;
static std::mt19937 Gen(RandDev());
static std::uniform_int_distribution<uint32_t> Dist(std::numeric_limits<uint32_t>::min(),
                                                    std::numeric_limits<uint32_t>::max());


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
    SI = new rdb::StorageInstance(16);
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

TEST(rdb, Key)
{
    for (size_t i = 0; i != 128; i++)
    {
        uint32_t gid = Dist(Gen);
        uint32_t mid = Dist(Gen);
        uint32_t pit = Dist(Gen);

        rdb::Key k1{gid, mid, pit};
        rdb::Slice s1 = k1.slice();

        rdb::Key k2{s1};

        EXPECT_EQ(k1.gid(), k2.gid());
        EXPECT_EQ(k1.mid(), k2.mid());
        EXPECT_EQ(k1.pit(), k2.pit());
    }
}

TEST(rdb, Page)
{
    constexpr size_t N = 10;
    std::vector<uint32_t> random_numbers(N);
    std::generate(random_numbers.begin(), random_numbers.end(),
                  [](){ return Dist(Gen); });

    google::protobuf::Arena A;
    std::optional<rdb::Page> OP = rdb::Page::create(A, rdb::PageOptions());
    EXPECT_TRUE(OP.has_value());

    uint32_t UE = 2;
    OP->setUpdateEvery(UE);

    for (uint32_t i : random_numbers)
    {
        STORAGE_POINT SP;
        SP.sum = i;
        SP.flags = SN_DEFAULT_FLAGS;
        OP->appendPoint(SP);
    }

    uint32_t PIT = 3600;
    size_t i = 0;

    auto End = OP->end();
    for (auto It = OP->begin(PIT); It != End; It++)
    {
        STORAGE_POINT SP = *It;
        EXPECT_EQ(SP.start_time_s, PIT + (i * UE));
        EXPECT_EQ(SP.end_time_s, SP.start_time_s + UE);

        NETDATA_DOUBLE exp = unpack_storage_number(pack_storage_number(random_numbers[i++], SN_DEFAULT_FLAGS));
        EXPECT_EQ(SP.sum, exp);

        EXPECT_EQ(SP.flags, SN_DEFAULT_FLAGS);
    }
}

TEST(rdb, CollectionHandle)
{
    const char *TmpDir = temp_dir_new();
    STORAGE_INSTANCE *si = storage_instance_new(TmpDir);
    EXPECT_NE(si, nullptr);

    using namespace rdb;

    PageOptions PO;
    PO.initial_slots = 4;
    PO.update_every = 5;
    usec_t UE = PO.update_every * USEC_PER_SEC;

    pb::Arena Arena;
    auto CH = CollectionHandle::create(Arena, PO, 1, 1);
    EXPECT_TRUE(CH.has_value());

    EXPECT_EQ(CH->after(), 0);
    EXPECT_EQ(CH->before(), 0);
    EXPECT_EQ(CH->duration(), 0);

    STORAGE_POINT SP = {
        .min = 0,
        .max = 0,
        .sum = 0,

        .start_time_s = 0,
        .end_time_s = 0,

        .count = 1,
        .anomaly_count = 0,

        .flags = SN_DEFAULT_FLAGS,
    };

    // Fill the entire page
    for (uint32_t i = 0; i != PO.initial_slots; i++)
    {
        usec_t PIT = 10 * USEC_PER_SEC + i * UE;
        usec_t After = 10 * USEC_PER_SEC;
        usec_t Before = PIT + UE;
        usec_t Duration = (i + 1) * UE;

        CH->store_next(PIT, SP);
        EXPECT_EQ(CH->after(), After);
        EXPECT_EQ(CH->before(), Before);
        EXPECT_EQ(CH->duration(), Duration);
    }

    // Adding a new point will cause the handle to flush the page
    uint32_t i = PO.initial_slots;
    usec_t PIT = (10 * USEC_PER_SEC) + (i * UE);
    usec_t After = PIT;
    usec_t Before = PIT + UE;
    usec_t Duration = UE;

    CH->store_next(PIT, SP);
    EXPECT_EQ(CH->after(), After);
    EXPECT_EQ(CH->before(), Before);
    EXPECT_EQ(CH->duration(), Duration);

    // Flushing should maintain the handle's PIT
    CH->flush();
    EXPECT_EQ(CH->after(), Before);
    EXPECT_EQ(CH->before(), Before);
    EXPECT_EQ(CH->duration(), 0);

    // No effect if we flush twice without adding new elements
    CH->flush();
    EXPECT_EQ(CH->after(), Before);
    EXPECT_EQ(CH->before(), Before);
    EXPECT_EQ(CH->duration(), 0);

    // ... repeatedly
    CH->flush();
    EXPECT_EQ(CH->after(), Before);
    EXPECT_EQ(CH->before(), Before);
    EXPECT_EQ(CH->duration(), 0);

    // After the original flush the page should be able to hold 1024 points
    usec_t StartPIT = Before;
    for (uint32_t i = 0; i != PO.capacity; i++)
    {
        usec_t PIT = StartPIT + (i * UE);
        usec_t Before = PIT + UE;
        usec_t Duration = (i + 1) * UE;

        CH->store_next(PIT, SP);
        EXPECT_EQ(CH->after(), StartPIT);
        EXPECT_EQ(CH->before(), Before);
        EXPECT_EQ(CH->duration(), Duration);
    }

    // Adding a new point will cause the handle to flush the page
    CH->store_next(CH->before(), SP);
    EXPECT_EQ(CH->before(), CH->after() + UE);
    EXPECT_EQ(CH->duration(), UE);

    // Flush the only point we have
    CH->flush();
    EXPECT_EQ(CH->after(), CH->before());
    EXPECT_EQ(CH->duration(), 0);

    // Try adding a gap that can be filled without flushing
    CH->flush();
    {
        usec_t StartPIT = CH->before();
        CH->store_next(StartPIT + (10 * UE), SP);
        EXPECT_EQ(CH->after(), StartPIT);
        EXPECT_EQ(CH->before(), StartPIT + (11 * UE));
        EXPECT_EQ(CH->duration(), (11 * UE));
    }

    // Try adding a gap that can be filled after only flushing
    CH->flush();
    {
        usec_t StartPIT = CH->before() + PO.capacity * UE;
        CH->store_next(StartPIT, SP);
        EXPECT_EQ(CH->after(), StartPIT);
        EXPECT_EQ(CH->before(), StartPIT + UE);
        EXPECT_EQ(CH->duration(), UE);
    }

    storage_instance_delete();
    temp_dir_delete(TmpDir);
}


TEST(rdb, CollectionHandleQuery)
{
    const char *TmpDir = temp_dir_new();
    STORAGE_INSTANCE *si = storage_instance_new(TmpDir);
    EXPECT_NE(si, nullptr);

    using namespace rdb;

    PageOptions PO;
    PO.initial_slots = 128;
    PO.update_every = 5;

    pb::Arena Arena;
    auto CH = CollectionHandle::create(Arena, PO, 1, 1);
    EXPECT_TRUE(CH.has_value());

    STORAGE_POINT SP = {
        .min = 0,
        .max = 0,
        .sum = 0,

        .start_time_s = 0,
        .end_time_s = 0,

        .count = 1,
        .anomaly_count = 0,

        .flags = SN_DEFAULT_FLAGS,
    };

    // Fill the entire page
    for (uint32_t i = 0; i != 4; i++)
    {
        usec_t PIT = (10 + i * PO.update_every) * USEC_PER_SEC;
        usec_t After = 10 * USEC_PER_SEC;
        usec_t Before = PIT + (PO.update_every * USEC_PER_SEC);
        usec_t Duration = ((i + 1) * PO.update_every) * USEC_PER_SEC;

        SP.min = SP.max = SP.sum = static_cast<double>(i + 666);
        CH->store_next(PIT, SP);
        EXPECT_EQ(CH->after(), After);
        EXPECT_EQ(CH->before(), Before);
        EXPECT_EQ(CH->duration(), Duration);
    }

    netdata_log_error("Page has range: [%u, %u]",
                      CH->after() / USEC_PER_SEC,
                      CH->before() / USEC_PER_SEC);

    auto OP = CH->queryLock(CH->after() + (PO.update_every * USEC_PER_SEC));
    EXPECT_TRUE(OP.has_value());

    for (Page::PageIterator It = OP->first, End = OP->second;
         It != End;
         It++)
    {
        STORAGE_POINT SP = *It;

        netdata_log_error("Start time: %ld, end time: %ld, value: %lf",
                          SP.start_time_s, SP.end_time_s, SP.sum);
    }

    CH->queryUnlock();

    storage_instance_delete();
    temp_dir_delete(TmpDir);
}

// TEST(rdb, PageIterator)
// {
//     const char *TmpDir = temp_dir_new();
//     STORAGE_INSTANCE *si = storage_instance_new(TmpDir);
//     EXPECT_NE(si, nullptr);

//     using namespace rdb;

//     PageOptions PO;
//     PO.initial_slots = 128;
//     PO.update_every = 5;

//     pb::Arena Arena;
//     std::optional<Page> P = Page::create(Arena, PO);
//     EXPECT_TRUE(P.has_value());

//     P->


//     STORAGE_POINT SP = {
//         .min = 0,
//         .max = 0,
//         .sum = 0,

//         .start_time_s = 0,
//         .end_time_s = 0,

//         .count = 1,
//         .anomaly_count = 0,

//         .flags = SN_DEFAULT_FLAGS,
//     };

//     // Fill the entire page
//     for (uint32_t i = 0; i != PO.initial_slots; i++)
//     {
//         usec_t PIT = (10 + i * PO.update_every) * USEC_PER_SEC;
//         usec_t After = 10 * USEC_PER_SEC;
//         usec_t Before = PIT + (PO.update_every * USEC_PER_SEC);
//         usec_t Duration = ((i + 1) * PO.update_every) * USEC_PER_SEC;

//         SP.min = SP.max = SP.sum = static_cast<double>(i + 666);
//         CH->store_next(PIT, SP);
//         EXPECT_EQ(CH->after(), After);
//         EXPECT_EQ(CH->before(), Before);
//         EXPECT_EQ(CH->duration(), Duration);
//     }

//     netdata_log_error("Page has range: [%u, %u]",
//                       CH->after() / USEC_PER_SEC,
//                       CH->before() / USEC_PER_SEC);

//     auto OP = CH->queryLock(CH->after());
//     EXPECT_TRUE(OP.has_value());

//     int i = 0;
//     for (Page::PageIterator It = OP->first, End = OP->second;
//          i++ != 10;)
//     {
//         STORAGE_POINT SP = *It;

//         netdata_log_error("Start time: %ld, end time: %ld, value: %lf",
//                           SP.start_time_s, SP.end_time_s, SP.sum);

//         netdata_log_error("Incrementing iterator");
//         It++;
//         netdata_log_error("Incremented iterator");
//         netdata_log_error("");
//     }

//     CH->queryUnlock();

//     storage_instance_delete();
//     temp_dir_delete(TmpDir);
// }

// TEST(rdb, SomeTest) {
//     return;

//     const char *TmpDir = temp_dir_new();
//     STORAGE_INSTANCE *si = storage_instance_new(TmpDir);
//     EXPECT_NE(si, nullptr);

//     RRDDIM rd;
//     uuid_generate(rd.metric_uuid);
//     STORAGE_METRIC_HANDLE *smh = rdb_metric_get_or_create(&rd, si);
//     EXPECT_NE(smh, nullptr);

//     uuid_t group_uuid;
//     uuid_generate(group_uuid);
//     STORAGE_METRICS_GROUP *smg = rdb_metrics_group_get(si, &group_uuid);
//     EXPECT_NE(smg, nullptr);

//     usec_t update_every = 1 * USEC_PER_SEC;
//     STORAGE_COLLECT_HANDLE *sch = rdb_store_metric_init(smh, update_every / USEC_PER_SEC, smg);
//     EXPECT_NE(sch, nullptr);

//     usec_t N = 6 * 3600;
//     usec_t after = 3600 * USEC_PER_SEC;
//     usec_t before = after + N * update_every;

//     netdata_log_error("Filling %zu elements in page with ue=%zu, after=%zu and before=%zu",
//                       N,
//                       update_every / USEC_PER_SEC,
//                       after / USEC_PER_SEC,
//                       before / USEC_PER_SEC);

//     for (usec_t i = 0; i != N; i++)
//     {
//         usec_t pit = after + i * update_every;
//         rdb_store_metric_next(sch, pit, i, 0, 0, 1, 0, SN_DEFAULT_FLAGS);
//     }
//     rdb_store_metric_flush(sch);

//     struct storage_engine_query_handle seqh;
//     rdb_load_metric_init(smh, &seqh, 2 * 3600, 3 * 3600, STORAGE_PRIORITY_NORMAL);

//     storage_engine_query_next_metric(&seqh);

//     storage_instance_delete();
//     temp_dir_delete(TmpDir);
// }

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
