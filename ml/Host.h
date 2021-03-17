// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"

namespace ml {

class Host {
public:
    Host(RRDHOST *RH, std::map<RRDSET *, Chart *> &ChartsMap)
        : RH(RH), ChartsMap(ChartsMap) {};

    void updateCharts();

public:
    RRDHOST *RH;
    std::map<RRDSET *, Chart *> &ChartsMap;
};

};

#endif /* ML_HOST_H */
