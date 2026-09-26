# libsqlite3-sys, patched to build the C agent's SQLite

A copy of [`libsqlite3-sys` 0.38.2](https://github.com/rusqlite/rusqlite) (MIT, `LICENSE`) that `rusqlite` uses
through `[patch.crates-io]` in `src/crates/Cargo.toml`. It compiles the SQLite that the C agent builds in
`packaging/cmake/Modules/NetdataSQLite.cmake`, so the Rust agent reports and writes what the C agent does (decisions
D11 and D59.1 of the Rust agent effort).

What differs from the upstream crate:

- `sqlite3/sqlite3.c` and `sqlite3/sqlite3.h` are the SQLite 3.53.4 amalgamation generated as the C build generates
  it (`configure --enable-update-limit`, `make sqlite3.c sqlite3.h`), and `sqlite3recover.{c,h}` and `dbdata.c`
  come from the same source tree (`ext/recover`). SQLite is in the public domain.
- `build.rs` compiles them with the C build's defines (`SQLITE_ENABLE_UPDATE_DELETE_LIMIT`,
  `SQLITE_ENABLE_MEMORY_MANAGEMENT`, `SQLITE_OMIT_LOAD_EXTENSION`, `SQLITE_ENABLE_DBSTAT_VTAB`,
  `SQLITE_ENABLE_DBPAGE_VTAB`) plus `_GNU_SOURCE`, instead of the upstream set.
- The SQLCipher, WASI and pre-generated non-bundled bindings are removed; only the `bundled` build is supported. The
  bundled 3.53.2 bindings are kept: the API is the same.

`SHA256SUMS` pins the five C files. `regen-amalgamation.sh` regenerates them from the source
zip the C build downloads and checks both hashes; when the C build moves to another SQLite, update the version and
hashes there and in this script, run it, and commit the new files with the new `SHA256SUMS`.
