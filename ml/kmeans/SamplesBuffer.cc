// SPDX-License-Identifier: GPL-3.0-or-later
//
#include "SamplesBuffer.h"

#include <fstream>
#include <sstream>
#include <string>

void Sample::print(std::ostream &OS) const {
    for (size_t Idx = 0; Idx != NumDims - 1; Idx++)
        OS << CNs[Idx] << ", ";

    OS << CNs[NumDims - 1];
}

void SamplesBuffer::print(std::ostream &OS) const {
    for (size_t Idx = Preprocessed ? (DiffN + (SmoothN - 1) + (LagN - 1)) : 0;
         Idx != NumSamples; Idx++) {
        Sample S = Preprocessed ? getPreprocessedSample(Idx) : getSample(Idx);
        OS << S << std::endl;
    }
}

void SamplesBuffer::diffSamples() {
    for (size_t Idx = 0; Idx != (NumSamples - DiffN); Idx++) {
        size_t High = (NumSamples - 1) - Idx;
        size_t Low = High - DiffN;

        Sample LHS = getSample(High);
        Sample RHS = getSample(Low);

        LHS.diff(RHS);
    }
}

void SamplesBuffer::smoothSamples() {
    // Holds the mean value of each window
    CalculatedNumber *AccCNs = new CalculatedNumber[NumDimsPerSample]();
    Sample Acc = Sample(AccCNs, NumDimsPerSample);

    // Used to avoid clobbering the accumulator when moving the window
    CalculatedNumber *TmpCNs = new CalculatedNumber[NumDimsPerSample]();
    Sample Tmp = Sample(TmpCNs, NumDimsPerSample);

    CalculatedNumber Factor = (CalculatedNumber) 1 / SmoothN;

    // Calculate the value of the 1st window
    for (size_t Idx = 0; Idx != std::min(SmoothN, NumSamples); Idx++) {
        Tmp.add(getSample(NumSamples - (Idx + 1)));
    }

    Acc.add(Tmp);
    Acc.scale(Factor);

    // Move the window and update the samples
    for (size_t Idx = NumSamples; Idx != (DiffN + SmoothN - 1); Idx--) {
        Sample S = getSample(Idx - 1);

        // Tmp <- Next window (if any)
        if (Idx >= (SmoothN + 1)) {
            Tmp.diff(S);
            Tmp.add(getSample(Idx - (SmoothN + 1)));
        }

        // S <- Acc
        S.copy(Acc);

        // Acc <- Tmp
        Acc.copy(Tmp);
        Acc.scale(Factor);
    }

    delete[] AccCNs;
    delete[] TmpCNs;
}

void SamplesBuffer::lagSamples() {
    if (LagN == 0)
        return;

    for (size_t Idx = NumSamples; Idx != LagN; Idx--) {
        Sample PS = getPreprocessedSample(Idx - 1);
        PS.lag(getSample(Idx - 1), LagN);
    }
}

std::vector<DSample> SamplesBuffer::preprocess() {
    assert(Preprocessed == false);

    std::vector<DSample> DSamples;
    size_t OutN = NumSamples;

    // Diff
    if (DiffN >= OutN)
        return DSamples;
    OutN -= DiffN;
    diffSamples();

    // Smooth
    if (SmoothN == 0 || SmoothN > OutN)
        return DSamples;
    OutN -= (SmoothN - 1);
    smoothSamples();

    // Lag
    if (LagN >= OutN)
        return DSamples;
    OutN -= LagN;
    lagSamples();

    Preprocessed = true;

    for (size_t Idx = NumSamples - OutN; Idx != NumSamples; Idx++) {
        DSample DS;
        DS.set_size(NumDimsPerSample * (LagN + 1));

        const Sample PS = getPreprocessedSample(Idx);
        PS.initDSample(DS);

        DSamples.push_back(DS);
    }

    return DSamples;
}

bool SamplesBuffer::testOk(const std::string filename) {
    size_t NumSamples, NumDimsPerSample;
    size_t DiffN, SmoothN, LagN;
    size_t OutN;

    std::ifstream ifs;
    ifs.open(filename, std::ios_base::in);

    // Read test config values
    ifs >> NumSamples;
    ifs >> NumDimsPerSample;
    ifs >> DiffN;
    ifs >> SmoothN;
    ifs >> LagN;

    // Read samples
    CalculatedNumber *Buf =
        new CalculatedNumber[NumSamples * NumDimsPerSample * (LagN + 1)]();
    for (size_t Idx = 0; Idx != NumSamples; Idx++)
        for (size_t Dim = 0; Dim != NumDimsPerSample; Dim++)
            ifs >> Buf[(Idx * NumDimsPerSample) + Dim];

    // Preprocess
    SamplesBuffer SB = SamplesBuffer(Buf,
                                     NumSamples, NumDimsPerSample,
                                     DiffN, SmoothN, LagN);
    std::vector<DSample> DSamples = SB.preprocess();

    // Make sure the number of rows matches
    ifs >> OutN;
    if (DSamples.size() != OutN) {
        std::cerr << "Expected " << OutN << " rows" << std::endl;
        std::cerr << "Got " << DSamples.size() << " rows instead" << std::endl;
        return false;
    }

    // Make sure the values of each row match
    CalculatedNumber CN;
    for (size_t Idx = 0; Idx != OutN; Idx++) {
        DSample &DS = DSamples[Idx];

        for (size_t Dim = 0; Dim != NumDimsPerSample * (LagN + 1); Dim++) {
            ifs >> CN;

            if ((CN - DS(Dim)) < 0.001)
                continue;

            std::cerr << "CN: " << CN << std::endl;
            std::cerr << "DS[" << Idx << "](" << Dim << "): " << DS(Dim) << std::endl;
            return false;
        }
    }

    delete[] Buf;
    return true;
}
