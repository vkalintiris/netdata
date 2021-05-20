// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"

#include "Unit.h"

namespace ml {

class Host {
public:
    Host(RRDHOST *RH) : RH(RH) { }

    void newUnit(RRDDIM *RD) {
        std::unique_lock<std::mutex> Lock(Mutex);

        UnitsMap[RD] = new Unit(RD);
    }

    void deleteUnit(RRDDIM *RD) {
        std::unique_lock<std::mutex> Lock(Mutex);

        Unit *U = UnitsMap[RD];
        delete U;

        UnitsMap.erase(RD);
    }

    bool isUnitAnomalous(RRDDIM *RD) {
        std::unique_lock<std::mutex> Lock(Mutex);
        Unit *U = UnitsMap[RD];
        return U->isAnomalous();
    }

private:
    RRDHOST *RH;

    std::mutex Mutex;
    std::map<RRDDIM *, Unit *> UnitsMap;
};

}

#endif /* ML_HOST_H */
