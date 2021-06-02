// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

#include "AnomalyDetector.h"
#include "Unit.h"
#include "Host.h"

#include "gtest/gtest.h"

using namespace ml;

using StorageNumbers = std::vector<storage_number>;
using StorageNumbersIterator = StorageNumbers::iterator;

class QueryOp {
public:
    QueryOp(StorageNumbers &SNs)
        : SNs(SNs) { }

    time_t latestTimeOp(RRDDIM *RD) {
        (void) RD;

        return SNs.size() - 1;
    }

    time_t oldestTimeOp(RRDDIM *RD) {
        (void) RD;

        return 0;
    }

    void initOp(RRDDIM *RD, struct rrddim_query_handle *Handle,
                time_t StartT, time_t EndT) {
        (void) RD, (void) Handle, (void) StartT, (void) EndT;

        Iter = SNs.begin();
    }

    int isFinishedOp(struct rrddim_query_handle *Handle) {
        (void) Handle;

        return Iter == SNs.end();
    }

    storage_number nextMetricOp(struct rrddim_query_handle *Handle, time_t *CurrT) {
        (void) Handle;

        *CurrT = Iter - SNs.begin();
        return *Iter++;
    }

    void finalizeOp(struct rrddim_query_handle *Handle) {
        (void) Handle;

        Iter = SNs.end();
    }

private:
    StorageNumbers &SNs;
    StorageNumbersIterator Iter;
};

static QueryOp *GlobalQOp;

class Dimension {
public:
    Dimension(std::string Name, StorageNumbers &SNs) {
        RD = new RRDDIM;
        RD->name = Name.c_str();

        RD->state = new struct rrddim_volatile;
        RD->state->query_ops.latest_time = &Dimension::latestTimeOp;
        RD->state->query_ops.oldest_time = &Dimension::oldestTimeOp;
        RD->state->query_ops.init = &Dimension::initOp;
        RD->state->query_ops.is_finished = &Dimension::isFinishedOp;
        RD->state->query_ops.next_metric = &Dimension::nextMetricOp;
        RD->state->query_ops.finalize = &Dimension::finalizeOp;

        GlobalQOp = new QueryOp(SNs);
    }

    RRDDIM *getRD() {
        return RD;
    }

    ~Dimension() {
        delete RD->state;
        delete RD;
        delete GlobalQOp;
    }

private:
    static time_t latestTimeOp(RRDDIM *RD) {
        return GlobalQOp->latestTimeOp(RD);
    }

    static time_t oldestTimeOp(RRDDIM *RD) {
        return GlobalQOp->oldestTimeOp(RD);
    }

    static void initOp(RRDDIM *RD, struct rrddim_query_handle *Handle,
                time_t StartT, time_t EndT) {
        GlobalQOp->initOp(RD, Handle, StartT, EndT);
    }

    static int isFinishedOp(struct rrddim_query_handle *Handle) {
        return GlobalQOp->isFinishedOp(Handle);
    }

    static storage_number nextMetricOp(struct rrddim_query_handle *Handle, time_t *CurrT) {
        return GlobalQOp->nextMetricOp(Handle, CurrT);
    }

    static void finalizeOp(struct rrddim_query_handle *Handle) {
        GlobalQOp->finalizeOp(Handle);
    }

private:
    RRDDIM *RD;
};


TEST(ANomalyDetectorTest, AnomalyEvents) {
    AnomalyDetector AD = AnomalyDetector(0, 4);

    StorageNumbers SNs = { 0, SN_ANOMALOUS, 0, SN_ANOMALOUS, 0 };
    Dimension Dim("TestRD", SNs);

    std::vector<AnomalyEvent> AEV = AD.getAnomalyEvents(Dim.getRD(), 4, 0.5);

    EXPECT_EQ(AEV.size(), 1);
    EXPECT_EQ(AEV[0].first, 0);
    EXPECT_EQ(AEV[0].second, 4);

    std::cout << AEV[0].second << std::endl;
}


int ml_test(int argc, char *argv[]) {
    ::testing::InitGoogleTest(&argc, argv);

    return RUN_ALL_TESTS();
}
