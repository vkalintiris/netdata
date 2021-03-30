// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"

namespace ml {

class Chart;

class Host {
public:
    Host(RRDHOST *RH) : RH(RH) {
        CreationTime = SteadyClock::now();
    }

    std::string uid() const { return RH->hostname; }
    const char *c_uid() const { return RH->hostname; }

    void updateCharts();

public:
    RRDHOST *RH;
    std::map<RRDSET *, Chart *> ChartsMap;

    TimePoint CreationTime;
};

}

#endif /* ML_HOST_H */
