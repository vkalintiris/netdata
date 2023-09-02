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

typedef struct {
    // uint32_t *next;
    uint32_t entries;
    uint32_t nbits;
} gorilla_header_t;

typedef struct {
    gorilla_header_t header;
    uint32_t data[];
} gorilla_buffer_t;

typedef struct {
    gorilla_buffer_t *buffer;

    uint32_t entries;

    uint32_t prev_number;
    uint32_t prev_xor_lzc;

    // in bits
    uint32_t position;
    uint32_t capacity;
} gorilla_writer_t;

gorilla_writer_t gorilla_writer_init(uint32_t *buf, size_t n)
{
    gorilla_buffer_t *buffer = reinterpret_cast<gorilla_buffer_t *>(buf);

    // __atomic_store_n(&buffer->header.next, 0, __ATOMIC_RELAXED);
    __atomic_store_n(&buffer->header.entries, 0, __ATOMIC_RELAXED);
    __atomic_store_n(&buffer->header.nbits, 0, __ATOMIC_RELAXED);

    uint32_t capacity = n * bit_size<uint32_t>();

    return gorilla_writer_t {
        .buffer = buffer,
        .entries = 0,
        .prev_number = 0,
        .prev_xor_lzc = 0,
        .position = 0,
        .capacity = capacity,
    };
}

bool gorilla_writer_write(gorilla_writer_t *gw, uint32_t number)
{
    gorilla_header_t *hdr = &gw->buffer->header;
    uint32_t *data = gw->buffer->data;

    // this is the first number we are writing
    if (hdr->entries == 0) {
        if (hdr->nbits + bit_size<uint32_t>() >= gw->capacity)
            return false;
        bit_buffer_write(data, hdr->nbits, number, bit_size<uint32_t>());

        __atomic_fetch_add(&hdr->nbits, bit_size<uint32_t>(), __ATOMIC_RELAXED);
        __atomic_fetch_add(&hdr->entries, 1, __ATOMIC_RELAXED);
        gw->prev_number = number;
        return true;
    }

    // write true/false based on whether we got the same number or not.
    if (number == gw->prev_number) {
        if (hdr->nbits + 1 >= gw->capacity)
            return false;

        bit_buffer_write(data, hdr->nbits, static_cast<uint32_t>(1), 1);
        __atomic_fetch_add(&hdr->nbits, 1, __ATOMIC_RELAXED);
        __atomic_fetch_add(&hdr->entries, 1, __ATOMIC_RELAXED);
        return true;
    }

    if (hdr->nbits + 1 >= gw->capacity)
        return false;
    bit_buffer_write(data, hdr->nbits, static_cast<uint32_t>(0), 1);
    __atomic_fetch_add(&hdr->nbits, 1, __ATOMIC_RELAXED);

    uint32_t xor_value = gw->prev_number ^ number;
    uint32_t xor_lzc = (bit_size<uint32_t>() == 32) ? __builtin_clz(xor_value) : __builtin_clzll(xor_value);
    uint32_t is_xor_lzc_same = (xor_lzc == gw->prev_xor_lzc) ? 1 : 0;

    if (hdr->nbits + 1 >= gw->capacity)
        return false;
    bit_buffer_write(data, hdr->nbits, is_xor_lzc_same, 1);
    __atomic_fetch_add(&hdr->nbits, 1, __ATOMIC_RELAXED);
    
    if (!is_xor_lzc_same) {
        if (hdr->nbits + 1 >= gw->capacity)
            return false;
        bit_buffer_write(data, hdr->nbits, xor_lzc, (bit_size<uint32_t>() == 32) ? 5 : 6);
        __atomic_fetch_add(&hdr->nbits, (bit_size<uint32_t>() == 32) ? 5 : 6, __ATOMIC_RELAXED);
    }

    // write the bits of the XOR'd value without the LZC prefix
    if (hdr->nbits + (bit_size<uint32_t>() - xor_lzc) >= gw->capacity)
        return false;
    bit_buffer_write(data, hdr->nbits, xor_value, bit_size<uint32_t>() - xor_lzc);
    __atomic_fetch_add(&hdr->nbits, bit_size<uint32_t>() - xor_lzc, __ATOMIC_RELAXED);
    __atomic_fetch_add(&hdr->entries, 1, __ATOMIC_RELAXED);

    gw->prev_number = number;
    gw->prev_xor_lzc = xor_lzc;
    return true;
}

typedef struct {
    const gorilla_buffer_t *buffer;

    // number of values
    size_t length;
    size_t entries;

    // in bits
    size_t capacity;
    size_t position;

    uint32_t prev_number;
    uint32_t prev_xor_lzc;
    uint32_t prev_xor;

} gorilla_reader_t;

gorilla_reader_t gorilla_reader_init(const uint32_t *buf)
{
    const gorilla_buffer_t *buffer = reinterpret_cast<const gorilla_buffer_t *>(buf);

    uint32_t length = __atomic_load_n(&buffer->header.entries, __ATOMIC_SEQ_CST);
    uint32_t capacity = __atomic_load_n(&buffer->header.nbits, __ATOMIC_SEQ_CST);

    return gorilla_reader_t {
        .buffer = buffer,
        .length = length,
        .entries = 0,
        .capacity = capacity,
        .position = 0,
        .prev_number = 0,
        .prev_xor_lzc = 0,
        .prev_xor = 0,
    };
}

bool gorilla_reader_read(gorilla_reader_t *gr, uint32_t *number)
{
    const uint32_t *data = gr->buffer->data;

    if (gr->entries + 1 > gr->length)
        return false;

    // read the first number
    if (gr->entries == 0) {
        bit_buffer_read(data, gr->position, number, bit_size<uint32_t>());

        gr->entries++;
        gr->position += bit_size<uint32_t>();
        gr->prev_number = *number;
        return true;
    }

    // process same-number bit
    uint32_t is_same_number;
    bit_buffer_read(data, gr->position, &is_same_number, 1);
    gr->position++;

    if (is_same_number) {
        *number = gr->prev_number;
        gr->entries++;
        return true;
    }

    // proceess same-xor-lzc bit
    uint32_t xor_lzc = gr->prev_xor_lzc;

    uint32_t same_xor_lzc;
    bit_buffer_read(data, gr->position, &same_xor_lzc, 1);
    gr->position++;

    if (!same_xor_lzc) {
        bit_buffer_read(data, gr->position, &xor_lzc, (bit_size<uint32_t>() == 32) ? 5 : 6);
        gr->position += (bit_size<uint32_t>() == 32) ? 5 : 6;
    }

    // process the non-lzc suffix
    uint32_t xor_value = 0;
    bit_buffer_read(data, gr->position, &xor_value, bit_size<uint32_t>() - xor_lzc);
    gr->position += bit_size<uint32_t>() - xor_lzc;

    *number = (gr->prev_number ^ xor_value);

    gr->entries++;
    gr->prev_number = *number;
    gr->prev_xor_lzc = xor_lzc;
    gr->prev_xor = xor_value;

    return true;
}

size_t gorilla_reader_entries(const gorilla_reader_t *gr)
{
    return gr->length;
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
