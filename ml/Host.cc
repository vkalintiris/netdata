// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Unit.h"

using namespace ml;

void Host::predictUnits() {
    return;
}

void Host::trainUnits() {
    return;
}

void Host::runMLThreads() {
    PredictionThread = std::thread(&Host::predictUnits, this);
    TrainingThread = std::thread(&Host::trainUnits, this);
}

void Host::stopMLThreads() {
    PredictionThread.join();
    TrainingThread.join();
}
