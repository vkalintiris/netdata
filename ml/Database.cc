// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Database.h"
#include "Host.h"
#include "Unit.h"

using namespace ml;

Host *Database::addHost(RRDHOST *RH) {
    std::unique_lock<std::mutex> Lock(Mutex);

    std::map<RRDHOST *, Host *>::iterator It = HostsMap.find(RH);
    if (It != HostsMap.end())
        fatal("ML host '%s' has already been created.", RH->hostname);

    Host *H = new Host(RH);
    HostsMap[RH] = H;
    return H;
}
