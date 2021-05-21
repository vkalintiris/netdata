// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_CHART_H
#define ML_CHART_H

#include "ml-private.h"
#include "Unit.h"

namespace ml {

class Chart {
public:
    Chart(RRDSET *RS) : RS(RS) { }

    void updateUnits();

private:
    RRDSET *RS;

    std::vector<Unit *> Units;
};

}

#endif /* ML_CHART_H */
