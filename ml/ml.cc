// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

using namespace ml;

/*
 * Global configuration instance to be shared between training and
 * prediction threads.
 */
Config ml::Cfg;

/*
 * Copy of the unpack_storage_number which allows us to convert
 * a storage_number to double.
 */
CalculatedNumber unpack_storage_number_dbl(storage_number value) {
    if(!value)
        return 0;

    int sign = 0, exp = 0;
    int factor = 10;

    // bit 32 = 0:positive, 1:negative
    if(unlikely(value & (1 << 31)))
        sign = 1;

    // bit 31 = 0:divide, 1:multiply
    if(unlikely(value & (1 << 30)))
        exp = 1;

    // bit 27 SN_EXISTS_100
    if(unlikely(value & (1 << 26)))
        factor = 100;

    // bit 26 SN_EXISTS_RESET
    // bit 25 SN_EXISTS

    // bit 30, 29, 28 = (multiplier or divider) 0-7 (8 total)
    int mul = (value & ((1<<29)|(1<<28)|(1<<27))) >> 27;

    // bit 24 to bit 1 = the value, so remove all other bits
    value ^= value & ((1<<31)|(1<<30)|(1<<29)|(1<<28)|(1<<27)|(1<<26)|(1<<25)|(1<<24));

    CalculatedNumber CN = value;

    if(exp) {
        for(; mul; mul--)
            CN *= factor;
    }
    else {
        for( ; mul ; mul--)
            CN /= 10;
    }

    if(sign)
        CN = -CN;

    return CN;
}

/*
 * Get a buffer of values contained in the window.
 */
CalculatedNumber *ml::Window::getCalculatedNumbers() {
    assert(std::numeric_limits<CalculatedNumber>::has_quiet_NaN);

    CalculatedNumber *CNs = new CalculatedNumber[NumSamples * (Cfg.LagN + 1)]();

    RRDDIM *RD = U->getDim();
    struct rrddim_volatile::rrddim_query_ops *Ops = &RD->state->query_ops;
    struct rrddim_query_handle Handle;

    /*
     * Figure out what our time window should be.
     */

    time_t BeforeT = now_realtime_sec() - 1;
    time_t AfterT = BeforeT - (NumSamples * U->updateEvery());

    BeforeT -=  (BeforeT % U->updateEvery());
    AfterT -= (AfterT % U->updateEvery());

    time_t LastT = Ops->latest_time(RD);
    BeforeT = (BeforeT > LastT) ? LastT : BeforeT;

    time_t FirstT = Ops->oldest_time(RD);
    AfterT = (AfterT < FirstT) ? FirstT : AfterT;

    if (AfterT >= BeforeT)
        return CNs;

    /*
     * Start the query.
     */

    unsigned Idx = 0;

    std::stringstream SS;

    Ops->init(RD, &Handle, AfterT, BeforeT);
    while (!Ops->is_finished(&Handle)) {
        time_t CurrT;

        storage_number SN = Ops->next_metric(&Handle, &CurrT);

        if (!does_storage_number_exist(SN)) {
            NumEmpty++;
            CNs[Idx] = std::numeric_limits<CalculatedNumber>::quiet_NaN();
        } else if (did_storage_number_reset(SN)) {
            NumReset++;
        }

        CNs[Idx] = unpack_storage_number_dbl(SN);

        Idx += 1;
        if (Idx == NumSamples)
            break;
    }
    Ops->finalize(&Handle);

    NumCollected = Idx;

    if (NumEmpty == 0 && NumCollected == NumSamples)
        return CNs;

    /*
     * Pack numbers that are not NaNs to the beginning of the array.
     */

    unsigned OldIdx, NewIdx;

    for (OldIdx = 0, NewIdx = 0; OldIdx != NumSamples && NewIdx != NumCollected; OldIdx++) {
        if (std::isnan(CNs[OldIdx]))
            continue;

        CNs[NewIdx++] = OldIdx;
    }

    assert(NewIdx == NumSamples || NewIdx == NumCollected);
    return CNs;
}

/*
 * Run KMeans on the unit.
 */
bool ml::Unit::train() {
    unsigned NumSamples = Cfg.TrainSecs / updateEvery();

    Window W = Window(this, NumSamples);
    CalculatedNumber *CNs = W.getCalculatedNumbers();

    LastTrainedAt = now_realtime_sec();

    if (W.ratioFilled() < 0.8) {
        info("%s - sparse training window: %lf", c_uid(), W.ratioFilled());
        Trained = false;
    } else {
        SamplesBuffer SB = SamplesBuffer(CNs, W.NumCollected, 1,
                                         Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);
        KM.train(SB);
        Trained = true;
    }

    delete[] CNs;
    return Trained;
}

/*
 * Calculate the anomaly score of the unit.
 */
bool ml::Unit::predict() {
    if (!Trained)
        return false;

    unsigned NumSamples = Cfg.DiffN + Cfg.SmoothN + Cfg.LagN;

    Window W = Window(this, NumSamples);
    CalculatedNumber *CNs = W.getCalculatedNumbers();

    if (W.NumCollected != W.NumSamples) {
        info("%s - sparse prediction window: %lf", c_uid(), W.ratioFilled());
        Predicted = false;
    } else {
        SamplesBuffer SB = SamplesBuffer(CNs, W.NumCollected, 1,
                                         Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);

        // Waiting for the next iteration is fine.
        AnomalyScore = KM.anomalyScore(SB);
        Predicted = true;
    }

    delete[] CNs;
    return true;
}

/*
 * Create/Update the ML set that will contain the anomaly scores of each
 * unit in this chart.
 */
void Chart::updateMLChart() {
    if (MLRS) {
        // Update each dimension with the anomaly score of each unit.
        rrdset_next(MLRS);
        for (auto &P : UnitsMap) {
            Unit *U = P.second;
            U->updateMLUnit(MLRS);
        }
        rrdset_done(MLRS);
        return;
    }

    /*
     * Create a new ML set.
     */

    std::string Name = std::string(RS->name);
    std::string Dot(".");
    std::size_t Pos = Name.find(Dot);

    if (Pos == std::string::npos) {
        info("Could not find set name: %s", RS->name);
        return;
    }

    Name = Name.substr(Pos + 1, Name.npos) + "_km";

    // Use properties of the wrapped set to make the ML set appear
    // next to the wrapped set.
    MLRS = rrdset_create_localhost(
            RS->type,           // type
            Name.c_str(),       // id
            NULL,               // name
            RS->family,         // family
            NULL,               // context
            "Anomaly score",    // title
            "percentage",       // units
            RS->plugin_name,    // plugin
            RS->module_name,    // module
            RS->priority,       // priority
            1,                  // update_every
            RRDSET_TYPE_LINE    // chart_type
            );

    // Create a dim for each unit in the chart.
    for (auto &P : UnitsMap) {
        Unit *U = P.second;
        U->updateMLUnit(MLRS);
    }

    Cfg.MLSets.insert(MLRS);
    return;
}

/*
 * Update the units referenced by the chart.
 */
void Chart::updateUnits() {
    RRDDIM *RD;

    netdata_rwlock_wrlock(&UnitsLock);
    rrdset_rdlock(RS);

    rrddim_foreach_read(RD, RS) {
        bool IsObsolete = rrddim_flag_check(RD, RRDDIM_FLAG_ARCHIVED) ||
                          rrddim_flag_check(RD, RRDDIM_FLAG_OBSOLETE);
        if (IsObsolete) {
            UnitsMap.erase(RD);
            continue;
        }

        std::map<RRDDIM *, Unit *>::iterator It = UnitsMap.find(RD);
        if (It == UnitsMap.end())
            UnitsMap[RD] = new Unit(RD);
    }

    rrdset_unlock(RS);
    netdata_rwlock_unlock(&UnitsLock);
}

/*
 * Update the charts referenced by the host.
 */
void Host::updateCharts() {
    RRDSET *RS;

    netdata_rwlock_wrlock(&Cfg.ChartsMapLock);
    rrdhost_rdlock(RH);

    rrdset_foreach_read(RS, RH) {
        if (Cfg.MLSets.count(RS))
            continue;

        if (simple_pattern_matches(Cfg.SP_ChartsToSkip, RS->name))
            continue;

        bool IsObsolete = rrdset_flag_check(RS, RRDSET_FLAG_ARCHIVED) ||
            rrdset_flag_check(RS, RRDSET_FLAG_OBSOLETE);

        if (IsObsolete) {
            ChartsMap.erase(RS);
            continue;
        }

        std::map<RRDSET *, Chart *>::iterator It = ChartsMap.find(RS);
        if (It == ChartsMap.end())
            ChartsMap[RS] = new Chart(RS);

        ChartsMap[RS]->updateUnits();
    }

    rrdhost_unlock(RH);
    netdata_rwlock_unlock(&Cfg.ChartsMapLock);
}

/*
 * Initialize global configuration variable.
 */
void ml_init(void) {
    if (Cfg.Initialized)
        return;

    Cfg.TrainSecs = config_get_number(CONFIG_SECTION_ML, "num secs to train", 60 * 60);
    Cfg.TrainEvery = config_get_number(CONFIG_SECTION_ML, "train every secs", 15 * 60);

    Cfg.DiffN = config_get_number(CONFIG_SECTION_ML, "num samples to diff", 1);
    Cfg.SmoothN = config_get_number(CONFIG_SECTION_ML, "num samples to smooth", 3);
    Cfg.LagN = config_get_number(CONFIG_SECTION_ML, "num samples to lag", 5);

    std::string ChartsToSkip = config_get(CONFIG_SECTION_ML,
            "charts to skip from training", "!*");
    Cfg.SP_ChartsToSkip = simple_pattern_create(
            ChartsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    Cfg.DisablePredictionThread = config_get_number(CONFIG_SECTION_ML, "disable prediction thread", 0);

    netdata_rwlock_init(&Cfg.ChartsMapLock);

    Cfg.Initialized = true;
}

/*
 * Main entry point
 */
void *ml_main(void *Ptr) {
    struct netdata_static_thread *Thread = (struct netdata_static_thread *) Ptr;

    // Wait for agent to initalize sets.
    sleep(30);

    // Get the thread's name and switch to the proper sub-main function.
    std::string ThreadName = Thread->name;

    if (ThreadName.compare("MLTRAIN") == 0)
        ml::trainMain(Thread);
    else if (!Cfg.DisablePredictionThread)
        ml::predictMain(Thread);

    return NULL;
}
