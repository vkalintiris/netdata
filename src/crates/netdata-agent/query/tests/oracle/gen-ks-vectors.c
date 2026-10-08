// SPDX-License-Identifier: GPL-3.0-or-later
//
// C oracle for netdata-agent-query's port of KSfbar() (src/weights/ks.rs): the production KolmogorovSmirnovDist.c,
// included unchanged, answers pairs of a sample size and a statistic spread over every method the function selects,
// and the bits of each answer are written out.
//
// The header names the glibc and the CPU class the answers were made on: glibc picks its exp(), log(), log1p() and
// pow() by the CPU (FMA with AVX2, or not), and the variants' last bits differ.
//
// Build and run: tests/oracle/gen-ks-vectors.sh

#include <gnu/libc-version.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "web/api/queries/KolmogorovSmirnovDist.c"

typedef struct {
    int n;
    uint64_t x;
} PAIR;

static PAIR *pairs;
static size_t pairs_len, pairs_size;

static uint64_t to_bits(double d) {
    uint64_t u;
    memcpy(&u, &d, sizeof(u));
    return u;
}

static double from_bits(uint64_t u) {
    double d;
    memcpy(&d, &u, sizeof(d));
    return d;
}

// the function's caller passes a statistic in [0, 1]
static void add(int n, double x) {
    if (!(x >= 0.0 && x <= 1.0))
        return;
    if (pairs_len == pairs_size) {
        pairs_size = pairs_size ? pairs_size * 2 : 4096;
        pairs = realloc(pairs, pairs_size * sizeof(*pairs));
        if (!pairs)
            abort();
    }
    pairs[pairs_len++] = (PAIR){ .n = n, .x = to_bits(x) };
}

// a limit of the function: the value, and two doubles either side of it
static void around(int n, double x) {
    x = nextafter(nextafter(x, 0.0), 0.0);
    for (int u = 0; u < 5; u++) {
        add(n, x);
        x = nextafter(x, 2.0);
    }
}

// splitmix64, so the vector is the same on every run
static uint64_t rnd_state = 0x6b73666261722e31ULL;
static uint64_t rnd(void) {
    uint64_t z = (rnd_state += 0x9e3779b97f4a7c15ULL);
    z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
    z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
    return z ^ (z >> 31);
}
static int below(int n) {
    return (int)(rnd() % (uint64_t)n);
}

// the sample size and the statistic as ks_2samp() makes them (weights.c) from the two samples' sizes and the two
// indexes of the largest distance between them
static void add_sizes(int base_size, int high_size, int base_idx, int high_idx) {
    double dbase_size = (double)base_size, dhigh_size = (double)high_size;
    double d = ((double)base_idx / dbase_size) - ((double)high_idx / dhigh_size);
    if (d < 0)
        d = -d;
    if (d > 1.0)
        d = 1.0;
    double en = round(dbase_size * dhigh_size / (dbase_size + dhigh_size));
    if (isnan(en) || isinf(en) || en == 0.0)
        return;
    add((int)en, d);
}

// two indexes whose distances are close: the small statistics most requests see
static void add_sizes_near(int base_size, int high_size) {
    int base_idx = below(base_size + 1);
    int high_idx = (int)((double)base_idx * high_size / base_size) + below(41) - 20;
    if (high_idx < 0)
        high_idx = 0;
    if (high_idx > high_size)
        high_idx = high_size;
    add_sizes(base_size, high_size, base_idx, high_idx);
}

static void generate(void) {
    // 1. the function's limits, for sample sizes on both sides of each limit it has on the size
    static const int sizes[] = { 1,    2,    3,    5,    10,    29,     30,     31,     36,     37,
                                 50,   100,  199,  200,  201,   499,    500,    501,    502,    1000,
                                 2999, 3000, 3001, 5000, 10000, 100000, 100001, 200000, 200001, 250000 };
    for (size_t s = 0; s < sizeof(sizes) / sizeof(sizes[0]); s++) {
        int n = sizes[s];
        double dn = n;
        double limits[] = {
            0.5 / dn,           1.0 / dn,         1.0 - 1.0 / dn,   sqrt(0.0274 / dn),  sqrt(0.754693 / dn),
            sqrt(2.65 / dn),    sqrt(4.0 / dn),   sqrt(18.0 / dn),  sqrt(370.0 / dn),   cbrt(7.0 / (dn * dn)),
        };
        for (size_t l = 0; l < sizeof(limits) / sizeof(limits[0]); l++)
            around(n, limits[l]);
        for (int g = 0; g <= 10; g++)
            add(n, g / 10.0);
    }

    // 2. a grid of statistics for the sample sizes around 500, where the exact methods end
    for (int n = 499; n <= 502; n++)
        for (int g = 1; g < 200; g++)
            add(n, g / 200.0);

    // 3. what ks_2samp() can ask: random sizes, with random indexes and with indexes close to each other
    for (int k = 0; k < 500; k++) {
        int base_size = 1 + below(10000), high_size = 1 + below(10000);
        add_sizes(base_size, high_size, below(base_size + 1), below(high_size + 1));
        add_sizes_near(base_size, high_size);
    }

    // 4. the sizes the handler makes: a highlight of P - 1 differences and a baseline of (P << shifts) - 1
    static const int points[] = { 2,   3,   15,   16,   20,   31,   50,   64,   100,  128,  250, 256,
                                  499, 500, 501,  1000, 1024, 2000, 2500, 4096, 5000, 9999, 10000 };
    for (size_t p = 0; p < sizeof(points) / sizeof(points[0]); p++)
        for (int shifts = 0; shifts <= 13 && ((int64_t)points[p] << shifts) <= 10000; shifts++) {
            int high_size = points[p] - 1, base_size = (points[p] << shifts) - 1;
            for (int k = 0; k < 5; k++) {
                add_sizes(base_size, high_size, below(base_size + 1), below(high_size + 1));
                add_sizes_near(base_size, high_size);
            }
        }
}

static int compare(const void *l, const void *r) {
    const PAIR *a = l, *b = r;
    if (a->n != b->n)
        return a->n < b->n ? -1 : 1;
    return (a->x > b->x) - (a->x < b->x);
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s OUTPUT\n", argv[0]);
        return 2;
    }

    generate();
    qsort(pairs, pairs_len, sizeof(*pairs), compare);

    const char *cpu = "other";
#if defined(__x86_64__)
    __builtin_cpu_init();
    if (__builtin_cpu_supports("fma") && __builtin_cpu_supports("avx2"))
        cpu = "fma+avx2";
#endif

    FILE *fp = fopen(argv[1], "w");
    if (!fp) {
        perror(argv[1]);
        return 1;
    }
    fprintf(fp, "# KSfbar(n, x) of src/web/api/queries/KolmogorovSmirnovDist.c; made by "
                "tests/oracle/gen-ks-vectors.sh\n");
    fprintf(fp, "# glibc %s\n", gnu_get_libc_version());
    fprintf(fp, "# cpu %s\n", cpu);
    fprintf(fp, "# n x answer (x and the answer as the bits of the doubles, in hex)\n");
    size_t written = 0;
    for (size_t i = 0; i < pairs_len; i++) {
        if (i && pairs[i].n == pairs[i - 1].n && pairs[i].x == pairs[i - 1].x)
            continue;
        double answer = KSfbar(pairs[i].n, from_bits(pairs[i].x));
        fprintf(fp, "%d %016llx %016llx\n", pairs[i].n, (unsigned long long)pairs[i].x,
                (unsigned long long)to_bits(answer));
        written++;
    }
    if (fclose(fp)) {
        perror(argv[1]);
        return 1;
    }
    fprintf(stderr, "%zu pairs (glibc %s, cpu %s)\n", written, gnu_get_libc_version(), cpu);
    return 0;
}
