#include <gtest/gtest.h>
#include "daemon/common.h"
#include "ml/json/single_include/nlohmann/json.hpp"

static void test_storage_number_loss(NETDATA_DOUBLE ND) {
    // Check precision loss of packing/unpacking
    {
        SN_FLAGS Flags = SN_DEFAULT_FLAGS;
        storage_number SN = pack_storage_number(ND, Flags);
        EXPECT_TRUE(does_storage_number_exist(SN));

        NETDATA_DOUBLE UnpackedND = unpack_storage_number(SN);

        NETDATA_DOUBLE AbsDiff = UnpackedND - ND;
        NETDATA_DOUBLE PctDiff = AbsDiff * 100.0 / ND;

        if (PctDiff < 0)
            PctDiff = - PctDiff;

        EXPECT_LT(PctDiff, ACCURACY_LOSS_ACCEPTED_PERCENT);
    }

    // check precision loss of custom formatting
    {
        char Buf[100];
        size_t Len = print_netdata_double(Buf, ND);
        EXPECT_EQ(strlen(Buf), Len);


        NETDATA_DOUBLE ParsedND = str2ndd(Buf, NULL);
        NETDATA_DOUBLE ParsedDiff = ND - ParsedND;
        NETDATA_DOUBLE PctParsedDiff = ParsedDiff * 100.0 / ND;

        if(PctParsedDiff < 0)
            PctParsedDiff = - PctParsedDiff;

        EXPECT_LT(PctParsedDiff, ACCURACY_LOSS_ACCEPTED_PERCENT);
    }
}

TEST(storage_number, precision_loss) {
    NETDATA_DOUBLE PosMinSN = unpack_storage_number(STORAGE_NUMBER_POSITIVE_MIN_RAW);
    NETDATA_DOUBLE NegMaxSN = unpack_storage_number(STORAGE_NUMBER_NEGATIVE_MAX_RAW);

    for (int g = -1; g <= 1 ; g++) {
        if(!g)
            continue;

        NETDATA_DOUBLE a = 0;
        for (int j = 0; j < 9 ;j++) {
            a += 0.0000001;

            NETDATA_DOUBLE c = a * g;
            for (int i = 0; i < 21 ;i++, c *= 10) {
                if (c > 0 && c < PosMinSN)
                    continue;
                if (c < 0 && c > NegMaxSN)
                    continue;

                test_storage_number_loss(c);
            }
        }
    }
}

TEST(database, rrdcalc_comparisons) {
    RRDCALC_STATUS a, b;

    memset(&a, 0, sizeof(RRDCALC_STATUS));
    EXPECT_EQ(a, RRDCALC_STATUS_UNINITIALIZED);

    a = RRDCALC_STATUS_REMOVED;
    b = RRDCALC_STATUS_UNDEFINED;
    EXPECT_LT(a, b);

    a = RRDCALC_STATUS_UNDEFINED;
    b = RRDCALC_STATUS_UNINITIALIZED;
    EXPECT_LT(a, b);

    a = RRDCALC_STATUS_UNINITIALIZED;
    b = RRDCALC_STATUS_CLEAR;
    EXPECT_LT(a, b);

    a = RRDCALC_STATUS_CLEAR;
    b = RRDCALC_STATUS_RAISED;
    EXPECT_LT(a, b);


    a = RRDCALC_STATUS_RAISED;
    b = RRDCALC_STATUS_WARNING;
    EXPECT_LT(a, b);

    a = RRDCALC_STATUS_WARNING;
    b = RRDCALC_STATUS_CRITICAL;
    EXPECT_LT(a, b);
}

TEST(storage_number, storage_number_exists) {
    storage_number sn = pack_storage_number(0.0, SN_DEFAULT_FLAGS);

    EXPECT_EQ(0.0, unpack_storage_number(sn));
}


TEST(netdata_double, number_printing) {
    using DoubleStringPair = std::pair<NETDATA_DOUBLE, const char *>;

    std::vector<DoubleStringPair> V = {
        { 0, "0" },
        { 0.0000001, "0.0000001" },
        { 0.00000009, "0.0000001" },
        { 0.000000001, "0" },
        { 99.99999999999999999, "100" },
        { -99.99999999999999999, "-100" },
        { 123.4567890123456789, "123.456789" },
        { 9999.9999999, "9999.9999999" },
        { -9999.9999999, "-9999.9999999" },
    };

    char Buf[50];
    for (const auto &P : V) {
        print_netdata_double(Buf, P.first);
        ASSERT_STREQ(Buf, P.second);
    }
}

TEST(database, renaming) {
   RRDSET *RS = rrdset_create_localhost("chart", "ID", NULL, "family", "context",
                                        "Unit Testing", "a value", "unittest",
                                        NULL, 1, 1, RRDSET_TYPE_LINE);

   RRDDIM *RD1 = rrddim_add(RS, "DIM1", NULL, 1, 1, RRD_ALGORITHM_INCREMENTAL);
   RRDDIM *RD2 = rrddim_add(RS, "DIM2", NULL, 1, 1, RRD_ALGORITHM_INCREMENTAL);

   rrdset_reset_name(RS, "CHARTNAME1");
   EXPECT_STREQ("chart.CHARTNAME1", rrdset_name(RS));
   rrdset_reset_name(RS, "CHARTNAME2");
   EXPECT_STREQ("chart.CHARTNAME2", rrdset_name(RS));

   rrddim_reset_name(RS, RD1, "DIM1NAME1");
   EXPECT_STREQ("DIM1NAME1", rrddim_name(RD1));
   rrddim_reset_name(RS, RD1, "DIM1NAME2");
   EXPECT_STREQ("DIM1NAME2", rrddim_name(RD1));

   rrddim_reset_name(RS, RD2, "DIM2NAME1");
   EXPECT_STREQ("DIM2NAME1", rrddim_name(RD2));
   rrddim_reset_name(RS, RD2, "DIM2NAME2");
   EXPECT_STREQ("DIM2NAME2", rrddim_name(RD2));

   BUFFER *Buf = buffer_create(1);
   health_api_v1_chart_variables2json(RS, Buf);
   nlohmann::json J = nlohmann::json::parse(buffer_tostring(Buf));
   buffer_free(Buf);

   std::string Chart = J["chart"];
   EXPECT_EQ(Chart, "chart.ID");

   std::string ChartName = J["chart_name"];
   EXPECT_EQ(ChartName, "chart.CHARTNAME2");
}

TEST(strdupz_path_subpath, test) {
    struct PathParts {
        const char *Path1;
        const char *Path2;
    };
    std::vector<std::pair<PathParts, const char *>> Values {
        { PathParts{"", ""}, "." },
        { PathParts{"/", ""}, "/" },
        { PathParts{"/etc/netdata", ""}, "/etc/netdata" },
        { PathParts{"/etc/netdata///", ""}, "/etc/netdata" },
        { PathParts{"/etc/netdata///", "health.d"}, "/etc/netdata/health.d" },
        { PathParts{"/etc/netdata///", "///health.d"}, "/etc/netdata/health.d" },
        { PathParts{"/etc/netdata", "///health.d"}, "/etc/netdata/health.d" },
        { PathParts{"", "///health.d"}, "./health.d" },
        { PathParts{"/", "///health.d"}, "/health.d" },
    };

    for (const auto &P : Values) {
        struct PathParts PP = P.first;
        const char *Expected = P.second;

        char *Res = strdupz_path_subpath(PP.Path1, PP.Path2);
        EXPECT_STREQ(Res, Expected);
        freez(Res);
    }
}

TEST(sqlite, statements) {
    sqlite3 *DB;
    int Ret;

    Ret = sqlite3_open(":memory:", &DB);
    EXPECT_EQ(Ret, SQLITE_OK);

    {
        Ret = sqlite3_exec_monitored(DB, "CREATE TABLE IF NOT EXISTS mine (id1, id2);", 0, 0, NULL);
        EXPECT_EQ(Ret, SQLITE_OK);

        Ret = sqlite3_exec_monitored(DB, "DELETE FROM MINE LIMIT 1;", 0, 0, NULL);
        EXPECT_EQ(Ret, SQLITE_OK);

        Ret = sqlite3_exec_monitored(DB, "UPDATE MINE SET id1=1 LIMIT 1;", 0, 0, NULL);
        EXPECT_EQ(Ret, SQLITE_OK);
    }

    {
        BUFFER *Stmt = buffer_create(ACLK_SYNC_QUERY_SIZE);
        const char *UUID = "0000_000";

        buffer_sprintf(Stmt, TABLE_ACLK_ALERT, UUID);
        Ret = sqlite3_exec_monitored(DB, buffer_tostring(Stmt), 0, 0, NULL);
        EXPECT_EQ(Ret, SQLITE_OK);
        buffer_flush(Stmt);

        buffer_sprintf(Stmt, INDEX_ACLK_ALERT, UUID, UUID);
        Ret = sqlite3_exec_monitored(DB, buffer_tostring(Stmt), 0, 0, NULL);
        EXPECT_EQ(Ret, SQLITE_OK);
        buffer_flush(Stmt);

        buffer_free(Stmt);
    }

    Ret = sqlite3_close(DB);
    EXPECT_EQ(Ret, SQLITE_OK);
}

TEST(bitmap, tests) {
    size_t N = 256;
    bitmap_t BM = bitmap_new(N);

    for (size_t Idx = 0; Idx != N; Idx += 2) {
        EXPECT_FALSE(bitmap_get(BM, Idx));
        bitmap_set(BM, Idx, true);
        EXPECT_TRUE(bitmap_get(BM, Idx));
    }

    bitmap_delete(BM);
}

TEST(str2ld, precision_loss) {
    std::vector<std::string> Values = {
            "1.2345678", "-35.6", "0.00123", "23842384234234.2", ".1",
            "1.2e-10", "hello", "1wrong", "nan", "inf"
    };

    for (const std::string &S : Values) {
        char *EndPtrMine = nullptr;
        NETDATA_DOUBLE MineND = str2ndd(S.data(), &EndPtrMine);

        char *EndPtrSys = nullptr;
        NETDATA_DOUBLE SysND = strtondd(S.data(), &EndPtrSys);

        EXPECT_EQ(EndPtrMine, EndPtrSys);

        EXPECT_EQ(isnan(MineND), isnan(SysND));
        EXPECT_EQ(isinf(MineND), isinf(SysND));

        if (isnan(MineND) || isinf(MineND))
            continue;

        NETDATA_DOUBLE Diff= ABS(MineND - SysND);
        EXPECT_LT(Diff, 0.000001);
    }
}

TEST(buffer, test) {
    const char *Fmt = "string1: %s\nstring2: %s\nstring3: %s\nstring4: %s";

    char Dummy[2048 + 1];
    for(int Idx = 0; Idx != 2048; Idx++)
        Dummy[Idx] = ((Idx % 24) + 'a');
    Dummy[2048] = '\0';

    char Expected[9000 + 1];
    snprintfz(Expected, 9000, Fmt, Dummy, Dummy, Dummy, Dummy);

    BUFFER *WB = buffer_create(1);
    buffer_sprintf(WB, Fmt, Dummy, Dummy, Dummy, Dummy);

    const char *Output = buffer_tostring(WB);
    EXPECT_STREQ(Expected, Output);

    buffer_free(WB);
}

TEST(static_threads, test) {
    struct netdata_static_thread *static_threads = static_threads_get();

    EXPECT_NE(static_threads, nullptr);

    size_t n;
    for (n = 0; static_threads[n].start_routine != NULL; n++) { }

    EXPECT_GT(n, 1);

    // check that each thread's start routine is unique
    for (size_t i = 0; i != n - 1; i++)
        for (size_t j = i + 1; j != n; j++)
            EXPECT_NE(static_threads[i].start_routine, static_threads[j].start_routine);

    freez(static_threads);
}
