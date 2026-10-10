#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""gen-netdata-streaming-columns.py <function-netdata-streaming.c> [--check <streaming.rs>]: the 85
`buffer_rrdf_table_add_field()` calls of C's netdata-streaming Function as the Rust column table of
`src/builtins/streaming.rs` (`COLUMNS`), in C's order. Prints the Rust lines; with `--check`, compares them with the
file's table instead and exits non-zero when they differ. From the crate's directory:

    python3 tests/oracle/gen-netdata-streaming-columns.py \
        <netdata>/src/web/api/functions/function-netdata-streaming.c --check src/builtins/streaming.rs
"""
import re
import sys

src = open(sys.argv[1]).read()
calls = []
for m in re.finditer(r"buffer_rrdf_table_add_field\(", src):
    i, depth, args, cur, s = m.end(), 1, [], "", None
    while depth:
        c = src[i]
        if s:
            cur += c
            if c == "\\":
                cur += src[i + 1]; i += 1
            elif c == s:
                s = None
        elif c == '"':
            s = c; cur += c
        elif c in "([":
            depth += 1; cur += c
        elif c in ")]":
            depth -= 1
            if depth:
                cur += c
        elif c == "," and depth == 1:
            args.append(" ".join(cur.split())); cur = ""
        else:
            cur += c
        i += 1
    args.append(" ".join(cur.split()))
    calls.append(args)
assert len(calls) == 85, len(calls)

KIND = {"STRING": "String", "INTEGER": "Integer", "TIMESTAMP": "Timestamp", "DURATION": "Duration", "ARRAY": "Array",
        "NONE": "None"}
VIS = {"RRDF_FIELD_VISUAL_VALUE": "Value", "RRDF_FIELD_VISUAL_BAR": "Bar", "RRDF_FIELD_VISUAL_PILL": "Pill",
       "RRDR_FIELD_VISUAL_ROW_OPTIONS": "RowOptions"}
TR = {"NONE": "None", "NUMBER": "Number", "DATETIME_MS": "DatetimeMs", "DURATION_S": "DurationS"}
SUM = {"COUNT": "Count", "MIN": "Min", "MAX": "Max", "SUM": "Sum"}
FIL = {"NONE": "None", "RANGE": "Range", "MULTISELECT": "MultiSelect"}
MAXES = {
    "NAN": "Max::None", "100.0": "Max::Percent",
    "(double)max_db_from * MSEC_PER_SEC": "Max::Of(Stat::DbFrom)", "(double)max_db_to * MSEC_PER_SEC":
    "Max::Of(Stat::DbTo)", "(double)max_db_duration": "Max::Of(Stat::DbDuration)",
    "(double)max_db_metrics": "Max::Of(Stat::DbMetrics)", "(double)max_db_instances": "Max::Of(Stat::DbInstances)",
    "(double)max_db_contexts": "Max::Of(Stat::DbContexts)",
    "(double)max_in_connections": "Max::Of(Stat::InConnections)", "(double)max_in_since": "Max::Of(Stat::InSince)",
    "(double)max_in_age": "Max::Of(Stat::InAge)", "(double)max_in_hops": "Max::Of(Stat::InHops)",
    "(double)max_collection_replication_instances": "Max::Of(Stat::InReplInstances)",
    "(double)max_in_local_port": "Max::Of(Stat::InLocalPort)",
    "(double)max_in_remote_port": "Max::Of(Stat::InRemotePort)",
    "(double)max_out_connections": "Max::Of(Stat::OutConnections)",
    "(double)max_out_since": "Max::Of(Stat::OutSince)", "(double)max_out_age": "Max::Of(Stat::OutAge)",
    "(double)max_out_hops": "Max::Of(Stat::OutHops)",
    "(double)max_streaming_replication_instances": "Max::Of(Stat::OutReplInstances)",
    "(double)max_out_remote_port": "Max::Of(Stat::OutRemotePort)",
    "(double)max_sent_bytes_on_this_connection_per_type[STREAM_TRAFFIC_TYPE_DATA]": "Max::Of(Stat::Sent(Traffic::Data))",
    "(double)max_sent_bytes_on_this_connection_per_type[STREAM_TRAFFIC_TYPE_METADATA]":
    "Max::Of(Stat::Sent(Traffic::Metadata))",
    "(double)max_sent_bytes_on_this_connection_per_type[STREAM_TRAFFIC_TYPE_REPLICATION]":
    "Max::Of(Stat::Sent(Traffic::Replication))",
    "(double)max_sent_bytes_on_this_connection_per_type[STREAM_TRAFFIC_TYPE_FUNCTIONS]":
    "Max::Of(Stat::Sent(Traffic::Functions))",
    "(double)max_out_attempt_since": "Max::Of(Stat::OutAttemptSince)",
    "(double)max_out_attempt_age": "Max::Of(Stat::OutAttemptAge)",
    "(double)max_ml_anomalous": "Max::Ml", "(double)max_ml_normal": "Max::Ml", "(double)max_ml_trained": "Max::Ml",
    "(double)max_ml_pending": "Max::Ml", "(double)max_ml_silenced": "Max::Ml",
}
SORT = {"RRDF_FIELD_SORT_ASCENDING": "sort::ASCENDING", "RRDF_FIELD_SORT_DESCENDING": "sort::DESCENDING",
        "RRDF_FIELD_SORT_FIXED": "sort::FIXED"}
OPT = {"NONE": "opts::NONE", "VISIBLE": "opts::VISIBLE", "UNIQUE_KEY": "opts::UNIQUE_KEY", "STICKY": "opts::STICKY",
       "DUMMY": "opts::DUMMY"}


def strip(prefix, v):
    assert v.startswith(prefix), (prefix, v)
    return v[len(prefix):]


out = []
for n, a in enumerate(calls):
    wb, fid, key, name, kind, vis, tr, dec, units, mx, srt, ptr, summ, fil, opt, dflt = a
    assert wb == "wb" and fid == "field_id++" and ptr == "NULL" and dflt == "NULL", a
    kind = KIND[strip("RRDF_FIELD_TYPE_", kind)]
    vis = VIS[vis]
    tr = TR[strip("RRDF_FIELD_TRANSFORM_", tr)]
    summ = SUM[strip("RRDF_FIELD_SUMMARY_", summ)]
    fil = FIL[strip("RRDF_FIELD_FILTER_", fil)]
    opt = " | ".join(OPT[strip("RRDF_FIELD_OPTS_", o.strip())] for o in opt.split("|"))
    units = "None" if units == "NULL" else f"Some({units})"
    mx = MAXES[mx]
    srt = SORT[srt]
    if (kind, vis, tr, dec, units, mx, srt, summ, fil) == ("String", "Value", "None", "0", "None", "Max::None",
                                                            "sort::ASCENDING", "Count", "MultiSelect"):
        line = f"    text({key}, {name}, {opt}),"
    else:
        line = (f"    col({key}, {name}, FieldType::{kind}, Visual::{vis}, Transform::{tr}, {dec}, {units}, {mx}, "
                f"{srt}, Summary::{summ}, Filter::{fil}, {opt}),")
    if len(line) > 120:
        head = f"    {line.strip().split('(', 1)[0]}("
        body = line.strip()[len(head) - 4:-2]
        parts = [p.strip() for p in re.findall(r'"(?:[^"\\]|\\.)*"|Some\("[^"]*"\)|[^,]+', body)]
        lines, cur = [head], "       "
        for p in parts:
            piece = " " + p + ","
            if len(cur) + len(piece) > 120:
                lines.append(cur); cur = "       "
            cur += piece
        lines.append(cur)
        lines.append("    ),")
        line = "\n".join(lines)
    out.append(line)
text = "\n".join(out) + "\n"
if len(sys.argv) > 3 and sys.argv[2] == "--check":
    rs = open(sys.argv[3]).read()
    start = rs.index("const COLUMNS: [Column; 85] = [\n") + len("const COLUMNS: [Column; 85] = [\n")
    end = rs.index("\n];\n", start) + 1
    if rs[start:end] == text:
        print("COLUMNS equal to C's 85 calls")
    else:
        sys.exit("COLUMNS differ from C's calls")
else:
    sys.stdout.write(text)
