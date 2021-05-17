// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"

namespace ml {

class Host {
public:
    Host(RRDHOST *RH) : RH(RH), CreationTime(SteadyClock::now()) { }

    std::string getHostname() const { return RH->hostname; }

private:
    RRDHOST *RH;
    TimePoint CreationTime;
};

}

#endif /* ML_HOST_H */
