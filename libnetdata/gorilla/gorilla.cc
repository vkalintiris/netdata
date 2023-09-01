// SPDX-License-Identifier: GPL-3.0-or-later

#include "gorilla.h"

#include <cassert>
#include <climits>
#include <cstdio>
#include <cstring>

#include <forward_list>

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

typedef struct {
    size_t capacity;
} bit_buffer_t;

bit_buffer_t bit_buffer_init(size_t capacity)
{
    return bit_buffer_t {
        capacity
    };
}

bool bit_buffer_write(const bit_buffer_t *bb, uint32_t *buf, size_t pos, uint32_t v, size_t nbits)
{
    assert(nbits > 0 && nbits <= bit_size<uint32_t>());

    if ((pos + nbits) > bb->capacity)
        return false;

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

    return true;
}

bool bit_buffer_read(const bit_buffer_t *bb, const uint32_t *buf, size_t pos, uint32_t *v, size_t nbits)
{
    assert(nbits > 0 && nbits <= bit_size<uint32_t>());

    if (pos + nbits > bb->capacity)
        return false;

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

    return true;
}

size_t bit_buffer_capacity(const bit_buffer_t *bb)
{
    return bb->capacity;
}

/*
 * bit stream writer
*/

typedef struct {
    bit_buffer_t bb;
    size_t pos;
} bit_stream_writer_t;

bit_stream_writer_t bit_stream_writer_init(size_t capacity)
{
    assert(bit_size<uint32_t>() <= capacity);

    return bit_stream_writer_t {
        .bb = bit_buffer_init(capacity),
        .pos = bit_size<uint32_t>()
    };
}

bool bit_stream_writer_write(bit_stream_writer_t *bsw, uint32_t *buf, uint32_t value, size_t nbits)
{
    bool ok = bit_buffer_write(&bsw->bb, buf, bsw->pos, value, nbits);

    if (ok)
        bsw->pos += nbits;

    return ok;
}

void bit_stream_writer_flush(const bit_stream_writer_t *bsw, uint32_t *buf)
{
    __atomic_store_n(&buf[0], bsw->pos, __ATOMIC_RELAXED);
}

size_t bit_stream_writer_capacity(const bit_stream_writer_t *bsw)
{
        return bsw->bb.capacity;
}

size_t bit_stream_writer_position(const bit_stream_writer_t *bsw)
{
        return bsw->pos;
}

/*
 * bit stream reader
*/

typedef struct {
    bit_buffer_t bb;
    size_t pos;
} bit_stream_reader_t;

bit_stream_reader_t bit_stream_reader_init(const uint32_t *buffer)
{
    size_t capacity = __atomic_load_n(&buffer[0], __ATOMIC_SEQ_CST);

    return bit_stream_reader_t {
        .bb = bit_buffer_init(capacity),
        .pos = bit_size<uint32_t>(),
    };
}

bool bit_stream_reader_read(bit_stream_reader_t *bsr, const uint32_t *buf, uint32_t *value, size_t nbits)
{
    bool ok = bit_buffer_read(&bsr->bb, buf, bsr->pos, value, nbits);

    if (ok)
        bsr->pos += nbits;

    return ok;
}

size_t bit_stream_reader_capacity(const bit_stream_reader_t *bsr)
{
    return bit_buffer_capacity(&bsr->bb);
}

size_t bit_stream_reader_position(const bit_stream_reader_t *bsr)
{
    return bsr->pos;
}

/*
 * gorilla writer
*/

typedef struct {
    uint32_t *buffer;
    uint32_t entries;

    bit_stream_writer_t bsw;

    uint32_t prev_number;
    uint32_t prev_xor_lzc;
} gorilla_writer_t;

gorilla_writer_t gorilla_writer_init(uint32_t *buf, size_t n)
{
    return gorilla_writer_t {
        .buffer = buf,
        .entries = 0,
        .bsw = bit_stream_writer_init((n - 1) * bit_size<uint32_t>()),
        .prev_number = 0,
        .prev_xor_lzc = 0,
    };
}

static uint32_t *gorilla_writer_get_bit_buffer(const gorilla_writer_t *gw)
{
    return &gw->buffer[1];
}

bool gorilla_writer_write(gorilla_writer_t *gw, uint32_t number)
{
    uint32_t *bit_buffer = gorilla_writer_get_bit_buffer(gw);

    // this is the first number we are writing
    if (gw->entries == 0) {
        bool ok = bit_stream_writer_write(&gw->bsw, bit_buffer, number, bit_size<uint32_t>());

        if (ok) {
            gw->entries++;
            gw->prev_number = number;
        }

        return ok;
    }

    // write true/false based on whether we got the same number or not.
    if (number == gw->prev_number) {
        bool ok = bit_stream_writer_write(&gw->bsw, bit_buffer, static_cast<uint32_t>(1), 1);
        if (ok)
            gw->entries++;
        return ok;
    } else {
        bool ok = bit_stream_writer_write(&gw->bsw, bit_buffer,static_cast<uint32_t>(0), 1);
        if (!ok)
            return false;
    }

    uint32_t xor_value = gw->prev_number ^ number;
    uint32_t xor_lzc = (bit_size<uint32_t>() == 32) ? __builtin_clz(xor_value) : __builtin_clzll(xor_value);
    uint32_t is_xor_lzc_same = (xor_lzc == gw->prev_xor_lzc) ? 1 : 0;

    if (!bit_stream_writer_write(&gw->bsw, bit_buffer, is_xor_lzc_same, 1))
        return false;
    
    if (!is_xor_lzc_same) {
        if (!bit_stream_writer_write(&gw->bsw, bit_buffer, xor_lzc, (bit_size<uint32_t>() == 32) ? 5 : 6))
            return false;
    }

    // write the bits of the XOR'd value without the LZC prefix
    if (!bit_stream_writer_write(&gw->bsw, bit_buffer, xor_value, bit_size<uint32_t>() - xor_lzc))
        return false;

    gw->entries++;
    gw->prev_number = number;
    gw->prev_xor_lzc = xor_lzc;
    return true;
}

void gorilla_writer_flush(gorilla_writer_t *gw)
{
    uint32_t *bit_buffer = gorilla_writer_get_bit_buffer(gw);

    bit_stream_writer_flush(&gw->bsw, bit_buffer);
    __atomic_store_n(&gw->buffer[0], gw->entries, __ATOMIC_RELAXED);
}

uint32_t *gorilla_writer_data(const gorilla_writer_t *gw)
{
    return gw->buffer;
}

size_t gorilla_writer_capacity(const gorilla_writer_t *gw)
{
    return bit_stream_writer_capacity(&gw->bsw) / bit_size<uint32_t>();
}

size_t gorilla_writer_size(const gorilla_writer_t *gw)
{
    return (bit_stream_writer_position(&gw->bsw) + (bit_size<uint32_t>() - 1)) / bit_size<uint32_t>();
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
 * GorillaPageWriter
*/

typedef struct buffer_list {
    uint32_t *data;
    struct buffer_list *next;
} buffer_list_t;

typedef struct {
    gorilla_writer_t gw;
    buffer_list_t buffers;
} gorilla_page_writer_t;

gorilla_page_writer_t gorilla_page_writer_init(uint32_t *buffer, size_t n)
{
    gorilla_page_writer_t gpw;

    gpw.gw = gorilla_writer_init(buffer, n);
    gpw.buffers.data = buffer;
    gpw.buffers.next = NULL;

    return gpw;
}

void gorilla_page_writer_add_buffer(gorilla_page_writer_t *gpw, uint32_t *buffer, size_t n)
{
    gorilla_writer_flush(&gpw->gw);

    buffer_list_t *ptr = &gpw->buffers;
    while (ptr->next) {
        ptr = ptr->next;
    }

    ptr->next = new buffer_list_t;
    ptr->next->data = buffer; 
    ptr->next->next = NULL;

    gpw->gw = gorilla_writer_init(buffer, n);
}

bool gorilla_page_writer_write(gorilla_page_writer_t *gpw, uint32_t value)
{
    return gorilla_writer_write(&gpw->gw, value);
}

class GorillaPageWriter {
public:
    void init() {
        buffers.data = nullptr;
        buffers.next = nullptr;
    }

    bool write(uint32_t value) {
        if (gorilla_writer_write(&gw, value))
            return true;

        add_buffer();
        return gorilla_writer_write(&gw, value);
    }

private:
    void add_buffer() {
        if (buffers.data == NULL) {
            size_t n = 256;
            uint32_t *buffer = new uint32_t[n];
            
            buffers.data = buffer;
            buffers.next = NULL;

            gw = gorilla_writer_init(buffer, n);
            return;
        }

        gorilla_writer_flush(&gw);

        buffer_list_t *ptr = &buffers;
        while (ptr->next) {
            ptr = ptr->next;
        }

        size_t n = 256;
        uint32_t *buffer = new uint32_t[n];

        ptr->next = new buffer_list_t;
        ptr->next->data = buffer; 
        ptr->next->next = NULL;

        gw = gorilla_writer_init(buffer, n);
    }

private:
    gorilla_writer_t gw;
    buffer_list_t buffers;
};

/*
 * C API
*/

// gpw_t *gpw_new() {
//     GorillaPageWriter<uint32_t> *gpw = new GorillaPageWriter<uint32_t>();
//     gpw->init();
//     return reinterpret_cast<gpw_t *>(gpw);
// }

// void gpw_free(gpw_t *ptr) {
//     GorillaPageWriter<uint32_t> *gpw = reinterpret_cast<GorillaPageWriter<uint32_t> *>(ptr);
//     delete gpw;
// }

// void gpw_add_buffer(gpw_t *ptr) {
//     GorillaPageWriter<uint32_t> *gpw = reinterpret_cast<GorillaPageWriter<uint32_t> *>(ptr);
//     fprintf(stderr, "\nGVD: adding new gorilla buffer\n");
//     gpw->add_buffer();
// }

// bool gpw_add(gpw_t *ptr, uint32_t value) {
//     GorillaPageWriter<uint32_t> *gpw = reinterpret_cast<GorillaPageWriter<uint32_t> *>(ptr);
//     return gpw->write(value);
// }

// gorilla_writer_t *gorilla_writer_new(uint32_t *buffer, size_t n)
// {
//     GorillaWriter<uint32_t> *GW = new GorillaWriter<uint32_t>();
//     GW->init(buffer, n);
//     return reinterpret_cast<gorilla_writer_t *>(GW);
// }

// void gorilla_writer_free(gorilla_writer_t *writer) {
//     GorillaWriter<uint32_t> *GW = reinterpret_cast<GorillaWriter<uint32_t> *>(writer);
//     delete GW;
// }

// bool gorilla_writer_add(gorilla_writer *writer, uint32_t number) {
//     GorillaWriter<uint32_t> *GW = reinterpret_cast<GorillaWriter<uint32_t> *>(writer);
//     return GW->write(number);
// }

// void gorilla_writer_flush(gorilla_writer_t *writer) {
//     GorillaWriter<uint32_t> *GW = reinterpret_cast<GorillaWriter<uint32_t> *>(writer);
//     GW->flush();
// }

// gorilla_reader_t *gorilla_reader_alloc(const uint32_t *buffer)
// {
//     GorillaReader<uint32_t> *GW = new GorillaReader<uint32_t>();
//     GW->init(buffer);
//     return reinterpret_cast<gorilla_reader_t *>(GW);
// }

// void gorilla_reader_free(gorilla_reader_t *reader) {
//     GorillaReader<uint32_t> *GR = reinterpret_cast<GorillaReader<uint32_t> *>(reader);
//     delete GR;
// }

// size_t gorilla_reader_entries(gorilla_reader_t *reader) {
//     GorillaReader<uint32_t> *GR = reinterpret_cast<GorillaReader<uint32_t> *>(reader);
//     return GR->entries();
// }

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
