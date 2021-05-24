// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Unit.h"

using namespace ml;

void Host::addUnit(Unit *U) {
    std::unique_lock<std::mutex> Lock(Mutex);
    Units.push_back(U);
}

void Host::predictUnits() {
    std::this_thread::sleep_for(Millis{5000});

    return;
}

void Host::trainUnits() {
    std::this_thread::sleep_for(Millis{5000});

    while (!netdata_exit)  {
        usec_t StartUSec = now_monotonic_high_precision_usec();
        usec_t EndUSec = now_monotonic_high_precision_usec();

        usec_t Delta = EndUSec - StartUSec;
        error("Collected %zu units in %llu usec",
              Units.size(), Delta);
        std::this_thread::sleep_for(Seconds{1});
    }
}

void Host::runMLThreads() {
    PredictionThread = std::thread(&Host::predictUnits, this);
    TrainingThread = std::thread(&Host::trainUnits, this);
}

void Host::stopMLThreads() {
    PredictionThread.join();
    TrainingThread.join();
}
