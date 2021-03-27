// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

using namespace ml;

void Config::updateHosts() {
    netdata_rwlock_wrlock(&Cfg.HostsLock);
    rrd_rdlock();

    SPDR_BEGIN(Cfg.SPDR, "cat", "rrd-locked");

    RRDHOST *RH;
    rrdhost_foreach_read(RH) {
        rrdhost_rdlock(RH);
        SPDR_BEGIN(Cfg.SPDR, "cat", RH->hostname);

        std::map<RRDHOST *, Host *>::iterator It = Hosts.find(RH);

        if (rrdhost_flag_check(RH, RRDHOST_FLAG_ARCHIVED)) {
            fatal("Found archived host %s", RH->hostname);
        } else {
            if (It == Hosts.end()) {
                info("Creating new host %s", RH->hostname);
                Hosts[RH] = new Host(RH);
            }
        }

        SPDR_END(Cfg.SPDR, "cat", RH->hostname);
        rrdhost_unlock(RH);
    }

    SPDR_END(Cfg.SPDR, "cat", "rrd-locked");

    rrd_unlock();
    netdata_rwlock_unlock(&Cfg.HostsLock);
}
