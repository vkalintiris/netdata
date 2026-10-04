# The configuration corpus

`health.d` files written to make C's reader take every path: one directory per family, one item per file. A
directory named `*.group` is one item whose files are read one after the other into the same prototype store.

| Family | What it holds |
|---|---|
| `lines` | continuations, comments, reads around the 4,096-byte buffer (a read that fills it, a join that reaches its end), a lone backslash before a comment, CRLF, CR alone, tabs, a NUL (also after a continuation and inside a long line), lines without a key or a value |
| `keys` | every keyword, repeated keys, quotes, names, durations, `green` and `red`, `options`, `repeat`, unknown keys; `errno.conf` holds the numbers that leave a range error behind, each followed by a line whose record shows it; keys before the first rule; `never`, `off` and `ago` as durations |
| `labels` | `os`, `hosts`, `plugin`, `module`, `host labels`, `chart labels` and their special values |
| `lookup` | the 149 inputs of C's unit table as rules, each option, method and duration; `*-every.conf` repeats them with an `every` after the lookup, so the rule is kept whatever the lookup did |
| `delay` | the inputs of C's delay unit table, bad durations, multipliers |
| `expr` | expressions that fail to parse, `green` and `red` replaced inside expressions, numbers that overflow inside one |
| `validate` | each reason a rule is refused, chains of rules under one name, the same name in two files |
| `smoke` | a first small file |
| `match` | rules for the matching of hosts and charts (`basic.conf`: alarms by id and by name, templates, chains mixing both, disabled rules, each label key), and `scenarios.txt`, which the link generator plays: label sets, pattern texts, and per scenario its files, `enabled alarms`, hosts and charts (the stock rules among them) |
| `loop` | the evaluation loop's scenarios: a rule file (`*.conf`, also an item of the configuration vectors) and a script (`*.scn`) of charts, values, clock and passes, which `../oracle/gen-loop-vectors.c` plays through C's own pass; the directives are at the top of that program. Statuses and their combinations, calculations, the lookup's outcomes, each reason an alert is not run, delays, repeats, obsolete charts, linking again after label changes, a stopping service, a postponed host, ids without a database, the trim, durations, names between alerts, the runtime texts, several charts |
| `files` | paths the reader cannot read: a dangling link, a directory, then a file whose record shows the `errno` left behind |

The files hold bytes on purpose (a NUL, bytes above 0x7F, CRLF, a missing final newline): do not reformat them.
`../vectors/` is what C made of them; regenerate it with `../oracle/gen-health-vectors.sh` after any change here.

No file here may hold a line that is only a backslash before a blank line, comment lines or the end of the file: C's
reader never returns from it, and the generator would hang.
