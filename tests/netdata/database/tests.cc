#include <gtest/gtest.h>
#include "daemon/common.h"
#include "ml/json/single_include/nlohmann/json.hpp"

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
