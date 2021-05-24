// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Unit.h"

using namespace ml;

void Host::addUnit(Unit *U) {
    std::unique_lock<std::mutex> Lock(Mutex);
    UnitsMap[U->RD] = U;
}

void Host::removeUnit(Unit *U) {
    std::unique_lock<std::mutex> Lock(Mutex);
    UnitsMap.erase(U->RD);
}

void Host::trainUnits() {
    std::this_thread::sleep_for(Seconds{10});

    while (!netdata_exit) {
        Duration<double> AvailableUnitTrainingDuration;

        TimePoint TrainingStartTP = SteadyClock::now();
        {
            std::unique_lock<std::mutex> Lock(Mutex);

            for (auto &UP : UnitsMap) {
                Unit *U = UP.second;

                if (U->train(TrainingStartTP))
                    break;
            }

            AvailableUnitTrainingDuration = Cfg.TrainEvery / (UnitsMap.size() + 1);
        }
        Duration<double> UnitTrainingDuration = SteadyClock::now() - TrainingStartTP;

        if (AvailableUnitTrainingDuration > UnitTrainingDuration)
            std::this_thread::sleep_for(AvailableUnitTrainingDuration - UnitTrainingDuration);
        else
            fatal("AvailableUnitTrainingDuration < UnitTrainingDuration");
    }
}

void Host::runMLThreads() {
    TrainingThread = std::thread(&Host::trainUnits, this);
}

void Host::stopMLThreads() {
    TrainingThread.join();
}
