// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_CHART_H
#define ML_CHART_H

#include "ml-private.h"

namespace ml {

class Unit;

class Chart {
public:
    Chart(RRDSET *RS) : RS(RS) {}

    std::string getFamily() const { return RS->family; }

    void updateUnits();

public:
    RRDSET *RS;

    std::map<RRDDIM *, Unit *> UnitsMap;
};

}

#endif /* ML_CHART_H */
