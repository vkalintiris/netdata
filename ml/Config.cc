// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

using namespace ml;

void Config::updateHosts() {
    SPDR_BEGIN(Cfg.SPDR, "cat", "update-hosts");

    RRDHOST *RH;

    netdata_rwlock_wrlock(&Cfg.HostsLock);
    rrd_rdlock();

    SPDR_BEGIN(Cfg.SPDR, "cat", "rrd-locked");

    rrdhost_foreach_read(RH) {
        SPDR_BEGIN(Cfg.SPDR, "cat", RH->hostname);

        std::map<RRDHOST *, Host *>::iterator It = Hosts.find(RH);

        if (rrdhost_flag_check(RH, RRDHOST_FLAG_ARCHIVED)) {
            fatal("Found archived host %s", RH->hostname);
        } else {
            if (It == Hosts.end()) {
                /* We will update this host's charts in the next iteration to
                 * allow it to get stable first */
                info("Creating new host %s", RH->hostname);
                Hosts[RH] = new Host(RH);
            } else {
                Hosts[RH]->updateCharts();
            }
        }

        SPDR_END(Cfg.SPDR, "cat", RH->hostname);
    }

    SPDR_END(Cfg.SPDR, "cat", "rrd-locked");

    rrd_unlock();
    netdata_rwlock_unlock(&Cfg.HostsLock);

    SPDR_END(Cfg.SPDR, "cat", "update-hosts");
}
