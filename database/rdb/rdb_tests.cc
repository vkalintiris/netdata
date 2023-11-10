#include "rdb-private.h"

using namespace rdb;

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
    SI = new rdb::StorageInstance();
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

    PageOptions PO;
    PO.initial_slots = 4;
    PO.update_every = 5;
    usec_t UE = PO.update_every * USEC_PER_SEC;

    pb::Arena Arena;
    MetricHandle MH(1, 1);
    auto CH = CollectionHandle::create(Arena, PO, MH.gid(), MH.mid());
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

        CH->store_next(MH, PIT, SP);
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

    CH->store_next(MH, PIT, SP);
    EXPECT_EQ(CH->after(), After);
    EXPECT_EQ(CH->before(), Before);
    EXPECT_EQ(CH->duration(), Duration);

    // Flushing should maintain the handle's PIT
    CH->flush(MH);
    EXPECT_EQ(CH->after(), Before);
    EXPECT_EQ(CH->before(), Before);
    EXPECT_EQ(CH->duration(), 0);

    // No effect if we flush twice without adding new elements
    CH->flush(MH);
    EXPECT_EQ(CH->after(), Before);
    EXPECT_EQ(CH->before(), Before);
    EXPECT_EQ(CH->duration(), 0);

    // ... repeatedly
    CH->flush(MH);
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

        CH->store_next(MH, PIT, SP);
        EXPECT_EQ(CH->after(), StartPIT);
        EXPECT_EQ(CH->before(), Before);
        EXPECT_EQ(CH->duration(), Duration);
    }

    // Adding a new point will cause the handle to flush the page
    CH->store_next(MH, CH->before(), SP);
    EXPECT_EQ(CH->before(), CH->after() + UE);
    EXPECT_EQ(CH->duration(), UE);

    // Flush the only point we have
    CH->flush(MH);
    EXPECT_EQ(CH->after(), CH->before());
    EXPECT_EQ(CH->duration(), 0);

    // Try adding a gap that can be filled without flushing
    CH->flush(MH);
    {
        usec_t StartPIT = CH->before();
        CH->store_next(MH, StartPIT + (10 * UE), SP);
        EXPECT_EQ(CH->after(), StartPIT);
        EXPECT_EQ(CH->before(), StartPIT + (11 * UE));
        EXPECT_EQ(CH->duration(), (11 * UE));
    }

    // Try adding a gap that can be filled after only flushing
    CH->flush(MH);
    {
        usec_t StartPIT = CH->before() + PO.capacity * UE;
        CH->store_next(MH, StartPIT, SP);
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

    PageOptions PO;
    PO.initial_slots = 1024;
    PO.update_every = 5;
    usec_t UE = PO.update_every * USEC_PER_SEC;

    pb::Arena Arena;
    MetricHandle MH(1, 1);
    auto CH = CollectionHandle::create(Arena, PO, MH.gid(), MH.mid());
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
    const usec_t After = 10 * USEC_PER_SEC;
    for (uint32_t i = 0; i != PO.initial_slots; i++)
    {
        usec_t PIT = After + (i * UE);
        usec_t Before = PIT + UE;
        usec_t Duration = (i + 1) * UE;

        SP.min = SP.max = SP.sum = static_cast<double>(i + 666);
        CH->store_next(MH, PIT, SP);
        EXPECT_EQ(CH->after(), After);
        EXPECT_EQ(CH->before(), Before);
        EXPECT_EQ(CH->duration(), Duration);
    }
    const usec_t Before = CH->before();

    // Query the entire page range
    {
        auto OP = CH->queryLock(CH->after());
        EXPECT_TRUE(OP.has_value());

        uint32_t PIT = After / USEC_PER_SEC;
        for (Page::PageIterator It = OP->first, End = OP->second;
             It != End;
             It++)
        {
            STORAGE_POINT SP = *It;

            EXPECT_EQ(SP.start_time_s, PIT);
            EXPECT_EQ(SP.end_time_s, PIT + PO.update_every);
            EXPECT_EQ(SP.sum, (It - OP->first) + 666);

            PIT += PO.update_every;
        }
        EXPECT_EQ(PIT * USEC_PER_SEC, Before);

        CH->queryUnlock();
    }

    // Query the first half
    {
        auto OP = CH->queryLock(CH->after());
        EXPECT_TRUE(OP.has_value());

        uint32_t PIT = After / USEC_PER_SEC;
        for (Page::PageIterator It = OP->first, End = OP->second;
             It != End;
             It++)
        {
            STORAGE_POINT SP = *It;

            EXPECT_EQ(SP.start_time_s, PIT);
            EXPECT_EQ(SP.end_time_s, PIT + PO.update_every);
            EXPECT_EQ(SP.sum, (It - OP->first) + 666);

            PIT += PO.update_every;
        }
        EXPECT_EQ(PIT * USEC_PER_SEC, Before);

        CH->queryUnlock();
    }

    storage_instance_delete();
    temp_dir_delete(TmpDir);
}

TEST(Gpt, EmptyHandleQuery)
{
    pb::Arena Arena;
    PageOptions PO;
    auto CH = CollectionHandle::create(Arena, PO, 1, 1);
    ASSERT_TRUE(CH.has_value());

    auto queryResult = CH->queryLock(CH->after());
    EXPECT_FALSE(queryResult.has_value());
}

TEST(Gpt, InvalidTimeRangeQuery)
{
    pb::Arena Arena;
    PageOptions PO;
    PO.update_every = 5;
    MetricHandle MH(1, 1);
    auto CH = CollectionHandle::create(Arena, PO, MH.gid(), MH.mid());
    ASSERT_TRUE(CH.has_value());

    {
        auto q = CH->queryLock(CH->after());
        EXPECT_FALSE(q.has_value());
        CH->queryUnlock();
    }

    STORAGE_POINT SP = {
        .min = 6,
        .max = 6,
        .sum = 6,
        .start_time_s = 0,
        .end_time_s = 0,
        .count = 1,
        .anomaly_count = 0,
        .flags = SN_DEFAULT_FLAGS,
    };
    CH->store_next(MH, 100 * PO.update_every * USEC_PER_SEC, SP);
    SP.min = SP.max = SP.sum = 7;
    CH->store_next(MH, 101 * PO.update_every * USEC_PER_SEC, SP);

    // Query the handle with a starting time older than CH's time range
    {
        auto q = CH->queryLock((50 * PO.update_every) * USEC_PER_SEC);
        EXPECT_TRUE(q.has_value());

        SP = *q->first++;
        EXPECT_EQ(SP.start_time_s, 100 * PO.update_every);
        EXPECT_EQ(SP.end_time_s, 101 * PO.update_every);
        EXPECT_EQ(SP.sum, 6);

        SP = *q->first++;
        EXPECT_EQ(SP.start_time_s, 101 * PO.update_every);
        EXPECT_EQ(SP.end_time_s, 102 * PO.update_every);
        EXPECT_EQ(SP.sum, 7);

        EXPECT_EQ(q->first, q->second);
        CH->queryUnlock();
    }

    // Query the handle with an valid time range
    {
        auto q = CH->queryLock(100 * PO.update_every * USEC_PER_SEC);
        EXPECT_TRUE(q.has_value());

        SP = *q->first++;
        EXPECT_EQ(SP.start_time_s, 100 * PO.update_every);
        EXPECT_EQ(SP.end_time_s, 101 * PO.update_every);
        EXPECT_EQ(SP.sum, 6);

        SP = *q->first++;
        EXPECT_EQ(SP.start_time_s, 101 * PO.update_every);
        EXPECT_EQ(SP.end_time_s, 102 * PO.update_every);
        EXPECT_EQ(SP.sum, 7);

        EXPECT_EQ(q->first, q->second);
        CH->queryUnlock();
    }

    // Query the handle with an valid but unaligned time range
    {
        auto q = CH->queryLock((100 * PO.update_every + 1) * USEC_PER_SEC);
        EXPECT_TRUE(q.has_value());

        SP = *q->first++;
        EXPECT_EQ(SP.start_time_s, 100 * PO.update_every);
        EXPECT_EQ(SP.end_time_s, 101 * PO.update_every);
        EXPECT_EQ(SP.sum, 6);

        SP = *q->first++;
        EXPECT_EQ(SP.start_time_s, 101 * PO.update_every);
        EXPECT_EQ(SP.end_time_s, 102 * PO.update_every);
        EXPECT_EQ(SP.sum, 7);

        EXPECT_EQ(q->first, q->second);
        CH->queryUnlock();
    }

    // Query the handle with an valid time at the second point
    {
        auto q = CH->queryLock((101 * PO.update_every) * USEC_PER_SEC);
        EXPECT_TRUE(q.has_value());

        SP = *q->first++;
        EXPECT_EQ(SP.start_time_s, 101 * PO.update_every);
        EXPECT_EQ(SP.end_time_s, 102 * PO.update_every);
        EXPECT_EQ(SP.sum, 7);

        EXPECT_EQ(q->first, q->second);
        CH->queryUnlock();
    }

    // Query the handle with an valid but unaligned time at the second
    {
        auto q = CH->queryLock((101 * PO.update_every + 1) * USEC_PER_SEC);
        EXPECT_TRUE(q.has_value());

        SP = *q->first++;
        EXPECT_EQ(SP.start_time_s, 101 * PO.update_every);
        EXPECT_EQ(SP.end_time_s, 102 * PO.update_every);
        EXPECT_EQ(SP.sum, 7);

        EXPECT_EQ(q->first, q->second);
        CH->queryUnlock();
    }

    // Query the handle with a timepoint past the end of the page
    {
        auto q = CH->queryLock((102 * PO.update_every) * USEC_PER_SEC);
        EXPECT_FALSE(q.has_value());

        CH->queryUnlock();
    }

    // Query the handle with an unaligned timepoint past the end of the page
    {
        auto q = CH->queryLock((102 * PO.update_every + 1) * USEC_PER_SEC);
        EXPECT_FALSE(q.has_value());

        CH->queryUnlock();
    }
}

TEST(rdb, PageIterator)
{
    const char *TmpDir = temp_dir_new();
    STORAGE_INSTANCE *si = storage_instance_new(TmpDir);
    EXPECT_NE(si, nullptr);

    PageOptions PO;
    PO.initial_slots = 128;
    PO.update_every = 5;

    pb::Arena Arena;
    std::optional<Page> P = Page::create(Arena, PO);
    EXPECT_TRUE(P.has_value());

    MetricHandle MH(1, 1);
    auto CH = CollectionHandle::create(Arena, PO, MH.gid(), MH.mid());
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
    std::vector<std::pair<time_t, uint32_t>> StoredValues;

    for (uint32_t i = 0; i != PO.initial_slots; i++)
    {
        usec_t PIT = (10 + i * PO.update_every) * USEC_PER_SEC;
        usec_t After = 10 * USEC_PER_SEC;
        usec_t Before = PIT + (PO.update_every * USEC_PER_SEC);
        usec_t Duration = ((i + 1) * PO.update_every) * USEC_PER_SEC;

        SP.min = SP.max = SP.sum = static_cast<double>(i + 666);
        StoredValues.push_back({ PIT / USEC_PER_SEC, SP.sum });

        CH->store_next(MH, PIT, SP);
        EXPECT_EQ(CH->after(), After);
        EXPECT_EQ(CH->before(), Before);
        EXPECT_EQ(CH->duration(), Duration);
    }

    auto OP = CH->queryLock(CH->after());
    EXPECT_TRUE(OP.has_value());

    EXPECT_EQ(std::distance(OP->first, OP->second), PO.initial_slots);
    std::vector<std::pair<time_t, uint32_t>> CollectedValues;
    for (Page::PageIterator& It = OP->first, End = OP->second;
         It != End; It++)
    {
        STORAGE_POINT SP = *It;

        CollectedValues.push_back({ SP.start_time_s, static_cast<uint32_t>(SP.sum) });
    }

    CH->queryUnlock();

    EXPECT_EQ(StoredValues, CollectedValues);

    storage_instance_delete();
    temp_dir_delete(TmpDir);
}

void checkVectors(std::vector<std::pair<time_t, uint32_t>> V1,
                  std::vector<std::pair<time_t, uint32_t>> V2)
{
    if (V1.size() != V2.size())
        fatal("V1.size(): %zu, V2.size: %zu", V1.size(), V2.size());

    for (size_t i = 0; i != V1.size(); i++) {
        if (V1[i] == V2[i])
            continue;

        netdata_log_error("V1[%zu] = { time = %ld, value: %u }, V2[%zu] = { time = %ld, value: %u }",
                          i, V1[i].first, V1[i].second,
                          i, V2[i].first, V2[i].second);
        break;
    }
}

TEST(rdb, FlushedQueryHandle)
{
    const char *TmpDir = temp_dir_new();
    STORAGE_INSTANCE *si = storage_instance_new(TmpDir);
    EXPECT_NE(si, nullptr);

    // Set up CollectionHandle
    PageOptions PO;
    PO.initial_slots = 1024;
    PO.update_every = 4;
    usec_t UE = PO.update_every * USEC_PER_SEC;
    size_t values_per_hour = 10;

    pb::Arena Arena;
    MetricHandle MH(1, 1);
    STORAGE_POINT SP = {
        .min = 0, .max = 0, .sum = 0,
        .start_time_s = 0, .end_time_s = 0,
        .count = 1, .anomaly_count = 0,
        .flags = SN_DEFAULT_FLAGS,
    };

    const usec_t Hour = 3600 * USEC_PER_SEC;

    std::vector<std::pair<time_t, uint32_t>> StoredValues;

    // Fill 10 values at the start of each hour of a day
    std::optional<CollectionHandle> CH;

    // for (usec_t PIT = Hour; PIT < 24 * Hour; PIT += Hour)
    for (size_t i = 1; i != 24; i++)
    {
        usec_t PIT = i * Hour;

        SP.min = SP.max = SP.sum = static_cast<NETDATA_DOUBLE>(PIT) / Hour;

        // TODO: Add another test that use a persistent collection handle.
        CH = CollectionHandle::create(Arena, PO, MH.gid(), MH.mid());
        EXPECT_TRUE(CH.has_value());

        for (usec_t CurrPIT = PIT; CurrPIT < PIT + (values_per_hour * UE); CurrPIT += UE)
        {
            time_t Timepoint = CurrPIT / USEC_PER_SEC;

            CH->store_next(MH, CurrPIT, SP);
            StoredValues.push_back({ Timepoint, static_cast<uint32_t>(SP.sum)});

            SP.min = SP.max = SP.sum += 1;
        }

        if (i < 23)
            CH->flush(MH);
    }

    // Query entire range
    {
        std::vector<std::pair<time_t, uint32_t>> CollectedValues;

        uint32_t After = Hour / USEC_PER_SEC;
        pb::Arena QA;

        UniversalQuery UQ(&MH, &CH.value(), After, 24 * 3600);
        while (!UQ.isFinished(QA))
        {
            STORAGE_POINT SP = UQ.next();
            CollectedValues.push_back({ SP.start_time_s, static_cast<uint32_t>(SP.sum) });
        }
        UQ.finalize();

        checkVectors(StoredValues, CollectedValues);
        EXPECT_EQ(StoredValues, CollectedValues);
    }
    
    // Queries for each second within the day
    {
        std::vector<std::pair<time_t, uint32_t>> CollectedValues;

        pb::Arena QA;

        std::vector<std::pair<time_t, uint32_t>> ExpectedValues;
        uint32_t Before = 24 * 3600;
        for (uint32_t i = 3600; i != Before; i++)
        {
            CollectedValues.clear();

            uint32_t After = i;
            size_t points_returned = 0;

            UniversalQuery UQ(&MH, &CH.value(), After, Before);
            while (!UQ.isFinished(QA))
            {
                STORAGE_POINT SP = UQ.next();

                EXPECT_GE(SP.start_time_s, After - PO.update_every);
                EXPECT_LT(SP.start_time_s, Before);
                EXPECT_EQ(SP.end_time_s - SP.start_time_s, PO.update_every);

                {
                    uint32_t hour = ((SP.start_time_s + 3600) / 3600) - 1;
                    if (!hour)
                        continue;

                    uint32_t hour_offset = SP.start_time_s - (hour * 3600);
                    uint32_t point_offset = hour_offset / PO.update_every;
                    EXPECT_LT(point_offset, values_per_hour);

                    EXPECT_DOUBLE_EQ(SP.sum, hour + point_offset);
                }

                points_returned++;
            }
            UQ.finalize();

            if ((i % 3600) == 0)
            {
                uint32_t Hour = ((i + 3600) / 3600) - 1;
                EXPECT_LT(Hour, 24);
                EXPECT_EQ(points_returned, (24 - Hour) * values_per_hour);
            }
        }
    }

    // Clean up
    storage_instance_delete();
    temp_dir_delete(TmpDir);
}

TEST(Query, UniversalQuery)
{
    const char *TmpDir = temp_dir_new();
    STORAGE_INSTANCE *si = storage_instance_new(TmpDir);
    EXPECT_NE(si, nullptr);

    PageOptions PO;
    PO.initial_slots = 1024;
    PO.update_every = 2;
    usec_t UE = PO.update_every * USEC_PER_SEC;

    pb::Arena CollectionArena;
    pb::Arena QueryArena;
    MetricHandle MH(1, 1);

    std::optional<CollectionHandle> CH = CollectionHandle::create(CollectionArena, PO, MH.gid(), MH.mid());
    EXPECT_TRUE(CH.has_value());

    STORAGE_POINT SP = {
        .min = 0, .max = 0, .sum = 0,
        .start_time_s = 0, .end_time_s = 0,
        .count = 1, .anomaly_count = 0,
        .flags = SN_DEFAULT_FLAGS,
    };

    for (size_t Idx = 0; Idx != 10; Idx++)
    {
        usec_t PIT = (3600 + Idx * PO.update_every) * USEC_PER_SEC;
        SP.min = SP.max = SP.sum = Idx;

        CH->store_next(MH, PIT, SP);
    }

    CH->flush(MH);

    // exact query range
    {
        uint32_t After = 3600;
        uint32_t Before = 3600 + 10 * PO.update_every;
        MetricHandleQuery MHQ(&MH, After, Before);

        EXPECT_FALSE(MHQ.isFinished(QueryArena));

        size_t Idx = 0;
        while (!MHQ.isFinished(QueryArena))
        {
            STORAGE_POINT SP = MHQ.next();

            EXPECT_EQ(SP.start_time_s, After + Idx * PO.update_every);
            EXPECT_EQ(SP.end_time_s, SP.start_time_s + PO.update_every);
            EXPECT_EQ(SP.sum, Idx);

            Idx++;
        }
        EXPECT_EQ(Idx, 10);

        MHQ.finalize();
    }

    //  query range: after LT first PIT
    {
        uint32_t After = 0;
        uint32_t Before = 3600 + 10 * PO.update_every;
        MetricHandleQuery MHQ(&MH, After, Before);

        EXPECT_FALSE(MHQ.isFinished(QueryArena));

        size_t Idx = 0;
        while (!MHQ.isFinished(QueryArena))
        {
            STORAGE_POINT SP = MHQ.next();

            EXPECT_EQ(SP.start_time_s, 3600 + Idx * PO.update_every);
            EXPECT_EQ(SP.end_time_s, SP.start_time_s + PO.update_every);
            EXPECT_EQ(SP.sum, Idx);

            Idx++;
        }
        EXPECT_EQ(Idx, 10);

        MHQ.finalize();
    }

    //  query range: before GT last PIT
    {
        uint32_t After = 0;
        uint32_t Before = 24 * 3600 + 10 * PO.update_every;
        MetricHandleQuery MHQ(&MH, After, Before);

        EXPECT_FALSE(MHQ.isFinished(QueryArena));

        size_t Idx = 0;
        while (!MHQ.isFinished(QueryArena))
        {
            STORAGE_POINT SP = MHQ.next();

            EXPECT_EQ(SP.start_time_s, 3600 + Idx * PO.update_every);
            EXPECT_EQ(SP.end_time_s, SP.start_time_s + PO.update_every);
            EXPECT_EQ(SP.sum, Idx);

            Idx++;
        }
        EXPECT_EQ(Idx, 10);

        MHQ.finalize();
    }

    //  query range: in-between first/last PIT
    {
        uint32_t After = 3600 + PO.update_every;
        uint32_t Before = 3600 + 9 * PO.update_every;
        MetricHandleQuery MHQ(&MH, After, Before);

        EXPECT_FALSE(MHQ.isFinished(QueryArena));

        size_t Idx = 1;
        while (!MHQ.isFinished(QueryArena))
        {
            STORAGE_POINT SP = MHQ.next();

            EXPECT_EQ(SP.start_time_s, 3600 + Idx * PO.update_every);
            EXPECT_EQ(SP.end_time_s, SP.start_time_s + PO.update_every);
            EXPECT_EQ(SP.sum, Idx);

            Idx++;
        }
        EXPECT_EQ(Idx, 10);

        MHQ.finalize();
    }

    // query range: unaligned in-between first/last PIT
    // FIXME: decide if return values for unaligned before.
    {
        uint32_t After = 3600 + PO.update_every + 1;
        uint32_t Before = 3600 + 9 * PO.update_every - 1;
        MetricHandleQuery MHQ(&MH, After, Before);

        EXPECT_FALSE(MHQ.isFinished(QueryArena));

        size_t Idx = 1;
        while (!MHQ.isFinished(QueryArena))
        {
            STORAGE_POINT SP = MHQ.next();

            EXPECT_EQ(SP.start_time_s, 3600 + Idx * PO.update_every);
            EXPECT_EQ(SP.end_time_s, SP.start_time_s + PO.update_every);
            EXPECT_EQ(SP.sum, Idx);

            Idx++;
        }
        EXPECT_EQ(Idx, 10);

        MHQ.finalize();
    }
}

int rdb_tests_main(int argc, char *argv[])
{
    // skip the `-W rdb-tests` args
    for (int i = 2; i < argc; ++i) {
        argv[i - 1] = argv[i];
    }
    argc -= 2;

    ::testing::InitGoogleTest(&argc, argv);
    // ::testing::GTEST_FLAG(filter) = "gvd.*";

    int rc = RUN_ALL_TESTS();
    google::protobuf::ShutdownProtobufLibrary();
    return rc;
}
