// SPDX-License-Identifier: GPL-3.0-or-later
//
// Golden vectors for the dbengine page codecs, produced by the C implementation itself:
//   - gorilla.cc from liblibnetdata.a (gorilla_writer_*, gorilla_buffer_patch, gorilla_reader_*);
//   - page.c compiled from the reference tree and linked with oracle-stubs.c (pgd_*, pgdc_*).
// Nothing here re-implements an encoder or decoder. The only replicated logic is the buffer growth of
// pgd_append_point (page.c:963-976) for the writer-level cases, whose inputs are arbitrary u32 values that
// pgd_append_point cannot produce (it packs doubles); the page-level cases call pgd_append_point directly and the
// generator checks that both paths give identical bytes.
//
// Usage: gen-gorilla OUT_DIR  -> OUT_DIR/{gorilla-writer,gorilla-page,gorilla-disk-load,raw-pages}.json

#include "database/engine/page.h"

#include <cinttypes>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <random>
#include <string>
#include <vector>

using std::string;
using std::vector;

static const size_t BUF_SIZE = RRDENG_GORILLA_32BIT_BUFFER_SIZE;          // 512
static const size_t BUF_SLOTS = RRDENG_GORILLA_32BIT_BUFFER_SLOTS;        // 128
static const uint32_t CAPACITY_BITS = (uint32_t)(BUF_SLOTS * 32 - sizeof(gorilla_header_t) * CHAR_BIT); // 3968
static const uint64_t CANONICAL_NEXT = 1; // stands in for the heap pointer C writes into non-last `next` fields

[[noreturn]] static void die(const char *msg) {
    fprintf(stderr, "gen-gorilla: %s\n", msg);
    exit(1);
}

// ---------------------------------------------------------------------------------------------------------------
// small helpers

static uint64_t rng_state;
static uint64_t rnd() { // splitmix64, as in netdata-agent-storage tests/oracle/gen-vectors.c
    uint64_t z = (rng_state += 0x9e3779b97f4a7c15ULL);
    z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
    z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
    return z ^ (z >> 31);
}

static uint64_t dbits(double v) { uint64_t b; memcpy(&b, &v, sizeof(b)); return b; }
static uint32_t fbits(float v) { uint32_t b; memcpy(&b, &v, sizeof(b)); return b; }

static string hexs(const uint8_t *p, size_t n) {
    static const char *d = "0123456789abcdef";
    string s;
    s.reserve(n * 2);
    for (size_t i = 0; i < n; i++) {
        s.push_back(d[p[i] >> 4]);
        s.push_back(d[p[i] & 15]);
    }
    return s;
}

static string u32s(uint32_t v) { char b[16]; snprintf(b, sizeof(b), "0x%08" PRIx32, v); return b; }
static string u32q(uint32_t v) { return "\"" + u32s(v) + "\""; }
static string u64s(uint64_t v) { char b[24]; snprintf(b, sizeof(b), "%016" PRIx64, v); return b; }
static string num(uint64_t v) { return std::to_string(v); }
static string qs(const string &s) {
    string o = "\"";
    for (char c : s) {
        if (c == '"' || c == '\\') { o.push_back('\\'); o.push_back(c); }
        else if ((unsigned char)c < 0x20) { char b[8]; snprintf(b, sizeof(b), "\\u%04x", c); o += b; }
        else o.push_back(c);
    }
    return o + "\"";
}
static string dbl(double v) { // exact bits plus a readable rendering (NaN/Inf are not JSON numbers)
    char b[64];
    snprintf(b, sizeof(b), "%.17g", v);
    return "{\"bits\":\"" + u64s(dbits(v)) + "\",\"repr\":" + qs(b) + "}";
}

template <typename T, typename F> static string arr(const vector<T> &v, F f) {
    string s = "[";
    for (size_t i = 0; i < v.size(); i++) {
        if (i) s += ",";
        s += f(v[i]);
    }
    return s + "]";
}

static uint8_t *zalloc(size_t n) { // 8-byte aligned, zeroed (the ARAL/extent buffers are pointer aligned)
    void *p = nullptr;
    if (posix_memalign(&p, 64, n ? n : 1)) die("posix_memalign");
    memset(p, 0, n ? n : 1);
    return (uint8_t *)p;
}

static uint64_t rd64(const uint8_t *p) { uint64_t v; memcpy(&v, p, 8); return v; }
static uint32_t rd32(const uint8_t *p) { uint32_t v; memcpy(&v, p, 4); return v; }
static void wr64(uint8_t *p, uint64_t v) { memcpy(p, &v, 8); }
static void wr32(uint8_t *p, uint32_t v) { memcpy(p, &v, 4); }

// ---------------------------------------------------------------------------------------------------------------
// writer-level chain: gorilla_writer_* with the buffer growth of pgd_append_point (page.c:963-976)

struct Failure {
    size_t index;        // index of the value whose write returned false
    size_t buffer;       // the buffer that was full
    uint32_t nbits_before;
    uint32_t nbits_after; // nbits of that buffer after the failed write (partial bits stay)
    uint32_t entries;
};

struct Chain {
    gorilla_writer_t gw;
    vector<gorilla_buffer_t *> bufs;
    vector<Failure> fails;
    size_t written = 0;

    Chain() {
        gorilla_buffer_t *b = (gorilla_buffer_t *)zalloc(BUF_SIZE);   // page.c:490-491
        gw = gorilla_writer_init(b, BUF_SLOTS);                      // page.c:494
        bufs.push_back(b);
    }
    ~Chain() {
        for (auto *b : bufs) free(b);
    }
    gorilla_buffer_t *last() { return bufs.back(); }

    void write(uint32_t v) {
        gorilla_buffer_t *l = last();
        uint32_t before = l->header.nbits;
        if (!gorilla_writer_write(&gw, v)) {                          // page.c:963
            fails.push_back(Failure{written, bufs.size() - 1, before, l->header.nbits, l->header.entries});
            gorilla_buffer_t *nb = (gorilla_buffer_t *)zalloc(BUF_SIZE); // page.c:965-966
            gorilla_writer_add_buffer(&gw, nb, BUF_SLOTS);            // page.c:968
            bufs.push_back(nb);
            if (!gorilla_writer_write(&gw, v))                        // page.c:972
                die("write into a fresh buffer failed");
        }
        written++;
    }
};

// Buffers of a serialized page: next masked to zero in the hex, reported as "nonzero"/"zero".
static string buffers_json(const uint8_t *page, size_t nbufs) {
    string s = "[";
    for (size_t b = 0; b < nbufs; b++) {
        const uint8_t *p = page + b * BUF_SIZE;
        uint8_t copy[BUF_SIZE];
        memcpy(copy, p, BUF_SIZE);
        uint64_t next = rd64(copy);
        wr64(copy, 0);
        if (b) s += ",";
        s += "{\"next\":\"" + string(next ? "nonzero" : "zero") + "\",\"entries\":" + num(rd32(p + 8)) +
             ",\"nbits\":" + num(rd32(p + 12)) + ",\"hex_next_masked\":\"" + hexs(copy, BUF_SIZE) + "\"}";
    }
    return s + "]";
}

// Replace the raw heap pointers in non-zero `next` fields with CANONICAL_NEXT (in place).
static void canonicalize_next(uint8_t *page, size_t nbufs) {
    for (size_t b = 0; b < nbufs; b++)
        if (rd64(page + b * BUF_SIZE))
            wr64(page + b * BUF_SIZE, CANONICAL_NEXT);
}

// Disk-direction decode with the real functions: gorilla_buffer_patch + gorilla_reader_init/read.
struct DiskDecode {
    bool patch_ok = false;
    uint32_t patch_entries = 0;
    vector<uint32_t> values;
};

static DiskDecode disk_decode(const uint8_t *page, size_t size, size_t max_reads) {
    DiskDecode d;
    uint8_t *copy = zalloc(size);
    memcpy(copy, page, size);
    d.patch_ok = gorilla_buffer_patch((gorilla_buffer_t *)copy, size / BUF_SIZE, &d.patch_entries);
    if (d.patch_ok) {
        gorilla_reader_t gr = gorilla_reader_init((gorilla_buffer_t *)copy);
        uint32_t v;
        while (d.values.size() < max_reads && gorilla_reader_read(&gr, &v))
            d.values.push_back(v);
    }
    free(copy);
    return d;
}

static string failures_json(const vector<Failure> &f) {
    return arr(f, [](const Failure &x) {
        return "{\"index\":" + num(x.index) + ",\"full_buffer\":" + num(x.buffer) + ",\"nbits_before\":" +
               num(x.nbits_before) + ",\"nbits_after\":" + num(x.nbits_after) + ",\"partial_bits\":" +
               num(x.nbits_after - x.nbits_before) + ",\"entries\":" + num(x.entries) + "}";
    });
}

static int writer_cases = 0;
static FILE *fw;

static void writer_case(const string &name, const string &desc, const vector<uint32_t> &in,
                        const string &extra_json = "") {
    Chain c;
    for (uint32_t v : in) c.write(v);

    size_t nbufs = c.bufs.size();
    size_t size = nbufs * BUF_SIZE;
    uint8_t *page = zalloc(size);
    if (!gorilla_writer_serialize(&c.gw, page, (uint32_t)size)) die("serialize failed");
    bool short_dst = gorilla_writer_serialize(&c.gw, zalloc(size), (uint32_t)(size - 1));

    // live reader (collector-side query path, page.c:1034-1035)
    vector<uint32_t> live;
    gorilla_reader_t gr = gorilla_writer_get_reader(&c.gw);
    uint32_t v;
    while (live.size() < in.size() + 16 && gorilla_reader_read(&gr, &v)) live.push_back(v);

    DiskDecode dd = disk_decode(page, size, in.size() + 16);
    if (live != in || dd.values != in || !dd.patch_ok || dd.patch_entries != in.size())
        die(("round trip mismatch in writer case " + name).c_str());

    string s = string(writer_cases++ ? ",\n" : "\n") + "{\"name\":" + qs(name) + ",\"description\":" + qs(desc) +
               ",\"count\":" + num(in.size()) + ",\"input_u32\":" + arr(in, u32q) +
               ",\"write_failures\":" + failures_json(c.fails) + ",\"num_buffers\":" + num(nbufs) +
               ",\"byte_length\":" + num(size) + ",\"writer_entries\":" + num(gorilla_writer_entries(&c.gw)) +
               ",\"actual_nbytes\":" + num(gorilla_writer_actual_nbytes(&c.gw)) +
               ",\"optimal_nbytes\":" + num(gorilla_writer_optimal_nbytes(&c.gw)) +
               ",\"serialize_into_size_minus_1\":" + (short_dst ? "true" : "false") +
               ",\"buffers\":" + buffers_json(page, nbufs) + ",\"decoded_live\":" + arr(live, u32q) +
               ",\"disk_patch_ok\":" + (dd.patch_ok ? "true" : "false") +
               ",\"disk_patch_entries\":" + num(dd.patch_entries) + ",\"decoded_disk\":" + arr(dd.values, u32q) +
               extra_json + "}";
    fputs(s.c_str(), fw);
    free(page);
}

// ---------------------------------------------------------------------------------------------------------------
// value generators

static uint32_t sn(double v, SN_FLAGS f = SN_DEFAULT_FLAGS) { return pack_storage_number(v, f); }

static uint32_t mask_all_below(uint32_t lzc) { // xor mask whose clz is lzc, all lower bits set
    return lzc >= 32 ? 0 : (0xFFFFFFFFu >> lzc);
}

// Adaptive worst case: every value's xor has a leading-zero count different from the previous one (1, 0, 1, ...),
// read from the real writer state, so each value pays 1+1+5+(32-lzc) bits; a new buffer resets prev_xor_lzc to 0.
static vector<uint32_t> adversarial(size_t n) {
    Chain c;
    vector<uint32_t> out;
    uint32_t v = 0x12345678;
    for (size_t i = 0; i < n; i++) {
        if (i) {
            uint32_t target = (c.gw.prev_xor_lzc == 0) ? 1 : 0;
            v = c.gw.prev_number ^ mask_all_below(target);
        }
        c.write(v);
        out.push_back(v);
    }
    return out;
}

// Fill one buffer to exactly `target` nbits with at most ~150 values: expensive alternating-lzc values first, then
// repeats of the last value (1 bit each).
static vector<uint32_t> fill_to(uint32_t target, Chain &c) {
    vector<uint32_t> out;
    uint32_t v = 0x3A5A5A5A;
    c.write(v);
    out.push_back(v);
    while (c.last()->header.nbits + 40 < target) {
        uint32_t t = (c.gw.prev_xor_lzc == 0) ? 1 : 0;
        v = c.gw.prev_number ^ mask_all_below(t);
        c.write(v);
        out.push_back(v);
    }
    while (c.last()->header.nbits < target) {
        c.write(v);
        out.push_back(v);
    }
    if (c.last()->header.nbits != target || c.bufs.size() != 1) die("fill_to missed its target");
    return out;
}

struct Breaker {
    const char *name;
    const char *check;         // which capacity check of gorilla_writer_write fails
    uint32_t nbits_before;
    int kind;                  // 0 = same value, 1 = different value with xor lzc == prev, 2 = lzc != prev
    uint32_t lzc;              // for kind 2
    uint32_t expect_partial;
};

// ---------------------------------------------------------------------------------------------------------------
// page-level (real page.c): pgd_create / pgd_append_point / pgd_disk_footprint / pgd_copy_to_extent /
// pgd_create_from_disk_data / pgdc_reset / pgdc_get_next_point

struct PointIn {
    double n;
    SN_FLAGS flags;
};

static string point_json(bool ok, const STORAGE_POINT &sp) {
    // [ok, sum_bits, min_bits, max_bits, count, anomaly_count, flags]
    return "[" + string(ok ? "true" : "false") + ",\"" + u64s(dbits(sp.sum)) + "\",\"" + u64s(dbits(sp.min)) +
           "\",\"" + u64s(dbits(sp.max)) + "\"," + num(sp.count) + "," + num(sp.anomaly_count) + "," +
           num(sp.flags) + "]";
}

static string cursor_json(PGD *pg, uint32_t from, uint32_t upto) {
    PGDC c;
    pgdc_reset(&c, pg, from);
    string s = "[";
    for (uint32_t pos = from; pos < upto; pos++) {
        STORAGE_POINT sp = {};
        bool ok = pgdc_get_next_point(&c, pos, &sp);
        if (pos != from) s += ",";
        s += point_json(ok, sp);
    }
    return s + "]";
}

static int page_cases = 0;
static FILE *fp;

static void page_case(const string &name, const string &desc, uint32_t slots, const vector<PointIn> &in,
                      const string &extra_json = "") {
    PGD *pg = pgd_create(RRDENG_PAGE_TYPE_GORILLA_32BIT, slots);
    vector<size_t> grew;
    vector<uint32_t> packed;
    for (size_t i = 0; i < in.size(); i++) {
        size_t r = pgd_append_point(pg, (usec_t)(i + 1) * USEC_PER_SEC, in[i].n, 0, 0, 1, 0, in[i].flags, (uint32_t)i);
        if (r) grew.push_back(i);
        packed.push_back(pack_storage_number(in[i].n, in[i].flags));
    }
    bool is_empty = pgd_is_empty(pg);
    uint32_t used = pgd_slots_used(pg);
    string collector_points = cursor_json(pg, 0, used + 1);
    uint32_t size = pgd_disk_footprint(pg);

    string s = string(page_cases++ ? ",\n" : "\n") + "{\"name\":" + qs(name) + ",\"description\":" + qs(desc) +
               ",\"slots\":" + num(slots) + ",\"count\":" + num(in.size()) +
               ",\"input_values\":" + arr(in, [](const PointIn &p) { return dbl(p.n); }) +
               ",\"input_flags\":" + arr(in, [](const PointIn &p) { return num(p.flags); }) +
               ",\"packed_u32\":" + arr(packed, u32q) +
               ",\"append_returned_buffer_size_at\":" + arr(grew, [](size_t i) { return num(i); }) +
               ",\"pgd_is_empty\":" + (is_empty ? "true" : "false") + ",\"pgd_slots_used\":" + num(used) +
               ",\"collector_points_from_0\":" + collector_points + ",\"disk_footprint\":" + num(size);

    if (size) {
        uint8_t *page = zalloc(size);
        for (uint32_t i = 0; i < size; i++) page[i] = 0xAA; // pgd_copy_to_extent must overwrite every byte
        pgd_copy_to_extent(pg, page, size);

        // the page bytes must equal the writer-level encoding of the packed values
        Chain c;
        for (uint32_t v : packed) c.write(v);
        uint8_t *wpage = zalloc(size);
        if (c.bufs.size() * BUF_SIZE != size || !gorilla_writer_serialize(&c.gw, wpage, size)) die("writer size");
        uint8_t *a = zalloc(size), *b = zalloc(size);
        memcpy(a, page, size);
        memcpy(b, wpage, size);
        canonicalize_next(a, size / BUF_SIZE);
        canonicalize_next(b, size / BUF_SIZE);
        if (memcmp(a, b, size)) die(("page bytes differ from writer bytes in " + name).c_str());

        PGD *d = pgd_create_from_disk_data(RRDENG_PAGE_TYPE_GORILLA_32BIT, page, size);
        s += ",\"num_buffers\":" + num(size / BUF_SIZE) + ",\"buffers\":" + buffers_json(page, size / BUF_SIZE) +
             ",\"matches_writer_encoding\":true";
        if (d == PGD_EMPTY)
            s += ",\"from_disk\":\"PGD_EMPTY\"";
        else {
            uint32_t du = pgd_slots_used(d);
            s += ",\"from_disk\":{\"pgd_slots_used\":" + num(du) + ",\"pgd_capacity\":" + num(pgd_capacity(d)) +
                 ",\"pgd_is_empty\":" + (pgd_is_empty(d) ? "true" : "false") +
                 ",\"points_from_0\":" + cursor_json(d, 0, du + 1) + "}";
            pgd_free(d);
        }
        free(page); free(wpage); free(a); free(b);
    }
    s += extra_json + "}";
    fputs(s.c_str(), fp);
    pgd_free(pg);
}

// ---------------------------------------------------------------------------------------------------------------
// disk-load cases: pgd_create_from_disk_data (page.c:522-576) on crafted bytes, then the cursor

static int disk_cases = 0;
static FILE *fd;

static vector<uint8_t> serialized(const vector<uint32_t> &in, size_t *nbufs = nullptr) {
    Chain c;
    for (uint32_t v : in) c.write(v);
    size_t size = c.bufs.size() * BUF_SIZE;
    vector<uint8_t> page(size);
    if (!gorilla_writer_serialize(&c.gw, page.data(), (uint32_t)size)) die("serialize");
    canonicalize_next(page.data(), c.bufs.size());
    if (nbufs) *nbufs = c.bufs.size();
    return page;
}

static void disk_case(const string &name, const string &desc, const vector<uint8_t> &bytes, uint32_t size,
                      uint32_t read_upto, const string &extra_json = "") {
    uint8_t *base = zalloc(bytes.size() + 8);
    memcpy(base, bytes.data(), bytes.size());
    PGD *d = pgd_create_from_disk_data(RRDENG_PAGE_TYPE_GORILLA_32BIT, base, size);
    string s = string(disk_cases++ ? ",\n" : "\n") + "{\"name\":" + qs(name) + ",\"description\":" + qs(desc) +
               ",\"input_hex\":\"" + hexs(bytes.data(), bytes.size()) + "\",\"size\":" + num(size);
    if (d == PGD_EMPTY)
        s += ",\"result\":\"PGD_EMPTY\"";
    else {
        uint32_t du = pgd_slots_used(d);
        s += ",\"result\":{\"pgd_slots_used\":" + num(du) + ",\"pgd_capacity\":" + num(pgd_capacity(d)) +
             ",\"points_from_0\":" + cursor_json(d, 0, read_upto) + "}";
        pgd_free(d);
    }
    s += extra_json + "}";
    fputs(s.c_str(), fd);
    free(base);
}

// ---------------------------------------------------------------------------------------------------------------
// raw pages: ARRAY_32BIT and ARRAY_TIER1 through the real page.c

static int raw_cases = 0;
static FILE *fr;

struct Tier1In {
    double sum, min, max;
    uint16_t count, anomaly_count;
};

static string raw_points(PGD *pg, uint32_t upto) {
    return cursor_json(pg, 0, upto);
}

static void raw_array32_case(const string &name, const string &desc, uint32_t slots, const vector<PointIn> &in) {
    PGD *pg = pgd_create(RRDENG_PAGE_TYPE_ARRAY_32BIT, slots);
    for (size_t i = 0; i < in.size(); i++)
        pgd_append_point(pg, (usec_t)(i + 1) * USEC_PER_SEC, in[i].n, 0, 0, 1, 0, in[i].flags, (uint32_t)i);
    bool is_empty = pgd_is_empty(pg);
    uint32_t used = pgd_slots_used(pg);
    uint32_t size = pgd_disk_footprint(pg);
    uint8_t *page = zalloc(size);
    if (size) pgd_copy_to_extent(pg, page, size);
    PGD *d = size ? pgd_create_from_disk_data(RRDENG_PAGE_TYPE_ARRAY_32BIT, page, size) : PGD_EMPTY;
    string s = string(raw_cases++ ? ",\n" : "\n") + "{\"name\":" + qs(name) + ",\"page_type\":\"ARRAY_32BIT\"" +
               ",\"page_type_id\":0,\"description\":" + qs(desc) + ",\"slots\":" + num(slots) +
               ",\"input_values\":" + arr(in, [](const PointIn &p) { return dbl(p.n); }) +
               ",\"input_flags\":" + arr(in, [](const PointIn &p) { return num(p.flags); }) +
               ",\"pgd_is_empty\":" + (is_empty ? "true" : "false") + ",\"pgd_slots_used\":" + num(used) +
               ",\"disk_footprint\":" + num(size) + ",\"page_hex\":\"" + hexs(page, size) + "\"";
    if (d == PGD_EMPTY)
        s += ",\"from_disk\":\"PGD_EMPTY\"";
    else {
        s += ",\"from_disk\":{\"pgd_slots_used\":" + num(pgd_slots_used(d)) +
             ",\"points_from_0\":" + raw_points(d, pgd_slots_used(d) + 1) + "}";
        pgd_free(d);
    }
    s += "}";
    fputs(s.c_str(), fr);
    free(page);
    pgd_free(pg);
}

static void raw_tier1_case(const string &name, const string &desc, uint32_t slots, const vector<Tier1In> &in) {
    PGD *pg = pgd_create(RRDENG_PAGE_TYPE_ARRAY_TIER1, slots);
    for (size_t i = 0; i < in.size(); i++)
        pgd_append_point(pg, (usec_t)(i + 1) * USEC_PER_SEC, in[i].sum, in[i].min, in[i].max, in[i].count,
                         in[i].anomaly_count, SN_DEFAULT_FLAGS, (uint32_t)i);
    bool is_empty = pgd_is_empty(pg);
    uint32_t used = pgd_slots_used(pg);
    uint32_t size = pgd_disk_footprint(pg);
    uint8_t *page = zalloc(size);
    if (size) pgd_copy_to_extent(pg, page, size);
    PGD *d = size ? pgd_create_from_disk_data(RRDENG_PAGE_TYPE_ARRAY_TIER1, page, size) : PGD_EMPTY;

    string recs = "[";
    for (uint32_t i = 0; i < used; i++) {
        const uint8_t *p = page + i * sizeof(storage_number_tier1_t);
        float f[3];
        memcpy(f, p, 12);
        if (i) recs += ",";
        recs += "{\"sum_f32\":\"" + u32s(fbits(f[0])) + "\",\"min_f32\":\"" + u32s(fbits(f[1])) +
                "\",\"max_f32\":\"" + u32s(fbits(f[2])) + "\",\"count\":" + num(p[12] | (p[13] << 8)) +
                ",\"anomaly_count\":" + num(p[14] | (p[15] << 8)) + ",\"hex\":\"" + hexs(p, 16) + "\"}";
    }
    recs += "]";

    string s = string(raw_cases++ ? ",\n" : "\n") + "{\"name\":" + qs(name) + ",\"page_type\":\"ARRAY_TIER1\"" +
               ",\"page_type_id\":1,\"description\":" + qs(desc) + ",\"slots\":" + num(slots) +
               ",\"input\":" + arr(in, [](const Tier1In &t) {
                   return "{\"sum\":" + dbl(t.sum) + ",\"min\":" + dbl(t.min) + ",\"max\":" + dbl(t.max) +
                          ",\"count\":" + num(t.count) + ",\"anomaly_count\":" + num(t.anomaly_count) + "}";
               }) +
               ",\"pgd_is_empty\":" + (is_empty ? "true" : "false") + ",\"pgd_slots_used\":" + num(used) +
               ",\"disk_footprint\":" + num(size) + ",\"records\":" + recs + ",\"page_hex\":\"" +
               hexs(page, size) + "\"";
    if (d == PGD_EMPTY)
        s += ",\"from_disk\":\"PGD_EMPTY\"";
    else {
        s += ",\"from_disk\":{\"pgd_slots_used\":" + num(pgd_slots_used(d)) +
             ",\"points_from_0\":" + raw_points(d, pgd_slots_used(d) + 1) + "}";
        pgd_free(d);
    }
    s += "}";
    fputs(s.c_str(), fr);
    free(page);
    pgd_free(pg);
}

static void raw_disk_case(const string &name, const string &desc, uint8_t type, const vector<uint8_t> &bytes,
                          uint32_t size) {
    uint8_t *base = zalloc(bytes.size() + 8);
    memcpy(base, bytes.data(), bytes.size());
    PGD *d = pgd_create_from_disk_data(type, base, size);
    string s = string(raw_cases++ ? ",\n" : "\n") + "{\"name\":" + qs(name) + ",\"page_type\":\"" +
               (type == RRDENG_PAGE_TYPE_ARRAY_32BIT ? "ARRAY_32BIT" : "ARRAY_TIER1") +
               "\",\"page_type_id\":" + num(type) + ",\"description\":" + qs(desc) + ",\"disk_input_hex\":\"" +
               hexs(bytes.data(), bytes.size()) + "\",\"size\":" + num(size);
    if (d == PGD_EMPTY)
        s += ",\"from_disk\":\"PGD_EMPTY\"";
    else {
        s += ",\"from_disk\":{\"pgd_slots_used\":" + num(pgd_slots_used(d)) +
             ",\"points_from_0\":" + raw_points(d, pgd_slots_used(d) + 1) + "}";
        pgd_free(d);
    }
    s += "}";
    fputs(s.c_str(), fr);
    free(base);
}

// ---------------------------------------------------------------------------------------------------------------

static FILE *open_json(const char *dir, const char *file, const char *what) {
    char path[4096];
    snprintf(path, sizeof(path), "%s/%s", dir, file);
    FILE *f = fopen(path, "w");
    if (!f) die(path);
    fprintf(f,
            "{\"generator\":\"gen/gen-gorilla.cc (run by gen/gen.sh); do not edit\",\"reference\":\"netdata/netdata "
            "@ 1e97a0fc9e\",\"contents\":%s,\n\"constants\":{\"buffer_size\":%zu,\"buffer_slots\":%zu,"
            "\"sizeof_gorilla_header_t\":%zu,\"capacity_bits\":%u,\"canonical_next\":%" PRIu64
            ",\"sizeof_storage_number_tier1_t\":%zu,\"SN_EMPTY_SLOT\":\"0x%08x\",\"SN_DEFAULT_FLAGS\":\"0x%08x\"},\n"
            "\"cases\":[",
            qs(what).c_str(), BUF_SIZE, BUF_SLOTS, sizeof(gorilla_header_t), CAPACITY_BITS, CANONICAL_NEXT,
            sizeof(storage_number_tier1_t), SN_EMPTY_SLOT, SN_DEFAULT_FLAGS);
    return f;
}

static void close_json(FILE *f) {
    fputs("\n]}\n", f);
    fclose(f);
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s OUT_DIR\n", argv[0]);
        return 1;
    }
    if (sizeof(gorilla_header_t) != 16 || CAPACITY_BITS != 3968) die("expected a 64-bit build (16-byte header)");
    pgd_init_arals();

    // =========================================================================== writer-level
    fw = open_json(argv[1], "gorilla-writer.json",
                   "gorilla_writer_* over u32 inputs, growth as pgd_append_point (page.c:963-976); serialized "
                   "buffers, then decoded by the live reader and by gorilla_buffer_patch + gorilla_reader_*");

    {
        vector<uint32_t> in = {sn(5), sn(5), sn(6)};
        Chain c;
        for (uint32_t v : in) c.write(v);
        uint8_t page[BUF_SIZE];
        gorilla_writer_serialize(&c.gw, page, BUF_SIZE);
        const char *spec_first28 = "0000000000000000030000003d000000404b4c3159c0c61700000000";
        bool confirmed = in[0] == 0x314C4B40 && in[2] == 0x315B8D80 && c.last()->header.entries == 3 &&
                         c.last()->header.nbits == 61 && rd32(page + 16) == 0x314C4B40 &&
                         rd32(page + 20) == 0x17C6C059 && hexs(page, 28) == spec_first28;
        writer_case("spec_example_5_5_6",
                    "spec-dbengine.md B1 6.3 worked example: 5, 5, 6 packed with SN_DEFAULT_FLAGS", in,
                    string(",\"spec_claim\":{\"packed\":[\"0x314c4b40\",\"0x314c4b40\",\"0x315b8d80\"],\"entries\":3,"
                           "\"nbits\":61,\"data0\":\"0x314c4b40\",\"data1\":\"0x17c6c059\",\"first_28_bytes\":\"") +
                        spec_first28 + "\"},\"spec_claim_confirmed\":" + (confirmed ? "true" : "false"));
    }
    writer_case("empty", "no value written: one zeroed buffer (a collector page with used == 0 is never flushed, "
                         "pgd_disk_footprint returns 0, page.c:822-823)", {});
    writer_case("single_value_666", "one value: pack(666, SN_DEFAULT_FLAGS)", {sn(666)});
    writer_case("zero_first_value", "raw 0 first (equal to the initial prev_number 0 but still written raw), then "
                                    "0 again (same bit), then 1",
                {0x00000000, 0x00000000, 0x00000001});
    writer_case("lzc_zero_equals_initial_prev_lzc",
                "second xor has lzc 0, equal to the initial prev_xor_lzc 0, so it is coded as 'same lzc' without the "
                "5-bit field",
                {0x00000000, 0x80000000, 0x00000000, 0x40000000});
    writer_case("lzc_31_single_bit_xor", "values differing only in bit 0 (xor lzc 31, 1 payload bit)",
                {0x39989680, 0x39989681, 0x39989680, 0x39989681, 0x39989680});
    writer_case("all_equal_1024", "1024 x pack(42, SN_DEFAULT_FLAGS)", vector<uint32_t>(1024, sn(42)));
    {
        vector<uint32_t> in;
        struct { double v; size_t n; } runs[] = {{1, 100}, {2, 200}, {3, 1}, {4, 300}, {1, 423}};
        for (auto &r : runs) for (size_t i = 0; i < r.n; i++) in.push_back(sn(r.v));
        writer_case("constant_runs", "runs of pack(v): 1 x100, 2 x200, 3 x1, 4 x300, 1 x423", in);
    }
    {
        vector<uint32_t> raw, snv;
        for (uint32_t i = 0; i < 1024; i++) { raw.push_back(i); snv.push_back(sn(i)); }
        writer_case("monotonic_raw_u32_0_1023", "raw u32 0..1023", raw);
        writer_case("monotonic_sn_0_1023", "pack(i, SN_DEFAULT_FLAGS) for i = 0..1023 (the page_test.cc input)", snv);
    }
    {
        vector<uint32_t> in;
        for (size_t i = 0; i < 1024; i++) in.push_back(sn(i % 2 ? 2.0 : 1.0));
        writer_case("alternating_1_2", "pack(1), pack(2) alternating, 1024 values", in);
    }
    vector<uint32_t> rand24;
    {
        rng_state = 0x676f72696c6c6131ULL;
        for (size_t i = 0; i < 1024; i++) rand24.push_back(0x01000000u | (uint32_t)(rnd() & 0xFFFFFF));
        writer_case("random_mantissa24_1024",
                    "0x01000000 | random 24-bit mantissa (NOT_ANOMALOUS, m=0, divide: value == mantissa); splitmix64 "
                    "seed 0x676f72696c6c6131, mantissa = rnd() & 0xFFFFFF",
                    rand24);
    }
    {
        rng_state = 0x676f72696c6c6132ULL;
        vector<uint32_t> in;
        for (size_t i = 0; i < 1024; i++) in.push_back((uint32_t)rnd());
        writer_case("random_u32_1024", "full-range random u32; splitmix64 seed 0x676f72696c6c6132, value = (u32)rnd()",
                    in);
    }
    writer_case("adversarial_lzc_alternating_1024",
                "adaptive worst case: each xor lzc differs from the previous (1,0,1,...; reset to 0 per buffer), "
                "xor = prev ^ (0xFFFFFFFF >> target_lzc); first value 0x12345678",
                adversarial(1024),
                ",\"note\":\"1024 is the tier-0 page maximum (tier_page_size[0] 4096 / 4, rrdengineapi.c:31,461-462)\"");
    writer_case("adversarial_lzc_alternating_1100",
                "same generator continued to 1100 values: 11 buffers = 5632 B, above the 5120 B gorilla limit of "
                "validate_page (pdc.c:921-931), so such a page would be rejected on load",
                adversarial(1100));
    {
        Chain c;
        for (uint32_t v : rand24) c.write(v);
        if (c.fails.size() < 2) die("rand24 needs at least 3 buffers");
        size_t f1 = c.fails[0].index, f2 = c.fails[1].index;
        writer_case("boundary_second_buffer", "prefix of random_mantissa24_1024 ending with the first value that "
                                               "did not fit: the second buffer holds exactly one value",
                    vector<uint32_t>(rand24.begin(), rand24.begin() + f1 + 1));
        writer_case("boundary_third_buffer", "prefix of random_mantissa24_1024 ending with the value that opened "
                                              "the third buffer",
                    vector<uint32_t>(rand24.begin(), rand24.begin() + f2 + 1));
    }
    {
        const Breaker br[] = {
            {"fail_same_value_check", "same value: nbits + 1 >= capacity (gorilla.cc:176)", 3967, 0, 0, 0},
            {"fail_control_bit_check", "different value, control bit: nbits + 1 >= capacity (gorilla.cc:185)", 3967, 1, 0, 0},
            {"fail_lzc_flag_check", "lzc flag: nbits + 1 >= capacity after the control bit (gorilla.cc:194)", 3966, 1, 0, 1},
            {"fail_lzc_bits_check", "5-bit lzc: nbits + 5 >= capacity (gorilla.cc:201)", 3963, 2, 23, 2},
            {"fail_xor_bits_same_lzc", "xor payload after 'same lzc': nbits + (32 - lzc) >= capacity (gorilla.cc:208)", 3940, 1, 0, 2},
            {"fail_xor_bits_new_lzc", "xor payload after a new 5-bit lzc: nbits + (32 - lzc) >= capacity (gorilla.cc:208)", 3950, 2, 15, 7},
        };
        for (const Breaker &b : br) {
            Chain c;
            vector<uint32_t> in = fill_to(b.nbits_before, c);
            uint32_t prev = c.gw.prev_number, plzc = c.gw.prev_xor_lzc, x;
            if (b.kind == 0) x = prev;
            else if (b.kind == 1) x = prev ^ mask_all_below(plzc);
            else {
                uint32_t l = (b.lzc == plzc) ? b.lzc + 1 : b.lzc;
                x = prev ^ ((0x80000000u >> l) | 1u);
            }
            in.push_back(x);
            // after the retry the new buffer starts from prev = x, prev_xor_lzc = 0
            in.push_back(x);                    // same value: 1 bit
            in.push_back(x ^ 0x80000001u);      // xor lzc 0 == reset prev_xor_lzc: coded as 'same lzc'
            in.push_back(in.back() ^ 0x00000100u); // xor lzc 23: new 5-bit lzc
            Chain v;
            for (uint32_t y : in) v.write(y);
            if (v.fails.size() != 1 || v.fails[0].index != in.size() - 4 || v.fails[0].nbits_before != b.nbits_before ||
                v.fails[0].nbits_after - v.fails[0].nbits_before != b.expect_partial)
                die(("breaker did not fail where intended: " + string(b.name)).c_str());
            writer_case(b.name, string("fill buffer 0 to nbits ") + std::to_string(b.nbits_before) +
                                    " with alternating-lzc values then repeats, then a value that fails at: " +
                                    b.check + "; the failed write leaves " + std::to_string(b.expect_partial) +
                                    " partial bit(s) in buffer 0; the value is rewritten raw in buffer 1, followed by "
                                    "3 values showing the per-buffer state reset",
                        in, ",\"failing_check\":" + qs(b.check));
        }
    }
    {
        vector<uint32_t> in;
        in.push_back(sn(1));
        for (int i = 0; i < 100; i++) in.push_back(SN_EMPTY_SLOT);
        in.push_back(sn(2));
        in.push_back(sn(3));
        for (int i = 0; i < 300; i++) in.push_back(SN_EMPTY_SLOT);
        in.push_back(sn(4));
        while (in.size() < 1024) in.push_back(SN_EMPTY_SLOT);
        writer_case("empty_slot_runs", "SN_EMPTY_SLOT (0x04000000) gaps: pack(1), EMPTY x100, pack(2), pack(3), "
                                       "EMPTY x300, pack(4), EMPTY to 1024",
                    in);
        writer_case("all_empty_slot_1024", "1024 x SN_EMPTY_SLOT", vector<uint32_t>(1024, SN_EMPTY_SLOT));
    }
    {
        const double vals[] = {1.0, -3.5, 0.0, 2.5, 100.0, 123456789.0, 1e-9, -0.1};
        const SN_FLAGS fl[] = {SN_FLAG_NONE, SN_FLAG_NOT_ANOMALOUS, SN_FLAG_RESET,
                               SN_FLAG_NOT_ANOMALOUS | SN_FLAG_RESET};
        vector<uint32_t> in;
        for (size_t i = 0; i < 32; i++) in.push_back(sn(vals[i % 8], fl[(i / 8) % 4]));
        writer_case("flags_mix_32", "pack(v, f) for v in {1,-3.5,0,2.5,100,123456789,1e-9,-0.1} x flags {NONE, "
                                    "NOT_ANOMALOUS, RESET, NOT_ANOMALOUS|RESET}",
                    in);
    }
    close_json(fw);

    // =========================================================================== page-level
    fp = open_json(argv[1], "gorilla-page.json",
                   "real page.c GORILLA_32BIT pages: pgd_append_point -> pgd_disk_footprint -> pgd_copy_to_extent -> "
                   "pgd_create_from_disk_data -> pgdc_reset/pgdc_get_next_point");

    page_case("spec_example_5_5_6", "5, 5, 6 with SN_DEFAULT_FLAGS", 1024,
              {{5, SN_DEFAULT_FLAGS}, {5, SN_DEFAULT_FLAGS}, {6, SN_DEFAULT_FLAGS}});
    page_case("copy_to_extent_666", "page_test.cc CopyToExtent input (value 666; count 1 and anomaly_count 0 are "
                                    "ignored by gorilla pages)",
              1024, {{666, SN_DEFAULT_FLAGS}});
    {
        vector<PointIn> in;
        for (int i = 0; i < 1024; i++) in.push_back({(double)i, SN_DEFAULT_FLAGS});
        page_case("sequence_0_1023", "page_test.cc Create/Cursor*/Roundtrip input with the tier-0 maximum of 1024 "
                                     "slots: n = i, SN_DEFAULT_FLAGS",
                  1024, in);
        in.resize(16);
        page_case("sequence_0_15", "page_test.cc RejectCorruptGorillaDiskChain input: 16 slots, n = i", 16, in);
    }
    page_case("empty_page", "pgd_create then no append: used 0, disk footprint 0 (not flushed)", 1024, {});
    {
        vector<PointIn> in;
        for (int i = 0; i < 64; i++) {
            bool gap = (i >= 3 && i < 20) || (i >= 40 && i < 60);
            in.push_back({gap ? NAN : (double)(i * 10), SN_DEFAULT_FLAGS});
        }
        page_case("nan_gaps_64", "NaN packs to SN_EMPTY_SLOT; slots 3-19 and 40-59 are gaps", 1024, in);
        vector<PointIn> all(16, PointIn{NAN, SN_DEFAULT_FLAGS});
        page_case("all_nan_16", "all NaN: PAGE_OPTION_ALL_VALUES_EMPTY stays set (page.c:960-961), pgd_is_empty "
                                "is true, yet pgd_disk_footprint still reports 512",
                  16, all);
    }
    {
        vector<PointIn> in = {{1, SN_FLAG_NONE},
                              {1, SN_FLAG_NOT_ANOMALOUS},
                              {2.5, SN_FLAG_RESET},
                              {2.5, SN_FLAG_NOT_ANOMALOUS | SN_FLAG_RESET},
                              {-3.5, SN_DEFAULT_FLAGS},
                              {0, SN_FLAG_NONE},
                              {0, SN_DEFAULT_FLAGS},
                              {INFINITY, SN_DEFAULT_FLAGS},
                              {1e30, SN_DEFAULT_FLAGS},
                              {100, SN_DEFAULT_FLAGS},
                              {NAN, SN_FLAG_NONE},
                              {0.1, SN_FLAG_NONE}};
        page_case("flags_and_edges", "anomalous (flags 0), RESET, zero, +Inf (-> empty), 1e30 (saturates), 100 "
                                     "(unpacks to 100.00000000000001), NaN",
                  1024, in);
    }
    {
        // page_test.cc DiskFootprint with the seed fixed to 5489 (the test itself seeds from std::random_device)
        std::mt19937 gen(5489u);
        std::uniform_int_distribution<uint32_t> distr(std::numeric_limits<uint32_t>::min(),
                                                      std::numeric_limits<uint32_t>::max());
        vector<PointIn> a, b;
        for (int i = 0; i < 16; i++) a.push_back({(double)distr(gen), SN_DEFAULT_FLAGS});
        for (int i = 0; i < 192; i++) b.push_back({(double)distr(gen), SN_DEFAULT_FLAGS});
        page_case("disk_footprint_random_16_seed5489", "page_test.cc DiskFootprint first page: 16 values "
                                                        "(double)uniform_int_distribution<uint32_t>(0, UINT32_MAX) "
                                                        "over std::mt19937(5489)",
                  1024, a);
        page_case("disk_footprint_random_192_seed5489", "page_test.cc DiskFootprint second page: the next 192 values "
                                                         "of the same generator",
                  1024, b);
    }
    close_json(fp);

    // =========================================================================== disk load (corrupt / edge)
    fd = open_json(argv[1], "gorilla-disk-load.json",
                   "pgd_create_from_disk_data(GORILLA_32BIT, bytes, size) (page.c:522-576, gorilla_buffer_patch "
                   "gorilla.cc:285-315) on crafted inputs, then pgdc points; non-zero next fields are canonical 1");
    {
        size_t nb16;
        vector<uint8_t> p16 = serialized({sn(0), sn(1), sn(2), sn(3), sn(4), sn(5), sn(6), sn(7), sn(8), sn(9),
                                          sn(10), sn(11), sn(12), sn(13), sn(14), sn(15)}, &nb16);
        disk_case("valid_sequence_0_15", "baseline: the sequence_0_15 page as flushed", p16, 512, 17);
        vector<uint8_t> x = p16;
        wr64(x.data(), CANONICAL_NEXT);
        disk_case("reject_last_buffer_next_nonzero", "page_test.cc RejectCorruptGorillaDiskChain: the only (last) "
                                                     "buffer has next != 0 -> buffers == nbuffers -> false "
                                                     "(gorilla.cc:293-295) -> PGD_EMPTY (page.c:546-552)",
                  x, 512, 17);

        vector<uint8_t> one = serialized({sn(666)});
        x = one;
        wr32(x.data() + 12, 4096);
        disk_case("reject_nbits_4096", "page_test.cc RejectCorruptGorillaDiskNbits: nbits = 512*8 >= 3968 "
                                       "(gorilla.cc:36-38,290-291) -> PGD_EMPTY",
                  x, 512, 2);
        x = one;
        wr32(x.data() + 12, 3968);
        disk_case("reject_nbits_3968", "nbits == capacity is already invalid (strict <)", x, 512, 2);
        x = one;
        wr32(x.data() + 12, 3967);
        disk_case("accept_nbits_3967", "nbits = 3967 (largest valid); entries 1 -> reads one value", x, 512, 2);
        x = one;
        wr32(x.data() + 8, 2);
        disk_case("entries_plus_one", "page_test.cc StopCorruptGorillaDiskEntriesAtEncodedBits: entries 2, nbits 32 "
                                      "-> used 2; point 0 = 666, point 1 fails at the bit bound (gorilla.cc:355-356)",
                  x, 512, 3);
        x = one;
        wr32(x.data() + 8, 0);
        disk_case("entries_zero", "entries 0 on a page holding one value: used 0, no point is returned", x, 512, 2);
        x = one;
        wr32(x.data() + 8, 65537);
        disk_case("entries_u16_truncation_65537", "sum of entries is u32 but pgd->used is u16 (page.c:32,554): "
                                                  "65537 -> used 1",
                  x, 512, 2);
        x = one;
        wr32(x.data() + 8, 65536);
        disk_case("entries_u16_truncation_65536", "65536 -> used 0", x, 512, 2);

        disk_case("reject_size_0", "size 0 -> PGD_EMPTY (page.c:524-525)", one, 0, 1);
        disk_case("reject_size_3", "size < page_type_size[GORILLA] = 4 -> PGD_EMPTY (page.c:524-525)", one, 3, 1);
        vector<uint8_t> pad = one;
        pad.resize(516, 0);
        disk_case("size_516_not_multiple_of_512", "size % 512 != 0 is only an internal_fatal (page.c:537, a no-op "
                                                  "without NETDATA_INTERNAL_CHECKS): nbuffers = 516/512 = 1, accepted",
                  pad, 516, 2);
        vector<uint8_t> half(one.begin(), one.begin() + 256);
        disk_case("size_256_one_value_next_zero", "size 256 < 512: nbuffers = 0, which the chain bound "
                                                  "`buffers == nbuffers` (gorilla.cc:294) never matches; with next = 0 "
                                                  "the page is accepted and the value decodes (all bits inside 256 B)",
                  half, 256, 2,
                  ",\"not_executed_hazard\":\"with next != 0 and size < 512 the walk steps past the copied bytes "
                  "(buffers starts at 1, nbuffers is 0) and reads/writes headers out of bounds; with nbits > "
                  "(size-16)*8 the reader reads past the copy. Derived from gorilla.cc:285-315 and page.c:540-548; "
                  "not run because it is out-of-bounds access.\"");
    }
    {
        size_t nb;
        vector<uint32_t> in2(rand24.begin(), rand24.begin() + 200);
        vector<uint8_t> two = serialized(in2, &nb);
        if (nb != 2) die("expected 2 buffers for 200 rand24 values");
        uint32_t e0 = rd32(two.data() + 8), e1 = rd32(two.data() + BUF_SIZE + 8);
        disk_case("valid_two_buffers", "first 200 values of random_mantissa24_1024: 2 buffers, next of buffer 0 is "
                                       "the canonical 1",
                  two, 1024, 201, ",\"entries\":[" + num(e0) + "," + num(e1) + "]");
        vector<uint8_t> x = two;
        wr64(x.data(), 0x00007f5a3c001240ULL);
        disk_case("valid_two_buffers_next_pointer_like", "buffer 0 next = 0x00007f5a3c001240: only non-zero-ness "
                                                         "matters (gorilla.cc:293), result identical to "
                                                         "valid_two_buffers",
                  x, 1024, 201);
        disk_case("reject_truncated_chain", "2-buffer page passed with size 512: buffer 0 says another follows but "
                                            "nbuffers = 1 -> PGD_EMPTY",
                  two, 512, 201);
        x = two;
        wr64(x.data() + BUF_SIZE, CANONICAL_NEXT);
        disk_case("reject_last_of_two_next_nonzero", "2-buffer page whose last buffer has next != 0 -> PGD_EMPTY",
                  x, 1024, 201);
        x = two;
        wr32(x.data() + BUF_SIZE + 12, 3968);
        disk_case("reject_second_buffer_nbits_3968", "the nbits check applies to every visited buffer "
                                                     "(gorilla.cc:307-308)",
                  x, 1024, 201);
        x = two;
        wr64(x.data(), 0);
        disk_case("first_next_zero_of_two", "buffer 0 next = 0: the chain ends early, used = entries of buffer 0, "
                                            "buffer 1 is ignored",
                  x, 1024, 201);
        x = two;
        wr32(x.data() + 8, e0 + 5);
        disk_case("entries_inflated_first_of_two", "buffer 0 entries + 5: used = e0+5+e1, the reader stops at buffer "
                                                   "0's bit bound and never advances to buffer 1 (the advance happens "
                                                   "only when index + 1 > entries, gorilla.cc:368-391)",
                  x, 1024, e0 + e1 + 6);
        x = two;
        wr32(x.data() + 8, e0 - 1);
        disk_case("entries_deflated_first_of_two", "buffer 0 entries - 1: the reader moves to buffer 1 one value "
                                                   "early; buffer 0's last value is skipped",
                  x, 1024, e0 + e1);
    }
    {
        size_t nb;
        vector<uint32_t> in3(rand24.begin(), rand24.begin() + 320);
        vector<uint8_t> three = serialized(in3, &nb);
        if (nb != 3) die("expected 3 buffers for 320 rand24 values");
        vector<uint8_t> x = three;
        wr64(x.data() + BUF_SIZE, 0);
        disk_case("middle_next_zero_of_three", "3-buffer page with buffer 1 next = 0: used = e0 + e1, buffer 2 "
                                               "ignored",
                  x, 1536, 321);
        vector<uint8_t> z(512, 0);
        disk_case("all_zero_512", "512 zero bytes: valid chain of one empty buffer, used 0", z, 512, 1);
        vector<uint8_t> big = serialized(adversarial(1100), &nb);
        disk_case("eleven_buffers_5632", "adversarial_lzc_alternating_1100 page: pgd_create_from_disk_data has no "
                                         "length limit and accepts it; validate_page (pdc.c:921-931) would reject "
                                         "page_length 5632 > 5120 before this call",
                  big, (uint32_t)big.size(), 1101);
    }
    close_json(fd);

    // =========================================================================== raw pages
    fr = open_json(argv[1], "raw-pages.json",
                   "real page.c ARRAY_32BIT (type 0) and ARRAY_TIER1 (type 1) pages: pgd_append_point -> "
                   "pgd_copy_to_extent bytes -> pgd_create_from_disk_data -> pgdc points");
    raw_array32_case("array32_edges", "ARRAY_32BIT page (u32 LE storage_number per slot)", 1024,
                     {{1, SN_DEFAULT_FLAGS},
                      {1, SN_FLAG_NONE},
                      {-3.5, SN_DEFAULT_FLAGS},
                      {0, SN_FLAG_NONE},
                      {NAN, SN_DEFAULT_FLAGS},
                      {2.5, SN_DEFAULT_FLAGS | SN_FLAG_RESET},
                      {100, SN_DEFAULT_FLAGS},
                      {1e-9, SN_DEFAULT_FLAGS},
                      {123456789, SN_DEFAULT_FLAGS},
                      {-INFINITY, SN_FLAG_RESET}});
    raw_array32_case("array32_all_nan", "all NaN: pgd_is_empty true; each slot still reads back (count 1)", 1024,
                     {{NAN, SN_DEFAULT_FLAGS}, {NAN, SN_DEFAULT_FLAGS}, {NAN, SN_DEFAULT_FLAGS}});
    raw_tier1_case("tier1_edges", "ARRAY_TIER1 records {f32 sum, f32 min, f32 max, u16 count, u16 anomaly_count} "
                                  "(storage_number.h:78-84); sum/min/max are (float) casts (page.c:983-985)",
                   24,
                   {{10, 1, 5, 3, 0},
                    {NAN, NAN, NAN, 1, 0},
                    {NAN, NAN, NAN, 0, 0},
                    {0.1, 0.1, 0.1, 1, 0},
                    {1e39, 1, 1e39, 2, 1},
                    {-1e39, -1e39, -1, 2, 2},
                    {16777217, 16777217, 16777217, 1, 0},
                    {1e-46, 1e-46, 1e-46, 1, 0},
                    {-0.0, -0.0, 0.0, 65535, 65535},
                    {3.4028235677973366e38, 1.401298464324817e-45, 3.4028234663852886e38, 7, 3}});
    raw_tier1_case("tier1_all_nan", "all NaN sums: ALL_VALUES_EMPTY stays set (page.c:990-991)", 24,
                   {{NAN, NAN, NAN, 1, 0}, {NAN, 1, 2, 3, 0}});
    {
        vector<uint8_t> b(10);
        for (int i = 0; i < 10; i++) b[i] = (uint8_t)(0x10 + i);
        raw_disk_case("array32_size_10", "size 10: used = 10 / 4 = 2 (page.c:560), all 10 bytes copied",
                      RRDENG_PAGE_TYPE_ARRAY_32BIT, b, 10);
        raw_disk_case("array32_size_3", "size 3 < 4 -> PGD_EMPTY (page.c:524)", RRDENG_PAGE_TYPE_ARRAY_32BIT, b, 3);
        vector<uint8_t> t(20, 0);
        float one = 1.0f;
        memcpy(t.data(), &one, 4);
        memcpy(t.data() + 4, &one, 4);
        memcpy(t.data() + 8, &one, 4);
        t[12] = 1;
        raw_disk_case("tier1_size_20", "size 20: used = 20 / 16 = 1", RRDENG_PAGE_TYPE_ARRAY_TIER1, t, 20);
        raw_disk_case("tier1_size_15", "size 15 < 16 -> PGD_EMPTY", RRDENG_PAGE_TYPE_ARRAY_TIER1, t, 15);
    }
    close_json(fr);
    return 0;
}
