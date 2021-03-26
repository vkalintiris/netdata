// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

using namespace ml;

void Config::updateHosts() {
    RRDHOST *RH;

    netdata_rwlock_wrlock(&Cfg.HostsLock);
    rrd_rdlock();

    rrdhost_foreach_read(RH) {
        std::map<RRDHOST *, Host *>::iterator It = Hosts.find(RH);

        if (rrdhost_flag_check(RH, RRDHOST_FLAG_ARCHIVED)) {
            fatal("Found archived host %s", RH->hostname);
        } else {
            if (It == Hosts.end()) {
                info("Creating new host %s", RH->hostname);
                Hosts[RH] = new Host(RH);
            }

            Hosts[RH]->updateCharts();
        }
    }

    rrd_unlock();
    netdata_rwlock_unlock(&Cfg.HostsLock);
}
