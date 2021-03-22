// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

using namespace ml;

void Config::updateHosts() {
    RRDHOST *RH;

    netdata_rwlock_wrlock(&Cfg.HostsLock);
    rrd_rdlock();

    NumUnits = 0;
    rrdhost_foreach_read(RH) {
        if (rrdhost_flag_check(RH, RRDHOST_FLAG_ARCHIVED))
            continue;

        std::map<RRDHOST *, Host *>::iterator It = Cfg.Hosts.find(RH);
        if (It == Cfg.Hosts.end())
            Cfg.Hosts[RH] = new Host(RH);

        Cfg.Hosts[RH]->updateCharts();
        NumUnits += Cfg.Hosts[RH]->numUnits();
    }

    rrd_unlock();
    netdata_rwlock_unlock(&Cfg.HostsLock);
}
