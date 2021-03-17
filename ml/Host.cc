// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Host.h"
#include "Chart.h"

using namespace ml;

/*
 * Update the charts referenced by the host.
 */
void Host::updateCharts() {
    rrdhost_rdlock(RH);

    RRDSET *RS;
    rrdset_foreach_read(RS, RH) {
        rrdset_rdlock(RS);

        std::map<RRDSET *, Chart *>::iterator It = ChartsMap.find(RS);

        bool IsObsolete = rrdset_flag_check(RS, RRDSET_FLAG_ARCHIVED) ||
            rrdset_flag_check(RS, RRDSET_FLAG_OBSOLETE);

        if (IsObsolete) {
            if (It != ChartsMap.end()) {
                // TODO: Remove obsolete charts.
                error("Found obsolete chart %s.%s", RS->rrdhost->hostname, RS->id);
                ChartsMap.erase(RS);
            }
        } else {
            if (It == ChartsMap.end()) {
                bool shouldSkip = false;

                // Skip if update every != 1 sec
                shouldSkip |= RS->update_every != 1;

                // Skip if this is a KMeans chart
                shouldSkip |= strstr(RS->id, "_km") != NULL;

                // Skip if our users want
                shouldSkip |= simple_pattern_matches(Cfg.SP_ChartsToSkip, RS->name) != 0;

                if (!shouldSkip)
                    ChartsMap[RS] = new Chart(RS);
            }
        }

        rrdset_unlock(RS);
    }

    rrdhost_unlock(RH);
}
