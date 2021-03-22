// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

using namespace ml;

/*
 * Update the charts referenced by the host.
 */
void Host::updateCharts() {
    RRDSET *RS;

    wrLock();
    rrdhost_rdlock(RH);

    NumUnits = 0;

    rrdset_foreach_read(RS, RH) {
        if (RS->update_every != 1)
            continue;

        if (Cfg.MLSets.count(RS))
            continue;

        if (simple_pattern_matches(Cfg.SP_ChartsToSkip, RS->name))
            continue;

        bool IsObsolete = rrdset_flag_check(RS, RRDSET_FLAG_ARCHIVED) ||
            rrdset_flag_check(RS, RRDSET_FLAG_OBSOLETE);

        if (IsObsolete) {
            ChartsMap.erase(RS);
            continue;
        }

        std::map<RRDSET *, Chart *>::iterator It = ChartsMap.find(RS);
        if (It == ChartsMap.end())
            ChartsMap[RS] = new Chart(RS);

        ChartsMap[RS]->updateUnits(Cfg.TrainSecs, Cfg.TrainEvery,
                                   Cfg.DiffN, Cfg.SmoothN, Cfg.LagN);

        NumUnits += ChartsMap[RS]->numUnits();
    }

    rrdhost_unlock(RH);
    unLock();
}
