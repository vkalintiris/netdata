// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Database.h"
#include "Host.h"

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
    if (Cfg.Initialized)
        fatal("Global ML configuration has been already initialized");

    Cfg.UpdateEvery = Millis{15 * 1000};
    Cfg.TrainSecs = Millis{config_get_number(CONFIG_SECTION_ML, "num secs to train", 6 * 3600) * 1000};
    Cfg.MinTrainSecs = Millis{config_get_number(CONFIG_SECTION_ML, "minimum num secs to train", 2 * 3600) * 1000};
    Cfg.TrainEvery = Millis{config_get_number(CONFIG_SECTION_ML, "train every secs", 3600) * 1000};

    Cfg.DiffN = config_get_number(CONFIG_SECTION_ML, "num samples to diff", 1);
    Cfg.SmoothN = config_get_number(CONFIG_SECTION_ML, "num samples to smooth", 3);
    Cfg.LagN = config_get_number(CONFIG_SECTION_ML, "num samples to lag", 5);

    std::string HostsToSkip = config_get(CONFIG_SECTION_ML, "hosts to skip from training", "!*");
    Cfg.SP_HostsToSkip = simple_pattern_create(HostsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    std::string ChartsToSkip = config_get(CONFIG_SECTION_ML, "charts to skip from training", "!*");
    Cfg.SP_ChartsToSkip = simple_pattern_create(ChartsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    Cfg.AnomalyScoreThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly score threshold", 0.99);

    Cfg.Initialized = true;
}

void ml_host_create(RRDHOST *RH) {
    if (simple_pattern_matches(Cfg.SP_HostsToSkip, RH->hostname))
        return;

    static std::once_flag ml_init_once_flag;
    std::call_once(ml_init_once_flag, ml_init);

    Host *H = DB.addHost(RH);

    std::thread TrainingThread(&Host::train, H);
    TrainingThread.detach();

    std::thread PredictionThread(&Host::predict, H);
    PredictionThread.detach();
}
