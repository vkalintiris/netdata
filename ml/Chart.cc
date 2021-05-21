// SPDX-License-Identifier: GPL-3.0-or-later

#include "Config.h"

#include "Chart.h"
#include "Unit.h"

using namespace ml;

void Chart::updateUnits() {
    for (Unit *U : Units) {
        if (!U->HasRD)
            delete U;
    }

    Units.clear();

    RRDDIM *RD;

    rrdset_rdlock(RS);
    rrddim_foreach_read(RD, RS) {
        Unit *U = static_cast<Unit *>(RD->state->ml_unit);
        Units.push_back(U);
    }
    rrdset_unlock(RS);
}
