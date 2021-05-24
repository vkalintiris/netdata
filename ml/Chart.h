// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_CHART_H
#define ML_CHART_H

#include "ml-private.h"
#include "Unit.h"

namespace ml {

class Chart {
public:
    Chart(RRDSET *RS) : RS(RS) { HasRD = true; }

    void unrefSet() { HasRD = false; }

    std::vector<Unit *> getUnits() {
        updateUnits();
        return Units;
    }

private:
    void updateUnits();

public:
    std::atomic<bool> HasRD;


private:
    RRDSET *RS;

    std::mutex Mutex;
    std::vector<Unit *> Units;
};

}

#endif /* ML_CHART_H */
