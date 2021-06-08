#include "RollingBitCounter.h"

using namespace ml;

void RollingBitCounter::print(std::ostream &OS) const {
    OS << "Start: " << start() << ", Size: " << size() << ", N: " << N << std::endl;

    std::cout << "\tV: ";

    size_t StartIdx = start();
    size_t EndIdx = StartIdx + size();

    for (size_t I = StartIdx; I != EndIdx; I++)
        OS << V[I % V.size()];

    OS << " (set bits: " << NumSetBits << ")";
}

#if 0
int main(int argc, char *argv[]) {
    (void) argc;
    (void) argv;

    std::vector<bool> V{0, 0, 1, 1, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0};

    RollingBitCounter RBC(4);

    std::cout << "Starting BW:\n\t" << RBC << "\n" << std::endl;

    for (bool B : V) {
        RBC.append(B);
        std::cout << "\t" << RBC << std::endl;
        std::cout << std::endl;
    }

    return 0;
}
#endif
