// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"

#include "Unit.h"

namespace ml {

class Unit;

class Host {
public:
    Host(RRDHOST *RH) : RH(RH) { }

    void addUnit(Unit *U) {
        std::unique_lock<std::mutex> Lock(Mutex);
        auto Pos = std::find_if(Units.begin(), Units.end(),
                                [U](Unit *RHS) { return U < RHS; });
        Units.insert(Pos, U);
    }

    void removeUnit(Unit *U) {
        std::unique_lock<std::mutex> Lock(Mutex);
        auto Pos = std::find_if(Units.begin(), Units.end(),
                                [U](Unit *RHS) { return U < RHS; });
        Units.erase(Pos);
    }

    void trainUnits();

private:
    RRDHOST *RH;

    std::mutex Mutex;
    std::vector<Unit *> Units;
};

}

#endif /* ML_HOST_H */
