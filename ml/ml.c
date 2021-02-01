// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

ml_config_t ml_cfg = { 0 };

#define CHARTS_TO_SKIP_PATTERN \
    "!system.cpu !system.io !system.pgpgio !system.ram !system.swap !system.swapio " \
    "!apps.cpu !apps.cpu_user !apps.cpu_system !apps.mem " \
    "!cpu.cpufreq !mem.kernel *"

#define CHARTS_TO_TRAIN_PER_DIM_PATTERN \
    "system.cpu system.io system.pgpgio system.ram system.swap system.swapio " \
    "apps.cpu apps.cpu_user apps.cpu_system apps.mem " \
    "cpu.cpufreq mem.kernel !*"

void
ml_init(void)
{
    if (ml_cfg.initialized)
        return;

    info("Initializing ML configuration");

    ml_cfg.train_secs = config_get_number(CONFIG_SECTION_ML, "num secs to train", 600);
    ml_cfg.train_every = config_get_number(CONFIG_SECTION_ML, "train every secs", 60);

    ml_cfg.diff_n = config_get_number(CONFIG_SECTION_ML, "num samples to diff", 1);
    ml_cfg.smooth_n = config_get_number(CONFIG_SECTION_ML, "num samples to smooth", 3);
    ml_cfg.lag_n = config_get_number(CONFIG_SECTION_ML, "num samples to lag", 5);

    ml_cfg.train_heartbeat = config_get_number(CONFIG_SECTION_ML, "train loop every", 10);
    ml_cfg.train_heartbeat *= USEC_PER_SEC;

    ml_cfg.predict_heartbeat = config_get_number(CONFIG_SECTION_ML, "predict loop every", 1);
    ml_cfg.predict_heartbeat *= USEC_PER_SEC;

    // Charts to skip
    const char *skip_charts = config_get(CONFIG_SECTION_ML,
            "charts to skip from training", CHARTS_TO_SKIP_PATTERN);
    ml_cfg.skip_charts = simple_pattern_create(skip_charts, NULL, SIMPLE_PATTERN_EXACT);

    // Charts to train per dim
    const char *train_per_dim = config_get(CONFIG_SECTION_ML,
            "charts to train per dim", CHARTS_TO_TRAIN_PER_DIM_PATTERN);
    ml_cfg.train_per_dim = simple_pattern_create(train_per_dim, NULL, SIMPLE_PATTERN_EXACT);

    // Anomaly score charts
    const char *anomaly_score_charts = config_get(CONFIG_SECTION_ML, "anomaly score charts",
            "system.cpu disk.nvme0n1 apps.cpu !*");
    ml_cfg.anomaly_score_charts = simple_pattern_create(anomaly_score_charts, NULL, SIMPLE_PATTERN_EXACT);

    uint8_t dict_flags = DICTIONARY_FLAG_SINGLE_THREADED |
                         DICTIONARY_FLAG_VALUE_LINK_DONT_CLONE |
                         DICTIONARY_FLAG_WITH_STATISTICS;
    ml_cfg.train_dict = dictionary_create(dict_flags);

    ml_cfg.ml_charts_dict = dictionary_create(dict_flags);

    netdata_rwlock_init(&ml_cfg.predict_dict_rwlock);

    ml_cfg.initialized = true;
}

bool
ml_heartbeat(size_t secs)
{
    static heartbeat_t hb;
    static bool initialized = false;

    if (!initialized) {
        heartbeat_init(&hb);
        initialized = true;
    }

    heartbeat_next(&hb, secs);
    return !netdata_exit;
}

void *
ml_main(void *ptr)
{
    struct netdata_static_thread *thr = ptr;

    sleep(5);

    if (!strcmp(thr->name, "MLTRAIN"))
        ml_train_main(thr);
    else
        ml_predict_main(thr);

    return NULL;
}
