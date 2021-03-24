// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

using namespace ml;

/*
 * Global configuration instance to be shared between training and
 * prediction threads.
 */
Config ml::Cfg;

/*
 * Initialize global configuration variable.
 */
void ml_init(void) {
    if (Cfg.Initialized)
        return;

    Cfg.TrainSecs = config_get_number(CONFIG_SECTION_ML, "num secs to train", 2 * 60);
    Cfg.TrainEvery = config_get_number(CONFIG_SECTION_ML, "train every secs", 1 * 60);

    Cfg.DiffN = config_get_number(CONFIG_SECTION_ML, "num samples to diff", 1);
    Cfg.SmoothN = config_get_number(CONFIG_SECTION_ML, "num samples to smooth", 3);
    Cfg.LagN = config_get_number(CONFIG_SECTION_ML, "num samples to lag", 5);

    std::string ChartsToSkip = config_get(CONFIG_SECTION_ML,
            "charts to skip from training", "!system.cpu *");
    Cfg.SP_ChartsToSkip = simple_pattern_create(
            ChartsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    Cfg.DisablePredictionThread = config_get_number(CONFIG_SECTION_ML, "disable prediction thread", 0);

    netdata_rwlock_init(&Cfg.HostsLock);

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
