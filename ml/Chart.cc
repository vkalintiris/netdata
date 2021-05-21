// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"

#include "Chart.h"
#include "Unit.h"

using namespace ml;

void Chart::updateUnits() {
    Units.clear();

    rrdset_rdlock(RS);

    RRDDIM *RD;
    rrddim_foreach_read(RD, RS) {
        Unit *U = static_cast<Unit *>(RD->state->ml_unit);
        if (!U)
            continue;

        //  - push only if there's a live RD
        //  - free memory otherwise
        Units.push_back(U);
    }

    rrdset_unlock(RS);
}
