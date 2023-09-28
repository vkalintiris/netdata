#include "storage_engine_benchmarks.h"
#include <benchmark/benchmark.h>

int Sum(int a, int b) {
    return a + b;
}

static void BM_SumFunction(benchmark::State& state) {
    for (auto _ : state) {
        int result = Sum(5, 7);
        benchmark::DoNotOptimize(result);
    }
}
BENCHMARK(BM_SumFunction);

int storage_engine_benchmarks(int argc, char *argv[]) {
    ::benchmark::Initialize(&argc, argv);

    // Run the benchmark
    ::benchmark::RunSpecifiedBenchmarks();

    return 0;
}

