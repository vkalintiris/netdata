// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Dimension.h"
#include "Database.h"

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
#if 1
    Cfg.TrainSecs = Seconds{config_get_number(CONFIG_SECTION_ML, "num secs to train", 4 * 60)};
    Cfg.MinTrainSecs = Seconds{config_get_number(CONFIG_SECTION_ML, "minimum num secs to train", 1 * 60)};
    Cfg.TrainEvery = Seconds{config_get_number(CONFIG_SECTION_ML, "train every secs", 1 * 60 )};

    Cfg.DiffN = config_get_number(CONFIG_SECTION_ML, "num samples to diff", 1);
    Cfg.SmoothN = config_get_number(CONFIG_SECTION_ML, "num samples to smooth", 3);
    Cfg.LagN = config_get_number(CONFIG_SECTION_ML, "num samples to lag", 5);

    std::string HostsToSkip = config_get(CONFIG_SECTION_ML, "hosts to skip from training", "!*");
    Cfg.SP_HostsToSkip = simple_pattern_create(HostsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    std::string ChartsToSkip = config_get(CONFIG_SECTION_ML, "charts to skip from training", "!*");
    Cfg.SP_ChartsToSkip = simple_pattern_create(ChartsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    Cfg.DimensionAnomalyScoreThreshold = config_get_float(CONFIG_SECTION_ML, "dimension anomaly score threshold", 0.99);
    Cfg.HostAnomalyRateThreshold = config_get_float(CONFIG_SECTION_ML, "host anomaly rate threshold", 0.2);

    Cfg.ADWindowSize = config_get_float(CONFIG_SECTION_ML, "window min size", 30);
    Cfg.ADWindowRateThreshold = config_get_float(CONFIG_SECTION_ML, "window min anomaly rate", 0.25);
    Cfg.ADDimensionRateThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly event min dimension rate threshold", 0.1);

    Cfg.EnableMLCharts = config_get_boolean(CONFIG_SECTION_ML, "enable ml charts", true);
#else
    Cfg.TrainSecs = Seconds{config_get_number(CONFIG_SECTION_ML, "num secs to train", 2 * 60)};
    Cfg.MinTrainSecs = Seconds{config_get_number(CONFIG_SECTION_ML, "minimum num secs to train", 60)};
    Cfg.TrainEvery = Seconds{config_get_number(CONFIG_SECTION_ML, "train every secs", 60)};

    Cfg.DiffN = config_get_number(CONFIG_SECTION_ML, "num samples to diff", 1);
    Cfg.SmoothN = config_get_number(CONFIG_SECTION_ML, "num samples to smooth", 3);
    Cfg.LagN = config_get_number(CONFIG_SECTION_ML, "num samples to lag", 5);

    std::string HostsToSkip = config_get(CONFIG_SECTION_ML, "hosts to skip from training", "!*");
    Cfg.SP_HostsToSkip = simple_pattern_create(HostsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    std::string ChartsToSkip = config_get(CONFIG_SECTION_ML, "charts to skip from training", "!*");
    Cfg.SP_ChartsToSkip = simple_pattern_create(ChartsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    Cfg.DimensionAnomalyScoreThreshold = config_get_float(CONFIG_SECTION_ML, "dimension anomaly score threshold", 0.99);
    Cfg.HostAnomalyRateThreshold = config_get_float(CONFIG_SECTION_ML, "host anomaly rate threshold", 0.01);

    Cfg.ADWindowSize = config_get_float(CONFIG_SECTION_ML, "window min size", 120);
    Cfg.ADWindowRateThreshold = config_get_float(CONFIG_SECTION_ML, "window min anomaly rate", 0.25);
    Cfg.ADDimensionRateThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly event min dimension rate threshold", 0.1);
#endif

    // ML database path
    std::stringstream SS;
    SS << netdata_configured_cache_dir << "/" << "netdata-ml.db";
    Cfg.AnomalyDBPath = SS.str();
}

/*
 * Assumptions:
 *  1) hosts outlive their charts, and charts outlive their dimensions,
 *  2) dimensions always have a set that has a host.
 */
void ml_new_host(RRDHOST *RH) {
    if (simple_pattern_matches(Cfg.SP_HostsToSkip, RH->hostname))
        return;

    Host *H = new Host(RH);
    RH->ml_host = static_cast<ml_host_t>(H);

    H->startAnomalyDetectionThreads();
}

void ml_delete_host(RRDHOST *RH) {
    Host *H = static_cast<Host *>(RH->ml_host);
    if (!H)
        return;

    H->stopAnomalyDetectionThreads();
    delete H;
    RH->ml_host = nullptr;
}

void ml_new_unit(RRDDIM *RD) {
    RRDSET *RS = RD->rrdset;

    if (RS->update_every != 1)
        return;

    if (simple_pattern_matches(Cfg.SP_ChartsToSkip, RS->name))
        return;

    if (strstr(RS->name, "ml_prediction_info") || strstr(RS->name, "ml_detector_info"))
        return;

    Host *H = static_cast<Host *>(RD->rrdset->rrdhost->ml_host);
    if (!H)
        return;

    Dimension *D = new Dimension(RD);
    RD->state->ml_unit = static_cast<ml_unit_t>(D);
    H->addDimension(D);
}

void ml_delete_unit(RRDDIM *RD) {
    Dimension *D = static_cast<Dimension *>(RD->state->ml_unit);
    if (!D)
        return;

    Host *H = static_cast<Host *>(RD->rrdset->rrdhost->ml_host);
    H->removeDimension(D);

    delete D;
    RD->state->ml_unit = nullptr;
}

bool ml_is_anomalous(RRDDIM *RD, double Value, bool Exists) {
    Dimension *D = static_cast<Dimension *>(RD->state->ml_unit);
    if (!D)
        return false;

    D->addValue(Value, Exists);
    return D->predict().second;
}

char *ml_get_anomaly_events(const char *AnomalyDetectorName,
                            int AnomalyDetectorVersion,
                            RRDHOST *RH,
                            time_t After, time_t Before)
{
    if (!RH) {
        error("No host");
        return nullptr;
    }

    if (!RH->ml_host) {
        error("No ML host");
        return nullptr;
    }

    Host *H = static_cast<Host *>(RH->ml_host);
    std::vector<std::pair<time_t, time_t>> TimeRanges;

    Database DB{Cfg.AnomalyDBPath};
    bool Res = DB.getAnomaliesInRange(TimeRanges, AnomalyDetectorName,
                                                  AnomalyDetectorVersion,
                                                  H->getUUID(),
                                                  After, Before);
    if (!Res) {
        error("DB result is empty");
        return nullptr;
    }

    nlohmann::json Json = TimeRanges;
    return strdup(Json.dump(4).c_str());
}

char *ml_get_anomaly_event_info(const char *AnomalyDetectorName,
                                int AnomalyDetectorVersion,
                                RRDHOST *RH,
                                time_t After, time_t Before)
{
    if (!RH) {
        error("No host");
        return nullptr;
    }

    if (!RH->ml_host) {
        error("No ML host");
        return nullptr;
    }

    Host *H = static_cast<Host *>(RH->ml_host);

    Database DB{Cfg.AnomalyDBPath};
    nlohmann::json Json;
    bool Res = DB.getAnomalyInfo(Json, AnomalyDetectorName,
                                       AnomalyDetectorVersion,
                                       H->getUUID(),
                                       After, Before);
    if (!Res) {
        error("DB result is empty");
        return nullptr;
    }

    return strdup(Json.dump(4).c_str());
}
