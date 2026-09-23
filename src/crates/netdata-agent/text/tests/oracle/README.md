# Golden vector oracle

The vectors under `../vectors/` are produced by the C implementation itself:
`gen-vectors.c` links the production `libnetdata` (same compiler flags, LTO)
and writes `input<TAB>outputs` lines for every function family;
`utf8-unittest-dump.inc` is spliced into a copy of
`src/libnetdata/sanitizers/utf8-sanitizer-unittest.c` to dump its case table as
`../vectors/utf8_unittest_cases.rs`. Inputs are deterministic (fixed seed).

Rebuild them from a netdata checkout that has a CMake build (gcc, glibc, x86-64):

```sh
NETDATA_SRC=/path/to/netdata NETDATA_BUILD=/path/to/netdata/build tests/oracle/gen-vectors.sh
```

`NETDATA_BUILD` defaults to `$NETDATA_SRC/build`. The field encoding is
described at the top of `gen-vectors.c`; `../common/mod.rs` decodes it.
