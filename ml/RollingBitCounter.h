// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ROLLING_BIT_COUNTER_H
#define ROLLING_BIT_COUNTER_H

#include "ml-private.h"

namespace ml {

class RollingBitCounter {
public:
    RollingBitCounter(size_t Capacity) : V(Capacity, 0), NumSetBits(0), N(0) {}

    bool isFilled() const {
       return N >= V.size();
    }

    void insert(bool Bit);

    size_t numSetBits() const {
        return NumSetBits;
    }

    std::vector<bool> getBuffer() const;

    void print(std::ostream &OS) const;
    
private:
    inline size_t size() const {
        return N < V.size() ? N : V.size();
    }

    inline size_t start() const {
        if (N <= V.size())
            return 0;

        return N % V.size();
    }

private:
    std::vector<bool> V;
    size_t NumSetBits;

    size_t N;
};

}

inline std::ostream& operator<<(std::ostream &OS, const ml::RollingBitCounter &RBC) {
    RBC.print(OS);
    return OS;
}

namespace ml {

class RollingBitWindow {
public:
    enum class State {
        NotFilled,
        BelowThreshold,
        AboveThreshold,
    };

    using Edge = std::pair<State, State>;
    using Action = void (RollingBitWindow::*)(State PrevState, bool NewBit);

private:
    std::map<Edge, Action> EdgeActions = {
        // From == To
        {
            Edge(State::NotFilled, State::NotFilled),
            &RollingBitWindow::onRoundtripNotFilled,
        },
        {
            Edge(State::BelowThreshold, State::BelowThreshold),
            &RollingBitWindow::onRoundtripBelowThreshold,
        },
        {
            Edge(State::AboveThreshold, State::AboveThreshold),
            &RollingBitWindow::onRoundtripAboveThreshold,
        },

        // NotFilled => {BelowThreshold, AboveThreshold}
        {
            Edge(State::NotFilled, State::BelowThreshold),
            &RollingBitWindow::onNotFilledToBelowThreshold
        },
        {
            Edge(State::NotFilled, State::AboveThreshold),
            &RollingBitWindow::onNotFilledToAboveThreshold
        },

        // BelowThreshold => AboveThreshold
        {
            Edge(State::BelowThreshold, State::AboveThreshold),
            &RollingBitWindow::onBelowToAboveThreshold
        },

        // AboveThreshold => BelowThreshold
        {
            Edge(State::AboveThreshold, State::BelowThreshold),
            &RollingBitWindow::onAboveToBelowThreshold
        },
    };

public:
    RollingBitWindow(size_t MinLength, size_t SetBitsThreshold) :
        MinLength(MinLength), SetBitsThreshold(SetBitsThreshold),
        CurrState(State::NotFilled), CurrLength(0), PrevLength(0), RBC(MinLength) {}

    std::pair<Edge, size_t> insert(bool Bit);

private:
    void onRoundtripNotFilled(State PrevState, bool NewBit) {
        (void) PrevState, (void) NewBit;

        CurrLength++;
    }

    void onRoundtripBelowThreshold(State PrevState, bool NewBit) {
        (void) PrevState, (void) NewBit;

        CurrLength = MinLength;
    }

    void onRoundtripAboveThreshold(State PrevState, bool NewBit) {
        (void) PrevState, (void) NewBit;

        CurrLength++;
    }

    void onNotFilledToBelowThreshold(State PrevState, bool NewBit) {
        (void) PrevState, (void) NewBit;

        CurrLength = MinLength;
    }

    void onNotFilledToAboveThreshold(State PrevState, bool NewBit) {
        (void) PrevState, (void) NewBit;

        CurrLength++;
    }

    void onBelowToAboveThreshold(State PrevState, bool NewBit) {
        (void) PrevState, (void) NewBit;

        CurrLength = MinLength;
    }

    void onAboveToBelowThreshold(State PrevState, bool NewBit) {
        (void) PrevState, (void) NewBit;

        CurrLength = MinLength;
    }

private:
    size_t MinLength;
    size_t SetBitsThreshold;

    State CurrState;
    size_t CurrLength;
    size_t PrevLength;
    RollingBitCounter RBC;
};

}

#endif /* ROLLING_BIT_COUNTER_H */
