# Golden vector oracle

The vectors under `../vectors/` are produced by the C implementation itself, linked from a production build of
`libnetdata` (same compiler flags, LTO):

- `gen-eval-vectors.c` runs C's expression evaluator over a deterministic corpus (fixed lists and a fixed seed) and
  writes `corpus.tsv`, `hardcode.tsv` and `bindings.tsv`;
- `eval-unittest-dump.inc` is spliced into copies of `src/libnetdata/eval/eval-unittest.c` and
  `eval-unittest-hardcoding.c`, so that C's own unit tests run, must pass, and dump their vectors with what C
  returned for each as `c_unittest.tsv` and `c_unittest_hardcode.tsv`.

Rebuild them from a netdata checkout that has a CMake build (gcc, glibc, x86-64):

```sh
NETDATA_SRC=/path/to/netdata NETDATA_BUILD=/path/to/netdata/build tests/oracle/gen-eval-vectors.sh
```

`NETDATA_BUILD` defaults to `$NETDATA_SRC/build`. The columns of each file are in its header and at the top of the
program that writes it; the field encoding is `netdata-agent-text`'s, decoded by its `tests/common/mod.rs`.
