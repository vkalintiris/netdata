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
    std::mutex Mutex;
};

extern Database DB;

}

#endif /* ML_DATABASE_H */
