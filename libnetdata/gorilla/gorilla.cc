// SPDX-License-Identifier: GPL-3.0-or-later

#include "gorilla.h"

#include <cassert>
#include <climits>
#include <cstdio>
#include <cstring>

using std::size_t;

template <typename T>
static constexpr size_t bit_size() noexcept
{
    static_assert((sizeof(T) * CHAR_BIT) == 32 || (sizeof(T) * CHAR_BIT) == 64,
                  "Word size should be 32 or 64 bits.");
    return (sizeof(T) * CHAR_BIT);
}

/*
 * bit buffer
*/

static void bit_buffer_write(uint32_t *buf, size_t pos, uint32_t v, size_t nbits)
{
    assert(nbits > 0 && nbits <= bit_size<uint32_t>());

    const size_t index = pos / bit_size<uint32_t>();
    const size_t offset = pos % bit_size<uint32_t>();

    pos += nbits;

    if (offset == 0) {
        buf[index] = v;
    } else {
        const size_t remaining_bits = bit_size<uint32_t>() - offset;

        // write the lower part of the value
        const uint32_t low_bits_mask = ((uint32_t) 1 << remaining_bits) - 1;
        const uint32_t lowest_bits_in_value = v & low_bits_mask;
        buf[index] |= (lowest_bits_in_value << offset);

        if (nbits > remaining_bits) {
            // write the upper part of the value
            const uint32_t high_bits_mask = ~low_bits_mask;
            const uint32_t highest_bits_in_value = (v & high_bits_mask) >> (remaining_bits);
            buf[index + 1] = highest_bits_in_value;
        }
    }
}

static void bit_buffer_read(const uint32_t *buf, size_t pos, uint32_t *v, size_t nbits)
{
    assert(nbits > 0 && nbits <= bit_size<uint32_t>());

    const size_t index = pos / bit_size<uint32_t>();
    const size_t offset = pos % bit_size<uint32_t>();

    pos += nbits;

    if (offset == 0) {
        *v = (nbits == bit_size<uint32_t>()) ?
                    buf[index] :
                    buf[index] & (((uint32_t) 1 << nbits) - 1);
    } else {
        const size_t remaining_bits = bit_size<uint32_t>() - offset;

        // extract the lower part of the value
        if (nbits < remaining_bits) {
            *v = (buf[index] >> offset) & (((uint32_t) 1 << nbits) - 1);
        } else {
            *v = (buf[index] >> offset) & (((uint32_t) 1 << remaining_bits) - 1);
            nbits -= remaining_bits;
            *v |= (buf[index + 1] & (((uint32_t) 1 << nbits) - 1)) << remaining_bits;
        }
    }
}

/*
 * bit stream reader
*/

typedef struct {
    size_t cap;
    size_t pos;
} bit_stream_reader_t;

bit_stream_reader_t bit_stream_reader_init(const uint32_t *buffer)
{
    size_t capacity = __atomic_load_n(&buffer[0], __ATOMIC_SEQ_CST);

    return bit_stream_reader_t {
        .cap = capacity,
        .pos = bit_size<uint32_t>(),
    };
}

bool bit_stream_reader_read(bit_stream_reader_t *bsr, const uint32_t *buf, uint32_t *value, size_t nbits)
{
    if (bsr->pos + nbits > bsr->cap)
        return false;

    bit_buffer_read(buf, bsr->pos, value, nbits);
    bsr->pos += nbits;

    return true;
}

/*
 * gorilla writer
*/

typedef struct {
    uint32_t *buffer;
    uint32_t entries;

    uint32_t prev_number;
    uint32_t prev_xor_lzc;

    // in bits
    uint32_t position;
    uint32_t capacity;
} gorilla_writer_t;

gorilla_writer_t gorilla_writer_init(uint32_t *buf, size_t n)
{
    uint32_t capacity = n * bit_size<uint32_t>();

    return gorilla_writer_t {
        .buffer = buf,
        .entries = 0,
        .prev_number = 0,
        .prev_xor_lzc = 0,
        .position = 2 * bit_size<uint32_t>(),
        .capacity = capacity,
    };
}

bool gorilla_writer_write(gorilla_writer_t *gw, uint32_t number)
{
    // this is the first number we are writing
    if (gw->entries == 0) {
        if (gw->position + bit_size<uint32_t>() >= gw->capacity)
            return false;
        bit_buffer_write(gw->buffer, gw->position, number, bit_size<uint32_t>());

        gw->position += bit_size<uint32_t>();
        gw->entries++;
        gw->prev_number = number;
        return true;
    }

    // write true/false based on whether we got the same number or not.
    if (number == gw->prev_number) {
        if (gw->position + 1 >= gw->capacity)
            return false;
        bit_buffer_write(gw->buffer, gw->position, static_cast<uint32_t>(1), 1);
        gw->position++;
        gw->entries++;
        return true;
    }

    if (gw->position + 1 >= gw->capacity)
        return false;
    bit_buffer_write(gw->buffer, gw->position,static_cast<uint32_t>(0), 1);
    gw->position++;

    uint32_t xor_value = gw->prev_number ^ number;
    uint32_t xor_lzc = (bit_size<uint32_t>() == 32) ? __builtin_clz(xor_value) : __builtin_clzll(xor_value);
    uint32_t is_xor_lzc_same = (xor_lzc == gw->prev_xor_lzc) ? 1 : 0;

    if (gw->position + 1 >= gw->capacity)
        return false;
    bit_buffer_write(gw->buffer, gw->position, is_xor_lzc_same, 1);
    gw->position++;
    
    if (!is_xor_lzc_same) {
        if (gw->position + 1 >= gw->capacity)
            return false;
        bit_buffer_write(gw->buffer, gw->position, xor_lzc, (bit_size<uint32_t>() == 32) ? 5 : 6);
        gw->position += (bit_size<uint32_t>() == 32) ? 5 : 6;
    }

    // write the bits of the XOR'd value without the LZC prefix
    if (gw->position + (bit_size<uint32_t>() - xor_lzc) >= gw->capacity)
        return false;
    bit_buffer_write(gw->buffer, gw->position, xor_value, bit_size<uint32_t>() - xor_lzc);
    gw->position += bit_size<uint32_t>() - xor_lzc;

    gw->entries++;
    gw->prev_number = number;
    gw->prev_xor_lzc = xor_lzc;
    return true;
}

void gorilla_writer_flush(gorilla_writer_t *gw)
{
    __atomic_store_n(&gw->buffer[0], gw->entries, __ATOMIC_RELAXED);
    __atomic_store_n(&gw->buffer[1], gw->position, __ATOMIC_RELAXED);
}

/*
 * gorilla_reader_t
*/

typedef struct {
    const uint32_t *buffer;
    bit_stream_reader_t bsr;

    size_t position;

    uint32_t prev_number;
    uint32_t prev_xor_lzc;
    uint32_t prev_xor;
} gorilla_reader_t;

gorilla_reader_t gorilla_reader_init(const uint32_t *buf)
{
    return gorilla_reader_t {
        .buffer = buf,
        .bsr = bit_stream_reader_init(&buf[1]),
        .position = 0,
        .prev_number = 0,
        .prev_xor_lzc = 0,
        .prev_xor = 0,
    };
}

const uint32_t *gorilla_reader_bit_buffer(const gorilla_reader_t *gr)
{
    return &gr->buffer[1];
}

bool gorilla_reader_read(gorilla_reader_t *gr, uint32_t *number)
{
    const uint32_t *bit_buffer = gorilla_reader_bit_buffer(gr);
    
    // read the first number
    if (gr->position == 0) {
        bool ok = bit_stream_reader_read(&gr->bsr, bit_buffer, number, bit_size<uint32_t>());

        if (ok) {
            gr->position++;
            gr->prev_number = *number;
        }

        return ok;
    }

    // process same-number bit
    uint32_t is_same_number;
    if (!bit_stream_reader_read(&gr->bsr, bit_buffer, &is_same_number, 1)) {
        return false;
    }

    if (is_same_number) {
        *number = gr->prev_number;
        return true;
    }

    // proceess same-xor-lzc bit
    uint32_t xor_lzc = gr->prev_xor_lzc;

    uint32_t same_xor_lzc;
    if (!bit_stream_reader_read(&gr->bsr, bit_buffer, &same_xor_lzc, 1)) {
        return false;
    }

    if (!same_xor_lzc) {
        if (!bit_stream_reader_read(&gr->bsr, bit_buffer, &xor_lzc, (bit_size<uint32_t>() == 32) ? 5 : 6)) {
            return false;        
        }
    }

    // process the non-lzc suffix
    uint32_t xor_value = 0;
    if (!bit_stream_reader_read(&gr->bsr, bit_buffer, &xor_value, bit_size<uint32_t>() - xor_lzc)) {
        return false;        
    }

    *number = (gr->prev_number ^ xor_value);

    gr->position++;
    gr->prev_number = *number;
    gr->prev_xor_lzc = xor_lzc;
    gr->prev_xor = xor_value;

    return true;
}

size_t gorilla_reader_entries(const gorilla_reader_t *gr)
{
    return __atomic_load_n(&gr->buffer[0], __ATOMIC_SEQ_CST);
}

const uint32_t *gorilla_reader_data(const gorilla_reader_t *gr)
{
    return gr->buffer;
}

/*
 * Internal code used for fuzzing the library
*/

#ifdef ENABLE_FUZZER

#include <vector>

template<typename Word>
static std::vector<Word> random_vector(const uint8_t *data, size_t size) {
    std::vector<Word> V;

    V.reserve(1024);

    while (size >= sizeof(Word)) {
        size -= sizeof(Word);

        Word w;
        memcpy(&w, &data[size], sizeof(Word));
        V.push_back(w);
    }

    return V;
}

extern "C" int LLVMFuzzerTestOneInput(const uint8_t *Data, size_t Size) {
    // 32-bit tests
    {
        if (Size < 4)
            return 0;

        std::vector<uint32_t> RandomData = random_vector<uint32_t>(Data, Size);
        std::vector<uint32_t> EncodedData(10 * RandomData.capacity(), 0);

        // write data
        {
            gorilla_writer_t gw = gorilla_writer_init(EncodedData.data(), EncodedData.capacity());
            for (size_t i = 0; i != RandomData.size(); i++)
                gorilla_writer_write(&gw, RandomData[i]);

            gorilla_writer_flush(&gw);
        }

        // read data
        {
            gorilla_reader_t gr = gorilla_reader_init(EncodedData.data());

            assert((gorilla_reader_entries(&gr) == RandomData.size()) &&
                   "Bad number of entries in gorilla buffer");

            for (size_t i = 0; i != RandomData.size(); i++) {
                uint32_t number = 0;
                bool ok = gorilla_reader_read(&gr, &number);
                assert(ok && "Failed to read number from gorilla buffer");

                assert((number == RandomData[i])
                        && "Read wrong number from gorilla buffer");
            }
        }
    }

    return 0;
}

#endif /* ENABLE_FUZZER */

#ifdef ENABLE_BENCHMARK

#include <benchmark/benchmark.h>
#include <random>

static size_t NumItems = 1024;

static void BM_EncodeU32Numbers(benchmark::State& state) {
    std::random_device rd;
    std::mt19937 mt(rd());
    std::uniform_int_distribution<uint32_t> dist(0x0, 0x0000FFFF);

    std::vector<uint32_t> RandomData;
    for (size_t idx = 0; idx != NumItems; idx++) {
        RandomData.push_back(dist(mt));
    }
    std::vector<uint32_t> EncodedData(10 * RandomData.capacity(), 0);

    for (auto _ : state) {
        gorilla_writer_t gw = gorilla_writer_init(EncodedData.data(), EncodedData.size());

        for (size_t i = 0; i != RandomData.size(); i++)
            benchmark::DoNotOptimize(gorilla_writer_write(&gw, RandomData[i]));

        benchmark::ClobberMemory();
    }

    state.SetItemsProcessed(NumItems * state.iterations());
    state.SetBytesProcessed(NumItems * state.iterations() * sizeof(uint32_t));
}
BENCHMARK(BM_EncodeU32Numbers);

static void BM_DecodeU32Numbers(benchmark::State& state) {
    std::random_device rd;
    std::mt19937 mt(rd());
    std::uniform_int_distribution<uint32_t> dist(0x0, 0xFFFFFFFF);

    std::vector<uint32_t> RandomData;
    for (size_t idx = 0; idx != NumItems; idx++) {
        RandomData.push_back(dist(mt));
    }
    std::vector<uint32_t> EncodedData(10 * RandomData.capacity(), 0);
    std::vector<uint32_t> DecodedData(10 * RandomData.capacity(), 0);

    gorilla_writer_t gw = gorilla_writer_init(EncodedData.data(), EncodedData.size());
    for (size_t i = 0; i != RandomData.size(); i++)
        gorilla_writer_write(&gw, RandomData[i]);
    gorilla_writer_flush(&gw);

    for (auto _ : state) {
        gorilla_reader_t gr = gorilla_reader_init(EncodedData.data());

        for (size_t i = 0; i != RandomData.size(); i++) {
            uint32_t number = 0;
            benchmark::DoNotOptimize(gorilla_reader_read(&gr, &number));
        }

        benchmark::ClobberMemory();
    }

    state.SetItemsProcessed(NumItems * state.iterations());
    state.SetBytesProcessed(NumItems * state.iterations() * sizeof(uint32_t));
}
BENCHMARK(BM_DecodeU32Numbers);

#endif /* ENABLE_BENCHMARK */
