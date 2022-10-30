// SPDX-License-Identifier: GPL-3.0-or-later

#include "plugin_profile.h"

#include <chrono>
#include <memory>
#include <vector>
#include <random>
#include <sstream>
#include <string>
#include <map>

static FILE *logfp = NULL;

class Dimension {
public:
    Dimension(RRDDIM *RD, size_t Index) : RD(RD), Index(Index), NumUpdates(0)
    {
    }

    void update(const struct timeval &TV, size_t Iteration)
    {
        UNUSED(Iteration);

        collected_number CN = Index + NumUpdates++; // % 5 ? Index + 1 : Index;
        rrddim_timed_set_by_pointer(RD->rrdset, RD, TV, CN);
#if 0
        if (strcmp(rrdhost_hostname(RD->rrdset->rrdhost), "nd0") == 0)
            error("[GVD] Added new value %lld", CN);
#endif
    }

private:
    RRDDIM *RD;
    size_t Index;
    size_t NumUpdates;
};

class Chart {
    static size_t ChartIdx;

public:
    static std::shared_ptr<Chart> create(const char *Id, size_t UpdateEvery)
    {
        RRDSET *RS = rrdset_create(
            localhost,
            "prof_type",
            Id,   // id
            NULL, // name
            "prof_family",
            NULL, // ctx
            Id,   // title
            "prof_units",
            "prof_plugin",
            "prof_module",
            41000 + ChartIdx++,
            static_cast<int>(UpdateEvery),
            RRDSET_TYPE_LINE);

        if (!RS)
            fatal("Could not create chart %s", Id);

#if 0
        rrdset_flag_set(RS, RRDSET_FLAG_STORE_FIRST);
#endif
        return std::make_shared<Chart>(RS);
    }

public:
    Chart(RRDSET *RS) : RS(RS), Initialized(false)
    {
    }

    void createDimension(const char *Name, size_t Index)
    {
        RRDDIM *RD = rrddim_add(RS, Name, NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);

        if (!RD)
            fatal("Could not create dimension %s.%s", rrdset_id(RS), Name);

        Dimensions.push_back(std::make_shared<Dimension>(RD, Index));
    }

    void update(const struct timeval &TV, size_t Iteration)
    {
        if (Initialized) {
            usec_t DurationSinceLastUpdate = RS->update_every * USEC_PER_SEC;
            rrdset_timed_next(RS, TV, DurationSinceLastUpdate);
        }

        Initialized = true;

        for (std::shared_ptr<Dimension> &D : Dimensions)
            D->update(TV, Iteration);

        rrdset_timed_done(RS, TV);
    }

private:
    RRDSET *RS;
    bool Initialized;
    std::vector<std::shared_ptr<Dimension> > Dimensions;
};

size_t Chart::ChartIdx = 0;

static std::vector<std::shared_ptr<Chart> > createCharts(size_t NumCharts, unsigned NumDimsPerChart, time_t UpdateEvery)
{
    std::vector<std::shared_ptr<Chart> > Charts;
    Charts.reserve(NumCharts);

    constexpr size_t Len = 1024;
    char Buf[Len];

    for (size_t ChartIdx = 0; ChartIdx != NumCharts; ChartIdx++) {
        snprintfz(Buf, Len, "chart_%zu", ChartIdx + 1);
        std::shared_ptr<Chart> C = Chart::create(Buf, UpdateEvery);

        for (size_t DimIdx = 0; DimIdx != NumDimsPerChart; DimIdx++) {
            snprintfz(Buf, Len, "dim_%zu", DimIdx + 1);
            C->createDimension(Buf, DimIdx + 1);
        }

        Charts.push_back(std::move(C));
    }

    return Charts;
}

static void updateCharts(std::vector<std::shared_ptr<Chart> > &Charts, const struct timeval &TV, size_t Iteration)
{
    for (std::shared_ptr<Chart> &C : Charts)
        C->update(TV, Iteration);
}

static void profile_main_cleanup(void *ptr)
{
    struct netdata_static_thread *static_thread = (struct netdata_static_thread *)ptr;
    static_thread->enabled = NETDATA_MAIN_THREAD_EXITING;

    info("cleaning up...");
    if (logfp) {
        fflush(logfp);
        fclose(logfp);
    }

    static_thread->enabled = NETDATA_MAIN_THREAD_EXITED;
}

void *profile_main(void *ptr)
{
    netdata_thread_cleanup_push(profile_main_cleanup, ptr);

    size_t NumCharts = config_get_number(CONFIG_SECTION_GLOBAL, "profplug charts", 25000);
    size_t NumDimsPerChart = config_get_number(CONFIG_SECTION_GLOBAL, "profplug dimensions", 5);
    time_t UpdateEvery = 1;

    error("sizeof rrddim: %zu, sizeof rrddim_tier: %zu", sizeof(RRDDIM), sizeof(struct rrddim_tier));
    exit(EXIT_SUCCESS);

    std::vector<std::shared_ptr<Chart> > Charts = createCharts(NumCharts, NumDimsPerChart, UpdateEvery);

    heartbeat_t HB;
    heartbeat_init(&HB);

    size_t Iteration = 0;
    size_t SecondsToFill = 2;

    struct timeval NowTV;
    now_realtime_timeval(&NowTV);
    NowTV.tv_usec = 0;

    struct timeval CurrTV = NowTV;
    CurrTV.tv_sec -= SecondsToFill;

    auto StartTP = std::chrono::system_clock::now();

    while (!netdata_exit) {
        if (CurrTV.tv_sec >= NowTV.tv_sec) {
            heartbeat_next(&HB, UpdateEvery * USEC_PER_SEC);
        }

        CurrTV.tv_sec++;
        updateCharts(Charts, CurrTV, ++Iteration);
    }

    auto EndTP = std::chrono::system_clock::now();

    double TotalPoints = NumCharts * NumDimsPerChart * SecondsToFill;
    error(
        "%lf points added in %ld ms (%zu charts with %zu dims each)",
        TotalPoints,
        std::chrono::duration_cast<std::chrono::milliseconds>(EndTP - StartTP).count(),
        NumCharts,
        NumDimsPerChart);

    // points per msec
    error(
        "%lf points per msec",
        TotalPoints / std::chrono::duration_cast<std::chrono::milliseconds>(EndTP - StartTP).count());

    exit(EXIT_SUCCESS);

    netdata_thread_cleanup_pop(1);
    return NULL;
}
