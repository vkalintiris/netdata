// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"

namespace ml {

class Host {
public:
    Host(RRDHOST *RH) : RH(RH) {
        CreationTime = steady_clock::now();
        netdata_rwlock_init(&RWLock);
    }

    std::string uid() const {
        return RH->hostname;
    }

    const char *c_uid() const {
        return RH->hostname;
    }

    void updateCharts();

    void rdLock() { netdata_rwlock_rdlock(&RWLock); }
    void wrLock() { netdata_rwlock_wrlock(&RWLock); }
    void unLock() { netdata_rwlock_unlock(&RWLock); }

public:
    RRDHOST *RH;
    std::chrono::time_point<std::chrono::steady_clock> CreationTime;

    std::map<RRDSET *, Chart *> ChartsMap;
    netdata_rwlock_t RWLock;
};

};

#endif /* ML_HOST_H */
