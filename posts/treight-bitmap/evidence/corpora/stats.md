# Corpus aggregate statistics (all values computed by `corpus_stats.py`)

## certstream-250k

### certstream-250k — certstream

- Lines: **250,000**
- Uncompressed bytes: **413,067,184**
- Line size (bytes): min 1,305, p50 1,576, p95 1,974, p99 2,607, max 15,140
- Line size histogram: 1024-2048: 238545, 2048-4096: 10739, 4096-8192: 629, 8192-16384: 87
- Event top-level keys (lines containing key): `cert_index` 250,000, `cert_link` 250,000, `leaf_cert` 250,000, `seen` 250,000, `source` 250,000, `update_type` 250,000
- `update_type` distribution: `X509LogEntry` 115,752, `PrecertLogEntry` 134,248

## jetstream-250k

### jetstream-250k — jetstream

- Lines: **250,000**
- Uncompressed bytes: **150,391,274**
- Line size (bytes): min 191, p50 503, p95 851, p99 1,854, max 54,151
- Line size histogram: <256: 7440, 256-512: 174933, 512-1024: 59640, 1024-2048: 5721, 2048-4096: 796, 4096-8192: 1312, 8192-16384: 25, 16384-32768: 124, 32768-65536: 9
- Event top-level keys (lines containing key): `did` 250,000, `kind` 250,000, `time_us` 250,000, `commit` 249,711, `account` 159, `identity` 130
- `kind` distribution: `commit` 249,711, `identity` 130, `account` 159
- commit `operation` distribution: `create` 239,676, `delete` 7,151, `update` 2,884

## mixed-250k

### mixed-250k — certstream

- Lines: **125,000**
- Uncompressed bytes: **213,902,250**
- Line size (bytes): min 1,343, p50 1,626, p95 2,098, p99 2,652, max 15,180
- Line size histogram: 1024-2048: 118030, 2048-4096: 6577, 4096-8192: 323, 8192-16384: 70
- Event top-level keys (lines containing key): `cert_index` 125,000, `cert_link` 125,000, `leaf_cert` 125,000, `seen` 125,000, `source` 125,000, `update_type` 125,000
- `update_type` distribution: `X509LogEntry` 42,804, `PrecertLogEntry` 82,196

### mixed-250k — jetstream

- Lines: **125,000**
- Uncompressed bytes: **80,975,197**
- Line size (bytes): min 228, p50 540, p95 899, p99 2,002, max 51,506
- Line size histogram: <256: 4561, 256-512: 15107, 512-1024: 100726, 1024-2048: 3382, 2048-4096: 324, 4096-8192: 798, 8192-16384: 18, 16384-32768: 80, 32768-65536: 4
- Event top-level keys (lines containing key): `did` 125,000, `kind` 125,000, `time_us` 125,000, `commit` 124,864, `account` 75, `identity` 61
- `kind` distribution: `commit` 124,864, `account` 75, `identity` 61
- commit `operation` distribution: `create` 118,936, `delete` 4,441, `update` 1,487
