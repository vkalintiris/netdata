// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Chart.h"
#include "Unit.h"
#include "Database.h"
#include "Tracer.h"

using namespace ml;


static constexpr size_t SpdrBufferLen = 512 * 1024 * 1024;
static std::vector<char> SpdrBuffer(SpdrBufferLen);

#if 0
static void sdprReport(const char* Line, void* UserData)
{
    FILE* FP = static_cast<FILE *>(UserData);
    fputs(Line, FP);
}
#endif

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
    Cfg.MinTrainSecs = Seconds{config_get_number(CONFIG_SECTION_ML, "minimum num secs to train", 60)};
    Cfg.TrainEvery = Seconds{config_get_number(CONFIG_SECTION_ML, "train every secs", 60)};

    Cfg.DiffN = config_get_number(CONFIG_SECTION_ML, "num samples to diff", 1);
    Cfg.SmoothN = config_get_number(CONFIG_SECTION_ML, "num samples to smooth", 3);
    Cfg.LagN = config_get_number(CONFIG_SECTION_ML, "num samples to lag", 5);

    std::string HostsToSkip = config_get(CONFIG_SECTION_ML, "hosts to skip from training", "!*");
    Cfg.SP_HostsToSkip = simple_pattern_create(HostsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    std::string ChartsToSkip = config_get(CONFIG_SECTION_ML, "charts to skip from training", "!*");
    Cfg.SP_ChartsToSkip = simple_pattern_create(ChartsToSkip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    Cfg.AnomalyScoreThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly score threshold", 99);
    Cfg.AnomalyRateThreshold = config_get_float(CONFIG_SECTION_ML, "anomalous host at this percent of anomalous units", 0.2);

    Cfg.ADWindowSize = config_get_float(CONFIG_SECTION_ML, "anomaly detector window size", 30);
    Cfg.ADWindowRateThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly detector window min anomaly rate", 0.25);
    Cfg.ADUnitRateThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly detector unit rate threshold", 0.1);
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

    Cfg.AnomalyScoreThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly score threshold", 99);
    Cfg.AnomalyRateThreshold = config_get_float(CONFIG_SECTION_ML, "anomalous host at this of anomalous units", 0.01);

    Cfg.ADWindowSize = config_get_float(CONFIG_SECTION_ML, "anomaly detector window size", 120);
    Cfg.ADWindowRateThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly detector window min anomaly rate", 0.25);
    Cfg.ADUnitRateThreshold = config_get_float(CONFIG_SECTION_ML, "anomaly detector unit rate threshold", 0.1);
#endif

    // ML database path
    std::stringstream SS;
    SS << netdata_configured_cache_dir << "/" << "netdata-ml.db";
    Cfg.AnomalyDBPath = SS.str();

    spdr_init(&Cfg.SpdrCtx, &SpdrBuffer.front(), SpdrBufferLen);
    spdr_enable_trace(Cfg.SpdrCtx, 1);

    Cfg.LogFP = fopen("/tmp/ml-trace.json", "wb");
    if (!Cfg.LogFP)
        fatal("Could not open log file");
    //spdr_set_log_fn(Cfg.SpdrCtx, sdprReport, FP);
}

/*
 * Assumptions:
 *  1) hosts outlive their dimensions,
 *  2) dimensions always have a set that has a host.
 */
void ml_new_host(RRDHOST *RH) {
    Tracer{"Database", "ml_new_host", "hostname", RH->hostname};

    if (simple_pattern_matches(Cfg.SP_HostsToSkip, RH->hostname))
        return;

    Host *H = new Host(RH);
    RH->ml_host = static_cast<ml_host_t>(H);

    // H->startAnomalyDetectionThreads();
}

void ml_delete_host(RRDHOST *RH) {
    ReportableTracer{"Database", "ml_delete_host", "hostname", RH->hostname};

    Host *H = static_cast<Host *>(RH->ml_host);
    if (!H)
        return;

    //H->stopAnomalyDetectionThreads();
    delete H;
    RH->ml_host = nullptr;
}

void ml_new_chart(RRDSET *RS) {
    Tracer{"Database", "ml_new_chart", "chart", RS->id};

    if (!RS)
        return;

    if (RS->update_every != 1)
        return;

    if (strstr(RS->name, "_km") != NULL)
        return;

    if (simple_pattern_matches(Cfg.SP_ChartsToSkip, RS->id))
        return;

    Chart *C = new Chart(RS);
    RS->state->ml_chart = static_cast<ml_chart_t>(C);

    Host *H = static_cast<Host *>(RS->rrdhost->ml_host);
    H->addChart(C);
}

void ml_delete_chart(RRDSET *RS) {
    Tracer{"Database", "ml_delete_chart", "chart", RS->id};

    if (!RS)
        return;

    Chart *C = static_cast<Chart *>(RS->state->ml_chart);
    if (!C)
        return;

    Host *H = static_cast<Host *>(RS->rrdhost->ml_host);
    H->removeChart(C);

    delete C;
    RS->state->ml_chart = nullptr;
}

void ml_new_unit(RRDDIM *RD) {
    Tracer{"Database", "ml_new_unit", "chart", RD->id};

    if (!RD)
        return;

    RRDSET *RS = RD->rrdset;

    if (RS->update_every != 1)
        return;

    if (strstr(RS->name, "_km") != NULL)
        return;

    if (simple_pattern_matches(Cfg.SP_ChartsToSkip, RS->name))
        return;

    Chart *C = static_cast<Chart *>(RS->state->ml_chart);
    if (!C)
        return;

    Unit *U = new Unit(RD);
    RD->state->ml_unit = static_cast<ml_unit_t>(U);
    C->addDimension(U);

    Host *H = static_cast<Host *>(RS->rrdhost->ml_host);
    H->NumDimensions++;
}

void ml_delete_unit(RRDDIM *RD) {
    Tracer{"Database", "ml_delete_unit", "chart", RD->id};

    Unit *U = static_cast<Unit *>(RD->state->ml_unit);
    if (!U)
        return;

    RRDSET *RS = RD->rrdset;
    Chart *C = static_cast<Chart *>(RS->state->ml_chart);
    C->removeDimension(U);

    Host *H = static_cast<Host *>(RS->rrdhost->ml_host);
    H->NumDimensions--;

    delete U;
    RD->state->ml_unit = nullptr;
}

bool ml_is_anomalous(RRDDIM *RD) {
    if (!RD)
        return false;

    Unit *U = static_cast<Unit *>(RD->state->ml_unit);
    if (!U)
        return false;

    return U->getAnomalyBit();
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
