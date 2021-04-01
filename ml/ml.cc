// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Database.h"

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
        return;

    Cfg.UpdateEvery = Millis{15 * 1000};
    Cfg.TrainSecs = Millis{config_get_number(CONFIG_SECTION_ML, "num secs to train", 60 * 60) * 1000};
    Cfg.TrainEvery = Millis{config_get_number(CONFIG_SECTION_ML, "train every secs", 30 * 60) * 1000};

    Cfg.DiffN = config_get_number(CONFIG_SECTION_ML, "num samples to diff", 1);
    Cfg.SmoothN = config_get_number(CONFIG_SECTION_ML, "num samples to smooth", 3);
    Cfg.LagN = config_get_number(CONFIG_SECTION_ML, "num samples to lag", 5);

    std::string HostsToSkip = config_get(CONFIG_SECTION_ML, "hosts to skip from training", "!*");
    Cfg.SP_HostsToSkip = simple_pattern_create(HostsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    std::string ChartsToSkip = config_get(CONFIG_SECTION_ML, "charts to skip from training", "!*");
    Cfg.SP_ChartsToSkip = simple_pattern_create(ChartsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    Cfg.Initialized = true;
}

/*
 * Main entry point
 */
void *ml_main(void *Ptr) {
    struct netdata_static_thread *Thread = (struct netdata_static_thread *) Ptr;

    std::this_thread::sleep_for(Cfg.UpdateEvery);

    std::string ThreadName = Thread->name;

    if (ThreadName.compare("MLTRAIN") == 0)
        ml::trainMain(Thread);
    else
        ml::predictMain(Thread);

    return NULL;
}
