// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

using namespace ml;

/*
 * Update the charts referenced by the host.
 */
void Host::updateCharts() {
    RRDSET *RS;

    wrLock();
    //rrdhost_rdlock(RH);
    if (netdata_rwlock_tryrdlock(&((RH)->rrdhost_rwlock)) != 0) {
        unLock();
        return;
    }

    SPDR_BEGIN(Cfg.SPDR, "cat", "host-locked");

    rrdset_foreach_read(RS, RH) {
        std::map<RRDSET *, Chart *>::iterator It = ChartsMap.find(RS);

        bool IsObsolete = rrdset_flag_check(RS, RRDSET_FLAG_ARCHIVED) ||
            rrdset_flag_check(RS, RRDSET_FLAG_OBSOLETE);

        if (IsObsolete) {
            if (It != ChartsMap.end()) {
                error("Found obsolete chart %s.%s", RS->rrdhost->hostname, RS->id);
                ChartsMap.erase(RS);
            }
        } else {
            if (It == ChartsMap.end()) {
                if (RS->update_every != 1)
                    continue;

                if (Cfg.MLSets.count(RS))
                    continue;

                if (simple_pattern_matches(Cfg.SP_ChartsToSkip, RS->name))
                    continue;

                ChartsMap[RS] = new Chart(RS);
            }

            ChartsMap[RS]->updateUnits(Cfg.TrainSecs, Cfg.TrainEvery,
                                       Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);
        }
    }

    SPDR_END(Cfg.SPDR, "cat", "host-locked");

    rrdhost_unlock(RH);
    unLock();
}
