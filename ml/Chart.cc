// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"

#include "Chart.h"
#include "Unit.h"

using namespace ml;

void Chart::updateUnits() {
    // Clear the vector we use for tracking units.
    Units.clear();

    rrdset_rdlock(RS);

    RRDDIM *RD;
    rrddim_foreach_read(RD, RS) {
        Unit *U = static_cast<Unit *>(RD->state->ml_unit);

        // This dimension does not have a unit.
        if (!U)
            continue;

        // This unit's RRD ref has been deleted.
        if (!U->HasRD)
            delete U;

        // We can use this unit.
        Units.push_back(U);
    }

    rrdset_unlock(RS);
}
