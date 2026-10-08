// SPDX-License-Identifier: GPL-3.0-or-later
//
// C oracle for netdata-agent-query's ports of KSfbar() (src/weights/ks.rs) and of ks_2samp() (src/weights/ks2.rs).
//
// The production KolmogorovSmirnovDist.c, included unchanged, answers pairs of a sample size and a statistic spread
// over every method the function selects, and the bits of each answer are written out. The two-sample statistic's
// own text, which gen-ks-vectors.sh cuts from weights.c by its boundaries (weights.c cannot be compiled alone),
// answers the 20000 seeded trials of C's unit test ks2_cursor_unittest(), and calculate_pairs_diff() turns pairs of
// doubles into its integers.
//
// The header names the glibc and the CPU class the answers were made on: glibc picks its exp(), log(), log1p() and
// pow() by the CPU (FMA with AVX2, or not), and the variants' last bits differ.
//
// Build and run: tests/oracle/gen-ks-vectors.sh

#include <gnu/libc-version.h>
#include <limits.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "web/api/queries/KolmogorovSmirnovDist.c"

// what the text cut from weights.c needs of libnetdata; NETDATA_DOUBLE is double in the builds the agent ships
#define likely(x) __builtin_expect(!!(x), 1)
#define unlikely(x) __builtin_expect(!!(x), 0)
typedef double NETDATA_DOUBLE;
#include "weights-ks2.inc"

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

static FILE *open_vector(const char *filename, const char *what, const char *columns) {
    const char *cpu = "other";
#if defined(__x86_64__)
    __builtin_cpu_init();
    if (__builtin_cpu_supports("fma") && __builtin_cpu_supports("avx2"))
        cpu = "fma+avx2";
#endif

    FILE *fp = fopen(filename, "w");
    if (!fp) {
        perror(filename);
        exit(1);
    }
    fprintf(fp, "# %s; made by tests/oracle/gen-ks-vectors.sh\n", what);
    fprintf(fp, "# glibc %s\n", gnu_get_libc_version());
    fprintf(fp, "# cpu %s\n", cpu);
    fprintf(fp, "# %s\n", columns);
    return fp;
}

static void close_vector(FILE *fp, const char *filename, size_t written) {
    if (fclose(fp)) {
        perror(filename);
        exit(1);
    }
    fprintf(stderr, "%s: %zu lines\n", filename, written);
}

static void write_ksfbar(const char *filename) {
    generate();
    qsort(pairs, pairs_len, sizeof(*pairs), compare);

    FILE *fp = open_vector(filename, "KSfbar(n, x) of src/web/api/queries/KolmogorovSmirnovDist.c",
                           "n x answer (x and the answer as the bits of the doubles, in hex)");
    size_t written = 0;
    for (size_t i = 0; i < pairs_len; i++) {
        if (i && pairs[i].n == pairs[i - 1].n && pairs[i].x == pairs[i - 1].x)
            continue;
        double answer = KSfbar(pairs[i].n, from_bits(pairs[i].x));
        fprintf(fp, "%d %016llx %016llx\n", pairs[i].n, (unsigned long long)pairs[i].x,
                (unsigned long long)to_bits(answer));
        written++;
    }
    close_vector(fp, filename, written);
}

// ks2_test_random() of weights.c
static uint64_t ks2_random(uint64_t *state) {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    return *state;
}

// read at run time, so the compiler cannot make the casts itself
static volatile double cast_values[] = {
    0.0,     -0.0,    1.0,      -1.0,      0.5,      1e-5,     1.9e-5,   -1.9e-5,  2.5e-5,   0.99999e-5, 123.456789,
    1e9,     1e13,    9.2e13,   9.22e13,   9.223e13, 9.224e13, 9.3e13,   1e14,     -9.2e13,  -9.224e13,  -1e14,
    1e300,   -1e300,  INFINITY, -INFINITY, NAN,
};
#define CAST_VALUES (sizeof(cast_values) / sizeof(cast_values[0]))

static void write_ks2samp(const char *filename) {
    FILE *fp = open_vector(filename, "ks_2samp() and calculate_pairs_diff() of src/web/api/queries/weights.c",
                           "`trial <answer>`: the trials of ks2_cursor_unittest(), in its order; then `diff <first> "
                           "<second> <change>`: the integer made of two doubles (all as bits, in hex)");
    size_t written = 0;

    // the trials of ks2_cursor_unittest(), from its seed
    const int max_points = 10000;
    DIFFS_NUMBERS *base = malloc((size_t)max_points * sizeof(*base));
    DIFFS_NUMBERS *high = malloc((size_t)max_points * sizeof(*high));
    if (!base || !high)
        abort();
    uint64_t random = 0x9182abcd1234ULL;
    for (size_t trial = 0; trial < 20000; trial++) {
        int bs = 1 + (int)(ks2_random(&random) % 64);
        int hs = 1 + (int)(ks2_random(&random) % 64);
        uint32_t shifts = (uint32_t)(ks2_random(&random) % 10);
        if (trial >= 19980) {
            bs = max_points - 1;
            hs = trial % 2 ? 14 : max_points - 1;
            shifts = (uint32_t)(trial % 10);
        }
        for (int i = 0; i < bs; i++)
            base[i] = (DIFFS_NUMBERS)(ks2_random(&random) % 17) - 8;
        for (int i = 0; i < hs; i++)
            high[i] = (DIFFS_NUMBERS)(ks2_random(&random) % 17) - 8;
        if (trial % 3 == 0)
            base[0] = LONG_MIN;
        if (trial % 5 == 0)
            high[0] = LONG_MAX;
        fprintf(fp, "trial %016llx\n", (unsigned long long)to_bits(ks_2samp(base, bs, high, hs, shifts)));
        written++;
    }
    free(base);
    free(high);

    // calculate_pairs_diff() of every pair of the values
    for (size_t f = 0; f < CAST_VALUES; f++)
        for (size_t s = 0; s < CAST_VALUES; s++) {
            NETDATA_DOUBLE pair[2] = { cast_values[f], cast_values[s] };
            DIFFS_NUMBERS change = 0;
            if (calculate_pairs_diff(&change, pair, 2) != 1)
                abort();
            fprintf(fp, "diff %016llx %016llx %016llx\n", (unsigned long long)to_bits(pair[0]),
                    (unsigned long long)to_bits(pair[1]), (unsigned long long)change);
            written++;
        }
    close_vector(fp, filename, written);
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: %s KSFBAR_OUTPUT KS2SAMP_OUTPUT\n", argv[0]);
        return 2;
    }
    write_ksfbar(argv[1]);
    write_ks2samp(argv[2]);
    return 0;
}
