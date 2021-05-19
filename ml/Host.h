// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"

#include "Unit.h"

namespace ml {

class Host {
public:
    Host(RRDHOST *RH) : RH(RH), CreationTime(SteadyClock::now()) { }

    std::string getHostname() const { return RH->hostname; }

    void incrNumUnits() { NumUnits++; }
    void decrNumUnits() { NumUnits--; }

private:
    RRDHOST *RH;
    TimePoint CreationTime;

    std::atomic<int> NumUnits;
};

}

#endif /* ML_HOST_H */
