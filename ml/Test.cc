// SPDX-License-Identifier: GPL-3.0-or-later

#include "config.h"

#include "ml-private.h"

#include "AnomalyDetector.h"
#include "Unit.h"
#include "Host.h"
#include "Query.h"
#include "RollingBitCounter.h"

#include "gtest/gtest.h"
#include "gmock/gmock.h"


using namespace ml;

using StorageNumbers = std::vector<storage_number>;
using StorageNumbersIterator = StorageNumbers::iterator;

class QueryOp {
public:
    QueryOp(StorageNumbers &SNs)
        : SNs(SNs) { }

    time_t latestTimeOp(RRDDIM *RD) {
        (void) RD;

        return SNs.size() ? SNs.size() - 1 : 0;
    }

    time_t oldestTimeOp(RRDDIM *RD) {
        (void) RD;

        return 0;
    }

    void initOp(RRDDIM *RD, struct rrddim_query_handle *Handle, time_t AfterT, time_t BeforeT) {
        (void) RD, (void) Handle;

        IterAfterT = SNs.begin() + AfterT;
        IterBeforeT = SNs.begin() + BeforeT + 1;

        Iter = SNs.begin() + AfterT;
    }

    int isFinishedOp(struct rrddim_query_handle *Handle) {
        (void) Handle;

        return std::distance(Iter, IterBeforeT) == 0;
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
    StorageNumbersIterator IterAfterT;
    StorageNumbersIterator IterBeforeT;
};

static QueryOp *GlobalQOp;

class Dimension {
public:
    Dimension(std::string Name, StorageNumbers &SNs) : Name(Name) {
        RD = new RRDDIM;

        RD->name = this->Name.c_str();

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
    std::string Name;
    RRDDIM *RD;
};

#if 0
TEST(AnomalyDetectorTest, AnomalyEvents) {
    AnomalyDetector AD = AnomalyDetector(0, 4);
    std::vector<AnomalyEvent> AEV;
    
    StorageNumbers SNs;
    Dimension Dim("TestRD", SNs);

    SNs = { 0, 0, 0, 0, 0 };
    
    AEV = AD.getAnomalyEvents(Dim.getRD(), 0, 1.0);
    EXPECT_EQ(AEV.size(), 0);
    AEV = AD.getAnomalyEvents(Dim.getRD(), 1, 1.0);
    EXPECT_EQ(AEV.size(), 0);

    SNs = { SN_ANOMALOUS, 0, 0, 0, 0 };
    
    AEV = AD.getAnomalyEvents(Dim.getRD(), 1, 1.0);
    EXPECT_EQ(AEV.size(), 1);
    EXPECT_EQ(AEV[0], AnomalyEvent(0, 0));

    SNs = { 0, 0, 0, 0, SN_ANOMALOUS };
    AEV = AD.getAnomalyEvents(Dim.getRD(), 1, 1.0);
    EXPECT_EQ(AEV.size(), 1);
    EXPECT_EQ(AEV[0], AnomalyEvent(SNs.size() -1 , SNs.size() - 1));

    SNs = { SN_ANOMALOUS, SN_ANOMALOUS, SN_ANOMALOUS, SN_ANOMALOUS, SN_ANOMALOUS };

    AEV = AD.getAnomalyEvents(Dim.getRD(), 5, 1.0);
    EXPECT_EQ(AEV.size(), 1);
    EXPECT_EQ(AEV[0], AnomalyEvent(0 , SNs.size() - 1));

    AEV = AD.getAnomalyEvents(Dim.getRD(), 1, 1.0);
    EXPECT_EQ(AEV.size(), SNs.size());
    for (unsigned Idx = 0; Idx != SNs.size(); Idx++)
        EXPECT_EQ(AEV[Idx], AnomalyEvent(Idx, Idx));

    AEV = AD.getAnomalyEvents(Dim.getRD(), 2, 1.0);
    EXPECT_EQ(AEV.size(), 1);
    EXPECT_EQ(AEV[0], AnomalyEvent(0, SNs.size() - 1));

    SNs = { SN_ANOMALOUS, SN_ANOMALOUS, 0, 0, 0 };

    AEV = AD.getAnomalyEvents(Dim.getRD(), 2, 1.0);
    EXPECT_EQ(AEV.size(), 1);
    EXPECT_EQ(AEV[0], AnomalyEvent(0, 1));

    AEV = AD.getAnomalyEvents(Dim.getRD(), 3, 0.5);
    EXPECT_EQ(AEV.size(), 1);
    EXPECT_EQ(AEV[0], AnomalyEvent(0, 2));

    SNs = { 0, SN_ANOMALOUS, 0, SN_ANOMALOUS, 0 };
    AEV = AD.getAnomalyEvents(Dim.getRD(), 2, 0.5);
    EXPECT_EQ(AEV.size(), 1);
    EXPECT_EQ(AEV[0], AnomalyEvent(0, SNs.size() - 1));

    SNs = { 0, SN_ANOMALOUS, 0, 0,  SN_ANOMALOUS };
    
    AEV = AD.getAnomalyEvents(Dim.getRD(), 2, 0.5);
    EXPECT_EQ(AEV.size(), 2);
    EXPECT_EQ(AEV[0], AnomalyEvent(0, 2));
    EXPECT_EQ(AEV[1], AnomalyEvent(3, 4));

    SNs = { 0, 0, 0, 0, SN_ANOMALOUS };
    
    AEV = AD.getAnomalyEvents(Dim.getRD(), SNs.size(), 1.0 / SNs.size());
    EXPECT_EQ(AEV.size(), 1);
    EXPECT_EQ(AEV[0], AnomalyEvent(0, SNs.size() - 1));
}

TEST(AnomalyDetectorTest, AnomalyEventInfo) {
    AnomalyDetector AD = AnomalyDetector(0, 3);
    AnomalyEventInfo AEI;
    
    StorageNumbers SNs;
    Dimension Dim("TestRD", SNs);

    SNs = { 0, 0, SN_ANOMALOUS, SN_ANOMALOUS };
    
    AEI = AD.getAnomalyEventInfo(Dim.getRD());
    EXPECT_EQ(AEI.Name, "TestRD");
    EXPECT_EQ(AEI.AnomalyRate, 0.5);
    EXPECT_THAT(AEI.AnomalyStatus, testing::ElementsAre(0, 0, 1, 1));

    SNs = { 0, 0, SN_ANOMALOUS, SN_ANOMALOUS, SN_ANOMALOUS };
    
    AEI = AD.getAnomalyEventInfo(Dim.getRD());
    EXPECT_EQ(AEI.Name, "TestRD");
    EXPECT_EQ(AEI.AnomalyRate, 0.5);
    EXPECT_THAT(AEI.AnomalyStatus, testing::ElementsAre(0, 0, 1, 1));

    AD = AnomalyDetector(1, 3);

    SNs = { 0, 0, SN_ANOMALOUS, SN_ANOMALOUS };
    
    AEI = AD.getAnomalyEventInfo(Dim.getRD());
    EXPECT_EQ(AEI.Name, "TestRD");
    EXPECT_EQ(AEI.AnomalyRate, 2.0 / 3);
    EXPECT_THAT(AEI.AnomalyStatus, testing::ElementsAre(0, 1, 1));

    SNs = { 0, 0, SN_ANOMALOUS, SN_ANOMALOUS, SN_ANOMALOUS };
    
    AEI = AD.getAnomalyEventInfo(Dim.getRD());
    EXPECT_EQ(AEI.Name, "TestRD");
    EXPECT_EQ(AEI.AnomalyRate, 2.0 / 3);
    EXPECT_THAT(AEI.AnomalyStatus, testing::ElementsAre(0, 1, 1));
}
#endif

TEST(RollingBitCounterTest, RollingBitCounter) {
    RollingBitCounter RBC{4};

    RBC.insert(0);
    EXPECT_EQ(RBC.numSetBits(), 0);

    RBC.insert(0);
    EXPECT_EQ(RBC.numSetBits(), 0);

    RBC.insert(1);
    EXPECT_EQ(RBC.numSetBits(), 1);

    RBC.insert(1);
    EXPECT_EQ(RBC.numSetBits(), 2);

    RBC.insert(0);
    EXPECT_EQ(RBC.numSetBits(), 2);

    RBC.insert(1);
    EXPECT_EQ(RBC.numSetBits(), 3);

    RBC.insert(0);
    EXPECT_EQ(RBC.numSetBits(), 2);

    RBC.insert(0);
    EXPECT_EQ(RBC.numSetBits(), 1);

    RBC.insert(0);
    EXPECT_EQ(RBC.numSetBits(), 1);

    RBC.insert(1);
    EXPECT_EQ(RBC.numSetBits(), 1);

    RBC.insert(0);
    EXPECT_EQ(RBC.numSetBits(), 1);

    RBC.insert(1);
    EXPECT_EQ(RBC.numSetBits(), 2);

    RBC.insert(0);
    EXPECT_EQ(RBC.numSetBits(), 2);

    RBC.insert(0);
    EXPECT_EQ(RBC.numSetBits(), 1);
}

TEST(RollingBitWindowTest, RollingBitWindow) {
    std::vector<bool> V{0, 0, 1, 1, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0};

    std::vector<size_t> WindowLengths;
    RollingBitWindow RBW{4, 2};

    auto insertBit = [&](bool B) {
        auto P = RBW.insert(B);
        auto Edge = P.first;
        auto Length = P.second;

        if (Edge.first == RollingBitWindow::State::AboveThreshold &&
            Edge.second == RollingBitWindow::State::BelowThreshold) {
            WindowLengths.push_back(Length);
        }
    };

    std::for_each(V.cbegin(), V.cend(), insertBit);

    EXPECT_EQ(WindowLengths.size(), 2);
    EXPECT_EQ(WindowLengths[0], 7); // 0 0 1 1 0 1 0
    EXPECT_EQ(WindowLengths[1], 5); // 0 1 0 1 0

    RBW = RollingBitWindow(4, 3);
    WindowLengths.clear();
    std::for_each(V.cbegin(), V.cend(), insertBit);

    EXPECT_EQ(WindowLengths.size(), 1);
    EXPECT_EQ(WindowLengths[0], 4); // 1 1 0 1

    RBW = RollingBitWindow(4, 4);
    WindowLengths.clear();
    std::for_each(V.cbegin(), V.cend(), insertBit);

    EXPECT_EQ(WindowLengths.size(), 0);
}

int ml_test(int argc, char *argv[]) {
    ::testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
