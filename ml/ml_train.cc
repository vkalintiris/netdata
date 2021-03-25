// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

static void dumpSpdr(const char *string, void *user_data) {
    (void) user_data;

    ml::Cfg.LogFp << string << std::endl;
}


static void cleanupTrainThread(void *ptr) {
    struct netdata_static_thread *thr = (struct netdata_static_thread *) ptr;

    thr->enabled = NETDATA_MAIN_THREAD_EXITING;
    info("Cleaning up train thread");

    spdr_report(ml::Cfg.SPDR, SPDR_CHROME_REPORT, dumpSpdr, nullptr);
    ml::Cfg.LogFp.close();

    thr->enabled = NETDATA_MAIN_THREAD_EXITED;
}

namespace ml {

usec_t processUnit(Unit *U, heartbeat_t &HB) {
    struct timeval BTV, ETV;

    now_monotonic_high_precision_timeval(&BTV);
    U->train();
    now_monotonic_high_precision_timeval(&ETV);

    usec_t Duration = dt_usec(&BTV, &ETV);
    usec_t UsecsPerUnit = (Cfg.TrainEvery * USEC_PER_SEC) / Cfg.NumUnits;

    if (Duration < UsecsPerUnit) {
        SPDR_BEGIN1(Cfg.SPDR, "cat", "train-unit-sleep", SPDR_STR("unit", U->c_uid()));
        heartbeat_next(&HB, UsecsPerUnit - Duration);
        SPDR_END(Cfg.SPDR, "cat", "train-unit-sleep");
    }

    return std::max(Duration, UsecsPerUnit);
}

void trainMain(struct netdata_static_thread *Thread) {
    netdata_thread_cleanup_push(cleanupTrainThread, Thread);

    heartbeat_t HB;
    heartbeat_init(&HB);

    usec_t UpdateHostsEvery = 10 * USEC_PER_SEC;

    while (!netdata_exit) {
        // Update data structures.
        Cfg.updateHosts();

        // Track how much time we've spent in training.
        usec_t TimeSpentTraining = 0;

        // For each host
        for (auto &HP : Cfg.Hosts) {
            Host *H = HP.second;

            SPDR_BEGIN1(Cfg.SPDR, "cat", "train-host-loop", SPDR_STR("host", H->c_uid()));

            // For each chart
            for (auto &CP: H->ChartsMap) {
                Chart *C = CP.second;

                SPDR_BEGIN1(Cfg.SPDR, "cat", "train-chart-loop", SPDR_STR("chart", C->RS->id));

                // For each unit
                for (auto &UP : C->UnitsMap) {
                    Unit *U = UP.second;

                    if (!U->shouldTrain())
                        continue;

                    SPDR_BEGIN1(Cfg.SPDR, "cat", "train-unit-loop", SPDR_STR("unit", U->c_uid()));

                    if (TimeSpentTraining < UpdateHostsEvery && !netdata_exit)
                        TimeSpentTraining += processUnit(U, HB);

                    SPDR_END(Cfg.SPDR, "cat", "train-unit-loop");
                }

                SPDR_END(Cfg.SPDR, "cat", "train-chart-loop");
            }

            SPDR_END(Cfg.SPDR, "cat", "train-host-loop");
        }

        // Sleep if we have to.
        if (TimeSpentTraining < UpdateHostsEvery && !netdata_exit)
            heartbeat_next(&HB, UpdateHostsEvery - TimeSpentTraining);
    }

    netdata_thread_cleanup_pop(1);
}

};
