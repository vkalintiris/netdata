// SPDX-License-Identifier: GPL-3.0-or-later

#include "plugin_profile.h"

#include <chrono>
#include <vector>
#include <random>
#include <sstream>
#include <string>
#include <unordered_map>

#define PLUGIN_PROFILE_NAME "profile.plugin"

#define CONFIG_SECTION_PROFILE "plugin:profile"

static void profile_main_cleanup(void *ptr)
{
    struct netdata_static_thread *static_thread = (struct netdata_static_thread *)ptr;
    static_thread->enabled = NETDATA_MAIN_THREAD_EXITING;

    info("cleaning up...");

    static_thread->enabled = NETDATA_MAIN_THREAD_EXITED;
}

static constexpr unsigned NumCharts = 2;
static constexpr unsigned NumDimsPerChart = 2;
static constexpr unsigned NumTotalDims = NumCharts * NumDimsPerChart;

static std::unordered_map<RRDSET *, std::vector<RRDDIM *>> ChartDimsMap;
static std::vector<collected_number> CNs;

static void initCNs() {
    std::uniform_int_distribution<> Dist(1, 10);

    std::random_device RD;
    std::mt19937 Gen(RD());

    CNs.reserve(NumTotalDims);
    for (unsigned Idx = 0; Idx != NumTotalDims; Idx++)
        CNs[Idx] = Dist(Gen);
}

static void createCharts(unsigned NumCharts, unsigned NumDimsPerChart) {
    std::vector<std::string> ChartNames, DimNames;
    std::stringstream SS;

    ChartNames.reserve(NumCharts);
    for (unsigned Idx = 0; Idx != NumCharts; Idx++) {
        SS.clear();
        SS.str(std::string());
        SS << "profchart_" << Idx;

        ChartNames.push_back(SS.str());
    }

    DimNames.reserve(NumDimsPerChart);
    for (unsigned Idx = 0; Idx != NumDimsPerChart; Idx++) {
        SS.clear();
        SS.str(std::string());
        SS << "profdim_" << Idx;

        DimNames.push_back(SS.str());
    }

    for (unsigned ChartIdx = 0; ChartIdx != NumCharts; ChartIdx++) {
        RRDSET *RS = rrdset_create(
            localhost,
            "prof_type",
            ChartNames[ChartIdx].c_str(), // id
            NULL, // name
            "prof_family",
            NULL, // ctx
            ChartNames[ChartIdx].c_str() , // title
            "prof_units",
            "prof_plugin",
            "prof_module",
            41000 + ChartIdx,
            localhost->rrd_update_every,
            RRDSET_TYPE_LINE
        );

        // rrdset_flag_set(RS, RRDSET_FLAG_HIDDEN);

        if (!RS)
            fatal("Could not create chart %s", ChartNames[ChartIdx].c_str());

        for (unsigned DimIdx = 0; DimIdx != NumDimsPerChart; DimIdx++) {
            RRDDIM *RD = rrddim_add(
                RS, DimNames[DimIdx].c_str(), NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE
            );

            if (RD == nullptr) {
                fatal("Could not create dimension %s.%s",
                      ChartNames[ChartIdx].c_str(),
                      DimNames[DimIdx].c_str());
            }

            ChartDimsMap[RS].push_back(RD);
        }
    }
}

static void updateCharts() {
    static unsigned Counter = 0;

    unsigned ChartIdx = 0;

    for (const auto &P : ChartDimsMap) {
        RRDSET *RS = P.first;
        const std::vector<RRDDIM *> &Dims = P.second;

        rrdset_next(RS);

        for (unsigned DimIdx = 0; DimIdx != NumDimsPerChart; DimIdx++) {
            collected_number CN = CNs[(Counter + ChartIdx++) % NumTotalDims];
            rrddim_set_by_pointer(RS, Dims[DimIdx], CN);
        }

        rrdset_done(RS);
    }


    Counter++;
}

void *profile_main(void *ptr)
{
    netdata_thread_cleanup_push(profile_main_cleanup, ptr);

    initCNs();

    createCharts(NumCharts, NumDimsPerChart);

    heartbeat_t HB;
    heartbeat_init(&HB);

    while (!netdata_exit) {
        heartbeat_next(&HB, USEC_PER_SEC);
        updateCharts();
    }

    netdata_thread_cleanup_pop(1);
    return NULL;
}
