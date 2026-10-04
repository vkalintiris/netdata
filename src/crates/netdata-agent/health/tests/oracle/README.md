# Golden vector oracle

The vectors under `../vectors/` are produced by the C implementation itself. Health's objects are in no static
library, so the configuration path is compiled from the reference tree (`health_config.c`, `health_prototypes.c`,
`health_dyncfg.c`, three sources they call, and the label code `rrdlabels.c` with `rrdlabels-aggregated.c`) with the
flags of a production build, and linked with `health-oracle-stubs.c` and the production `libnetdata`:

- `gen-health-vectors.c` runs C's `health_readfile()` over the stock `src/health/health.d` of this tree and over
  `../corpus/`, and writes `rules.tsv` (each rule C accepted, with the JSON C hashes and the hash, as C stores it;
  the prototype store after each item), `records.tsv` (every record C logged, in order) and `copy.tsv` (each stored
  rule's three expressions as an alert would hold them after `health_prototype_copy_config()`, and what that logs);
- `gen-link-vectors.c` plays `../corpus/match/scenarios.txt`: `labels.tsv` (label sets as C holds them, and C's
  verdict on each pattern text against each set) and `link.tsv` (for each chart of each scenario, the rules C would
  link to it, in C's order: the stub of `rrdcalc_add_from_prototype()` writes them). It first runs C's own
  `rrdlabels_unittest()`, which must find no error;
- `health-unittest-dump.inc` and `health-unittest-main.inc` are spliced into a copy of
  `src/health/health-config-unittest.c`, so that C's own unit test runs, must pass, and leaves each call it makes to
  the ported functions, with what C returned, in `c_unittest.tsv`.

Rebuild them from a netdata checkout that has a CMake build (gcc, glibc, x86-64):

```sh
NETDATA_SRC=/path/to/netdata NETDATA_BUILD=/path/to/netdata/build tests/oracle/gen-health-vectors.sh
```

`NETDATA_BUILD` defaults to `$NETDATA_SRC/build`. The program runs in the crate's directory and names every file by a
path relative to it, as `cargo test` does: a path is part of each rule's source and of the records. The columns of
each file are in its header and at the top of the program that writes it; the field encoding is
`netdata-agent-text`'s, with a NULL string written as `\x00`.
