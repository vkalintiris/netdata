// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"
#include "Unit.h"
#include "Chart.h"

using namespace ml;

void Chart::updateUnits() {
    rrdset_rdlock(RS);

    RRDDIM *RD;
    rrddim_foreach_read(RD, RS) {
        std::map<RRDDIM *, Unit *>::iterator It = UnitsMap.find(RD);

        bool IsObsolete = rrddim_flag_check(RD, RRDDIM_FLAG_ARCHIVED) ||
                          rrddim_flag_check(RD, RRDDIM_FLAG_OBSOLETE);
        if (IsObsolete) {
            if (It != UnitsMap.end()) {
                error("Found obsolete dim %s.%s.%s", RS->rrdhost->hostname, RS->id, RD->id);
                UnitsMap.erase(RD);
            }
        } else {
            if (It == UnitsMap.end())
                UnitsMap[RD] = new Unit(RD);
        }
    }

    rrdset_unlock(RS);
}
