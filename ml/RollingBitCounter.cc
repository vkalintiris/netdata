// SPDX-License-Identifier: GPL-3.0-or-later

#include "RollingBitCounter.h"

using namespace ml;

void RollingBitCounter::print(std::ostream &OS) const {
    std::vector<bool> Buffer = getBuffer();

    std::cout << "V: ";

    for (bool B : Buffer)
        OS << B;

    OS << " (set bits: " << NumSetBits << ")";
}

std::vector<bool> RollingBitCounter::getBuffer() const {
    std::vector<bool> Buffer;

    for (size_t Idx = start(); Idx != (start() + size()); Idx++)
        Buffer.push_back(V[Idx % V.size()]);

    return Buffer;
}

void RollingBitCounter::insert(bool Bit) {
    if (N >= V.size())
        NumSetBits -= (V[start()] == true);

    NumSetBits += (Bit == true);
    V[N++ % V.size()] = Bit;
}

std::pair<RollingBitWindow::Edge, size_t> RollingBitWindow::insert(bool Bit) {
    Edge E;

    PrevLength = CurrLength;

    RBC.insert(Bit);
    switch (CurrState) {
        case State::NotFilled: {
            if (RBC.isFilled()) {
                if (RBC.numSetBits() < SetBitsThreshold) {
                    CurrState = State::BelowThreshold;
                } else {
                    CurrState = State::AboveThreshold;
                }
            } else {
                CurrState = State::NotFilled;
            }

            E = {State::NotFilled, CurrState};
            break;
        } case State::BelowThreshold: {
            if (RBC.numSetBits() >= SetBitsThreshold) {
                CurrState = State::AboveThreshold;
            }

            E = {State::BelowThreshold, CurrState};
            break;
        } case State::AboveThreshold: {
            if (RBC.numSetBits() < SetBitsThreshold) {
                CurrState = State::BelowThreshold;
            }

            E = {State::AboveThreshold, CurrState};
            break;
        }
    }

    if (EdgeActions.count(E)) {
        Action A =  EdgeActions[E];
        (this->*A)(E.first, Bit);
    }

    return {E, PrevLength};
}
