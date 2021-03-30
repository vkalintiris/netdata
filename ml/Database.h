// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_DATABASE_H
#define ML_DATABASE_H

#include "ml-private.h"

namespace ml {

class Unit;
class Host;

class Database {
public:
    void trainUnits();
    void predictUnits();

    void updateMLCharts();

private:
    std::vector<Unit *> getUnits();

    void updateHosts();
    void updateCharts();
    void updateUnits();

private:
    std::map<RRDHOST *, Host *> HostsMap;
    std::mutex Mutex;
};

extern Database DB;

}

#endif /* ML_DATABASE_H */
