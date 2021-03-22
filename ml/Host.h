// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_HOST_H
#define ML_HOST_H

#include "ml-private.h"

namespace ml {

class Host {
public:
    Host(RRDHOST *RH) : RH(RH), NumUnits(0) {
        netdata_rwlock_init(&RWLock);
    };

    void updateCharts();

    void rdLock() { netdata_rwlock_rdlock(&RWLock); }
    void wrLock() { netdata_rwlock_wrlock(&RWLock); }
    void unLock() { netdata_rwlock_unlock(&RWLock); }

    size_t numUnits() const { return NumUnits; }

public:
    RRDHOST *RH;
    size_t NumUnits;

    std::map<RRDSET *, Chart *> ChartsMap;
    netdata_rwlock_t RWLock;
};

};

#endif /* ML_HOST_H */
