// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Database.h"
#include "Host.h"
#include "Unit.h"

using namespace ml;

/*
 * Global configuration instance to be shared between training and
 * prediction threads.
 */
Config ml::Cfg;

/*
 * Global database instance to be shared between training and
 * prediction threads.
 */
Database ml::DB;

/*
 * Initialize global configuration variable.
 */
void ml_init(void) {
    Cfg.UpdateEvery = Millis{15 * 1000};
    Cfg.TrainSecs = Millis{config_get_number(CONFIG_SECTION_ML, "num secs to train", 60) * 1000};
    Cfg.MinTrainSecs = Millis{config_get_number(CONFIG_SECTION_ML, "minimum num secs to train", 30) * 1000};
    Cfg.TrainEvery = Millis{config_get_number(CONFIG_SECTION_ML, "train every secs", 30) * 1000};

    Cfg.DiffN = config_get_number(CONFIG_SECTION_ML, "num samples to diff", 1);
    Cfg.SmoothN = config_get_number(CONFIG_SECTION_ML, "num samples to smooth", 3);
    Cfg.LagN = config_get_number(CONFIG_SECTION_ML, "num samples to lag", 5);

    std::string HostsToSkip = config_get(CONFIG_SECTION_ML, "hosts to skip from training", "!*");
    Cfg.SP_HostsToSkip = simple_pattern_create(HostsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    std::string ChartsToSkip = config_get(CONFIG_SECTION_ML, "charts to skip from training", "!*");
    Cfg.SP_ChartsToSkip = simple_pattern_create(ChartsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    Cfg.AnomalyScoreThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly score threshold", 0.99);
}

ml_host_handle_t *ml_host_new(RRDHOST *RH) {
    if (!RH)
        return nullptr;

    if (simple_pattern_matches(Cfg.SP_HostsToSkip, RH->hostname))
        return nullptr;

    ml_host_handle_t *handle = new ml_host_handle_t;
    handle->HostPtr = new Host(RH);
    return handle;
}

void ml_host_delete(ml_host_handle_t *host_handle) {
    if (!host_handle)
        return;

    delete static_cast<Host *>(host_handle->HostPtr);
    delete host_handle;
}

ml_unit_handle_t *ml_unit_new(RRDDIM *RD) {
    if (!RD)
        return nullptr;

    if (simple_pattern_matches(Cfg.SP_ChartsToSkip, RD->rrdset->name))
        return nullptr;

    ml_unit_handle_t *handle = new ml_unit_handle_t;
    handle->UnitPtr = new Unit(RD);
    return handle;
}

void ml_unit_delete(ml_unit_handle_t *unit_handle) {
    delete static_cast<Unit *>(unit_handle->UnitPtr);
    delete unit_handle;
}
