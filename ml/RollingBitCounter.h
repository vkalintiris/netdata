// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ROLLING_BIT_COUNTER_H
#define ROLLING_BIT_COUNTER_H

#include "ml-private.h"

namespace ml {

class RollingBitCounter {
public:
    RollingBitCounter(size_t Capacity) : V(Capacity, 0), NumSetBits(0), N(0) {}

    void insert(bool Bit) {
        if (N >= V.size())
            NumSetBits -= (V[start()] == true);

        NumSetBits += (Bit == true);
        V[N++ % V.size()] = Bit;
    }

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

#endif /* ROLLING_BIT_COUNTER_H */
