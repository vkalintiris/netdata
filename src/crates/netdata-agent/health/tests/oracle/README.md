# Golden vector oracle

The vectors under `../vectors/` are produced by the C implementation itself. Health's objects are in no static
library, so the configuration path is compiled from the reference tree (`health_config.c`, `health_prototypes.c`,
`health_dyncfg.c`, three sources they call, and the label code `rrdlabels.c` with `rrdlabels-aggregated.c`) with the
flags of a production build, and linked with `health-oracle-stubs.c` and the production `libnetdata` (the loop's
program compiles that file with `HEALTH_ORACLE_LOOP`, which leaves out what `rrdcalc.c` and `health_event_loop.c`
define themselves):

- `gen-health-vectors.c` runs C's `health_readfile()` over the stock `src/health/health.d` of this tree and over
  `../corpus/`, and writes `rules.tsv` (each rule C accepted, with the JSON C hashes and the hash, as C stores it;
  the prototype store after each item), `records.tsv` (every record C logged, in order) and `copy.tsv` (each stored
  rule's three expressions as an alert would hold them after `health_prototype_copy_config()`, and what that logs);
- `gen-link-vectors.c` plays `../corpus/match/scenarios.txt`: `labels.tsv` (label sets as C holds them, and C's
  verdict on each pattern text against each set) and `link.tsv` (for each chart of each scenario, the rules C would
  link to it, in C's order: the stub of `rrdcalc_add_from_prototype()` writes them). It first runs C's own
  `rrdlabels_unittest()`, which must find no error;
- `gen-loop-vectors.c` is the evaluation loop's: it links C's alert instances (`rrdcalc.c`), alert log
  (`health_log.c`), variable lookup (`health_variable.c`, `rrdvar.c`), the log's scan (`health_notifications.c`) and
  the per-host pass itself (`health_event_loop.c`, compiled as a copy with `health-loop-splice.inc` appended, because
  the pass is static), with what the daemon gives them stubbed in `health-loop-stubs.c`: the charts, the database
  lookup (scripted results; its arguments are recorded), SQLite, the queues, the notification (recorded, the entry
  marked as processed), the wall clock (the program defines `clock_gettime()`, so every id and time is a fixed
  number) and an entry's transition id (counted out instead of random, so the trace shows which entry got which). A
  scenario can also close the host's gate or take a chart out of the index in the middle of a pass. It writes
  `delay.tsv` (the delay multiplier over a grid), `units.tsv` (a value with its unit, as an
  entry's value texts are made) and `loop.tsv`: each scenario under `../corpus/loop/` played step by step, with the
  alerts, their published snapshots, the log's entries, the stubs' calls and C's log records after every step. One
  process per scenario. `queue.tsv` is the same over `../corpus/queue/`: the scenarios in which the metadata queue
  and its thread's store job are in play (the queue takes or refuses a save, the thread a step runs on, the job
  that saves each queued entry as it stands by then, the alarm ids the alert log's table knows);
- `health-unittest-dump.inc` and `health-unittest-main.inc` are spliced into a copy of
  `src/health/health-config-unittest.c`, so that C's own unit test runs, must pass, and leaves each call it makes to
  the ported functions, with what C returned, in `c_unittest.tsv`.

Rebuild them from a netdata checkout that has a CMake build (gcc, glibc, x86-64):

```sh
NETDATA_SRC=/path/to/netdata NETDATA_BUILD=/path/to/netdata/build tests/oracle/gen-health-vectors.sh
```

`../vectors/variables/` is not made by these programs: it holds answers of the running C agent, recorded by the
parity harness's check `health.variables` (`tests/parity/health_variables_test.go`), once with health off and once
with two linked rules. Each case has its `index.tsv` (file, HTTP status, content type, request), the bodies as C sent
them, and under `inputs/` what the agent was fed (the plugin's lines, the rules, the `[health]` section). The wall
clock is in some bodies (`after`, `before`, `now`, a collected chart's `last_collected_t`); the unit tests of
`src/variable.rs` and `src/api.rs` build the same host and read those seconds from each body.

`../vectors/loop-api/flags/` is recorded the same way, by the check `health.loop` (`tests/parity/health_loop_test.go`,
case `flags`): six answers of the three alarm endpoints at one moment of the running C agent (the listing of all
alerts and of the raised ones, their values, two counts), with the rules and the plugin's lines. The run's directory
in them is written `{run}`. The unit test of `src/api.rs` links the case's rules, sets each alert's published state
to what the recorded listing of all alerts shows of it, and must write every one of the six bodies byte for byte.

`NETDATA_BUILD` defaults to `$NETDATA_SRC/build`. The program runs in the crate's directory and names every file by a
path relative to it, as `cargo test` does: a path is part of each rule's source and of the records. The columns of
each file are in its header and at the top of the program that writes it; the field encoding is
`netdata-agent-text`'s, with a NULL string written as `\x00`.
