// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_DATABASE_H
#define ML_DATABASE_H

#include "ml-private.h"

namespace ml {

class Host;

class Database {
public:
    Host *addHost(RRDHOST *RH);

private:
    std::map<RRDHOST *, Host *> HostsMap;
    std::mutex Mutex;
};

extern Database DB;

}

#endif /* ML_DATABASE_H */
