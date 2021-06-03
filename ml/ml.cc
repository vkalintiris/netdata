// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Unit.h"

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
    Cfg.TrainSecs = Millis{config_get_number(CONFIG_SECTION_ML, "num secs to train", 60 * 60) * 1000};
    Cfg.MinTrainSecs = Millis{config_get_number(CONFIG_SECTION_ML, "minimum num secs to train", 40 * 60) * 1000};
    Cfg.TrainEvery = Millis{config_get_number(CONFIG_SECTION_ML, "train every secs", 30 * 60) * 1000};

    Cfg.DiffN = config_get_number(CONFIG_SECTION_ML, "num samples to diff", 1);
    Cfg.SmoothN = config_get_number(CONFIG_SECTION_ML, "num samples to smooth", 3);
    Cfg.LagN = config_get_number(CONFIG_SECTION_ML, "num samples to lag", 5);

    std::string HostsToSkip = config_get(CONFIG_SECTION_ML, "hosts to skip from training", "!*");
    Cfg.SP_HostsToSkip = simple_pattern_create(HostsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    std::string ChartsToSkip = config_get(CONFIG_SECTION_ML, "charts to skip from training", "!system.cpu *");
    Cfg.SP_ChartsToSkip = simple_pattern_create(ChartsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    Cfg.AnomalyScoreThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly score threshold", 0.1);
    Cfg.AnomalousHostRateThreshold = config_get_float(CONFIG_SECTION_ML, "anomalous host at this percent of anomalous units", 1.0);

    Cfg.ADWindowSize = config_get_float(CONFIG_SECTION_ML, "anomaly detector window size", 120);
    Cfg.ADWindowRateThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly detector window rate threshold", 0.25);
    Cfg.ADUnitRateThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly detector unit rate threshold", 0.1);
}

/*
 * Assumptions:
 *  1) hosts outlive their dimensions,
 *  2) dimensions always have a set that has a host.
 */
void ml_new_host(RRDHOST *RH) {
    if (!RH)
        return;

    if (simple_pattern_matches(Cfg.SP_HostsToSkip, RH->hostname))
        return;

    Host *H = new Host(RH);
    H->runMLThreads();

    RH->ml_host = static_cast<ml_host_t>(H);
}

void ml_delete_host(RRDHOST *RH) {
    if (!RH)
        return;

    Host *H = static_cast<Host *>(RH->ml_host);
    if (!H)
        return;

    H->stopMLThreads();

    delete H;
}

void ml_new_unit(RRDDIM *RD) {
    if (!RD)
        return;

    if (simple_pattern_matches(Cfg.SP_ChartsToSkip, RD->rrdset->name))
        return;

    RRDHOST *RH = RD->rrdset->rrdhost;
    Host *H = static_cast<Host *>(RH->ml_host);
    if (!H)
        return;

    Unit *U = new Unit(RD);
    H->addUnit(U);
    RD->state->ml_unit = static_cast<ml_unit_t>(U);
}

void ml_delete_unit(RRDDIM *RD) {
    if (!RD)
        return;

    Unit *U = static_cast<Unit *>(RD->state->ml_unit);
    if (!U)
        return;

    RRDHOST *RH = RD->rrdset->rrdhost;
    Host *H = static_cast<Host *>(RH->ml_host);
    if (!H)
        return;

    H->removeUnit(U);

    delete U;
}

bool ml_is_anomalous(RRDDIM *RD) {
    if (!RD)
        return false;

    Unit *U = static_cast<Unit *>(RD->state->ml_unit);
    if (!U)
        return false;

    U->predict();
    return U->isAnomalous();
}

char *ml_find_anomaly_events(RRDHOST *RH, time_t After, time_t Before) {
    if (!RH)
        return nullptr;

    Host *H = static_cast<Host *>(RH->ml_host);
    if (!H)
        return nullptr;

    std::string JsonResult = H->getAnomalyEventsJson(After, Before);
    return strdup(JsonResult.c_str());
}

char *ml_get_anomaly_event_info(RRDHOST *RH, time_t After, time_t Before) {
    if (!RH)
        return nullptr;

    Host *H = static_cast<Host *>(RH->ml_host);
    if (!H)
        return nullptr;

    std::string JsonResult = H->getAnomalyEventInfoJson(After, Before);
    char *JsonBuffer = new char[JsonResult.length() + 1];
    strcpy(JsonBuffer, JsonResult.c_str());
    return JsonBuffer;
}
