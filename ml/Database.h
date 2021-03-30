// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_DATABASE_H
#define ML_DATABASE_H

#include "ml-private.h"

namespace ml {

class Host;

class Database {
public:
    void updateHosts();

public:
    std::map<RRDHOST *, Host *> Hosts;
};

extern Database DB;

}

#endif /* ML_DATABASE_H */
