// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_WINDOW_H
#define ML_WINDOW_H

#include "ml-private.h"

namespace ml {

class Unit;

class Window {
public:
    Window(Unit *U, unsigned NumSamples) :
        U(U), NumSamples(NumSamples),
        NumCollected(0), NumEmpty(0), NumReset(0) {};

    CalculatedNumber *getCalculatedNumbers();

    double ratioFilled() const {
        return static_cast<double>(NumCollected) / NumSamples;
    }

public:
    Unit *U;

    unsigned NumSamples;
    unsigned NumCollected;
    unsigned NumEmpty;
    unsigned NumReset;
};

};

#endif /* ML_WINDOW_H */
