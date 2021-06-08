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
