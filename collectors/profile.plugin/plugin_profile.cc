// SPDX-License-Identifier: GPL-3.0-or-later

#include "plugin_profile.h"

#include <chrono>
#include <vector>
#include <random>
#include <sstream>
#include <string>
#include <map>

#include "streaming/replication/GapData.h"
using namespace replication;

#define PLUGIN_PROFILE_NAME "profile.plugin"

#define CONFIG_SECTION_PROFILE "plugin:profile"

static void profile_main_cleanup(void *ptr)
{
    struct netdata_static_thread *static_thread = (struct netdata_static_thread *)ptr;
    static_thread->enabled = NETDATA_MAIN_THREAD_EXITING;

    info("cleaning up...");

    static_thread->enabled = NETDATA_MAIN_THREAD_EXITED;
}

static constexpr unsigned NumCharts = 500;
static constexpr unsigned NumDimsPerChart = 10;
static constexpr unsigned NumTotalDims = NumCharts * NumDimsPerChart;
static constexpr unsigned UpdateEvery = 1;

static std::map<RRDSET *, std::vector<RRDDIM *>> ChartDimsMap;
static std::vector<collected_number> CNs;

static void initCNs() {
    double RFreq = 1.0 / 60;
    for (unsigned Idx = 0; Idx != 3600; Idx++) {
        CNs.push_back(500 * sin(2 * M_PI * RFreq * Idx) + 10);
    }
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
            UpdateEvery,
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
    static unsigned NumUpdates = 0;

    RRDSET *RS = nullptr;
    for (const auto &P : ChartDimsMap) {
        RS = P.first;
        const std::vector<RRDDIM *> &Dims = P.second;

        rrdset_next(RS);

        for (unsigned DimIdx = 0; DimIdx != NumDimsPerChart; DimIdx++) {
            collected_number CN = CNs[NumUpdates % CNs.size()];
            rrddim_set_by_pointer(RS, Dims[DimIdx], CN);
        }

        rrdset_done(RS);
    }

    NumUpdates++;
}

void *profile_main(void *ptr)
{
    netdata_thread_cleanup_push(profile_main_cleanup, ptr);

    initCNs();

    createCharts(NumCharts, NumDimsPerChart);

    heartbeat_t HB;
    heartbeat_init(&HB);

    while (!netdata_exit) {
        heartbeat_next(&HB, UpdateEvery * USEC_PER_SEC);
        updateCharts();
    }

    netdata_thread_cleanup_pop(1);
    return NULL;
}
