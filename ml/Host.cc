// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Unit.h"

using namespace ml;

void Host::updateCharts() {
    Charts.clear();

    rrdhost_rdlock(RH);

    RRDSET *RS;
    rrdset_foreach_read(RS, RH) {
        Charts.push_back(new Chart(RS));
    }

    rrdhost_unlock(RH);
}

void Host::predictUnits() {
    std::this_thread::sleep_for(Millis{5000});

    return;
}

void Host::trainUnits() {
    std::this_thread::sleep_for(Millis{5000});

    while (!netdata_exit)  {
        usec_t StartUSec = now_monotonic_high_precision_usec();
        updateCharts();
        usec_t EndUSec = now_monotonic_high_precision_usec();

        usec_t Delta = EndUSec - StartUSec;
        error("Collected %zu units in %llu usec",
              Charts.size(), Delta);
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
