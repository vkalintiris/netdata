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
    Dimension(RRDDIM *RD, size_t Index) :
        RD(RD), Index(Index), NumUpdates(0) {}

    void update(size_t Iteration) {
        UNUSED(Iteration);
        collected_number CN = ++NumUpdates; // % 5 ? Index + 1 : Index;
        rrddim_set_by_pointer(RD->rrdset, RD, CN);
        if (strcmp(rrdhost_hostname(RD->rrdset->rrdhost), "nd0") == 0)
            error("[GVD] Added new value %lld", CN);
    }

private:
    RRDDIM *RD;
    size_t Index;
    size_t NumUpdates;
};

class Chart {
    static size_t ChartIdx;

public:
    static std::shared_ptr<Chart> create(const char *Id, size_t UpdateEvery) {
        RRDSET *RS = rrdset_create(
            localhost,
            "prof_type",
            Id, // id
            NULL, // name
            "prof_family",
            NULL, // ctx
            Id, // title
            "prof_units",
            "prof_plugin",
            "prof_module",
            41000 + ChartIdx++,
            static_cast<int>(UpdateEvery),
            RRDSET_TYPE_LINE
        );

        if (!RS)
            fatal("Could not create chart %s", Id);

#if 0
        rrdset_flag_set(RS, RRDSET_FLAG_STORE_FIRST);
#endif
        return std::make_shared<Chart>(RS);
    }

public:
    Chart(RRDSET *RS) : RS(RS), Initialized(false) {}

    void createDimension(const char *Name, size_t Index) {
        RRDDIM *RD = rrddim_add(
            RS, Name, NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE
        );

        if (!RD)
            fatal("Could not create dimension %s.%s", rrdset_id(RS), Name);

        Dimensions.push_back(std::make_shared<Dimension>(RD, Index));
    }

    void update(size_t Iteration) {
        UNUSED(Iteration);

        if (Initialized) {
            rrdset_next(RS);
        }

        Initialized = true;

        for (std::shared_ptr<Dimension> &D : Dimensions)
            D->update(Iteration);

        rrdset_done(RS);
    }

private:
    RRDSET *RS;
    bool Initialized;
    std::vector<std::shared_ptr<Dimension>> Dimensions;
};

size_t Chart::ChartIdx = 0;

static std::vector<std::shared_ptr<Chart>> createCharts(size_t NumCharts, unsigned NumDimsPerChart, time_t UpdateEvery) {
    std::vector<std::shared_ptr<Chart>> Charts;
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

static void updateCharts(std::vector<std::shared_ptr<Chart>> &Charts, size_t Iteration) {
    for (std::shared_ptr<Chart> &C : Charts)
        C->update(Iteration);
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

#if 1
void *profile_main(void *ptr)
{
    netdata_thread_cleanup_push(profile_main_cleanup, ptr);

    size_t NumCharts = config_get_number(CONFIG_SECTION_GLOBAL, "profplug charts", 1);
    size_t NumDimsPerChart = config_get_number(CONFIG_SECTION_GLOBAL, "profplug dimensions", 1);
    time_t UpdateEvery = 5;

    std::vector<std::shared_ptr<Chart>> Charts = createCharts(NumCharts, NumDimsPerChart, UpdateEvery);

    heartbeat_t HB;
    heartbeat_init(&HB);

    size_t Iteration = 0;

    while (!netdata_exit) {
        heartbeat_next(&HB, UpdateEvery * USEC_PER_SEC);
        updateCharts(Charts, ++Iteration);
    }

    netdata_thread_cleanup_pop(1);
    return NULL;
}
#else
void *profile_main(void *ptr)
{
    netdata_thread_cleanup_push(profile_main_cleanup, ptr);

    logfp = fopen("/tmp/profplug.txt", "w");
    if (!logfp)
        fatal("Could not open log file");
    setvbuf(logfp, NULL, _IONBF, 0);

    heartbeat_t HB;
    heartbeat_init(&HB);

    time_t UpdateEvery = 1;
    size_t Iteration = 0;

    RRDSET *RS = nullptr;
    RRDDIM *RD = nullptr;

    // 2:00:00 PM
    struct timeval Now;
    now_realtime_timeval(&Now);
    Now.tv_sec -= 360000;
    Now.tv_usec = 0;

    // heartbeat_next(&HB, USEC_PER_SEC);

    while (!netdata_exit) {
        Now.tv_sec++;
        error("heartbeat: %ld", Now.tv_sec);

        if (++Iteration == (360000 - 1))
            break;

        collected_number CN = Iteration;

        if (!RS) {
            RS = rrdset_create(
                    localhost,
                    "prof_type",
                    "prof_type.prof_chart", // id
                    NULL, // name
                    "prof_family",
                    NULL, // ctx
                    "my chart", // title
                    "prof_units",
                    "prof_plugin",
                    "prof_module",
                    41000,
                    static_cast<int>(UpdateEvery),
                    RRDSET_TYPE_LINE
            );

            RD = rrddim_add(RS, "my_dim", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
        } else {
            usec_t DurationSinceLastUpdate = RS->update_every * USEC_PER_SEC;
            rrdset_timed_next(RS, Now, DurationSinceLastUpdate);
        }

        rrddim_timed_set_by_pointer(RD->rrdset, RD, Now, CN);

        rrdset_timed_done(RS, Now);
    }

    netdata_thread_cleanup_pop(1);
    return NULL;
}
#endif
