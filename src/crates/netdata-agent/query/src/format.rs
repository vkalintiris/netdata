//! The v1 renderers of a result (`src/web/api/formatters/`): json and datatable (`json/json.c`), csv and its
//! relatives (`csv/csv.c`), ssv (`ssv/ssv.c`) and the per-row reduction they share (`value/value.c`). Spec §9.2-9.4.

use netdata_agent_sys::localtime;
use netdata_agent_text::datetime::rfc3339_datetime_utc;
use netdata_agent_text::json::{JsonWriter, json_escape};
use netdata_agent_text::print::{print_date, print_jsdate, print_netdata_double_or_null};

use crate::jsonwrap::jskey;
use crate::rrdr::{Rrdr, value_flags};
use crate::tables::{Format, options};
use crate::target::metric_status;

/// `rrdr_dimension_should_be_exposed()`.
pub fn exposed(od: u32, options: u64) -> bool {
    if options & options::RETURN_RAW != 0 && od & metric_status::QUERIED != 0 {
        return true;
    }
    od & metric_status::HIDDEN == 0
        && od & metric_status::QUERIED != 0
        && (options & options::NONZERO == 0 || od & metric_status::NONZERO != 0)
}

/// Row indexes in output order: newest first unless `flip` (REVERSED).
fn row_order(rows: usize, options: u64) -> Box<dyn Iterator<Item = usize>> {
    if options & options::REVERSED != 0 {
        Box::new(0..rows)
    } else {
        Box::new((0..rows).rev())
    }
}

/// `rrdr2json_single_quoted_strcat()`: control bytes as `\uXXXX`, `\` and `'` escaped, everything else raw.
fn single_quoted_escape(out: &mut Vec<u8>, text: &[u8]) {
    for &b in text {
        if b == 0 {
            break;
        }
        if b < b' ' {
            out.extend_from_slice(format!("\\u{:04X}", b).as_bytes());
        } else {
            if b == b'\\' || b == b'\'' {
                out.push(b'\\');
            }
            out.push(b);
        }
    }
}

/// `buffer_strcat()`: the bytes up to the first NUL.
fn strcat(out: &mut Vec<u8>, text: &[u8]) {
    let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
    out.extend_from_slice(&text[..end]);
}

/// `rrdr2json()`: `datatable` selects the Google visualization table.
pub fn rrdr2json(r: &Rrdr, out: &mut Vec<u8>, mut options: u64, datatable: bool) {
    let js_dates;
    let mut dates_with_new = false;
    let (kq, sq): (&str, &str);
    let (pre_label, post_label, pre_date, post_date, pre_value, post_value, post_line, data_begin);
    let (mut normal_annotation, mut overflow_annotation, mut object_rows_time) =
        (String::new(), String::new(), String::new());
    let finish = "\n        ]\n    }";
    if datatable {
        js_dates = true;
        (kq, sq) = if options & options::GOOGLE_JSON != 0 {
            ("", "'")
        } else {
            ("\"", "\"")
        };
        pre_date = format!("        {{{kq}c{kq}:[{{{kq}v{kq}:{sq}");
        post_date = format!("{sq}}}");
        pre_label = format!(",\n     {{{kq}id{kq}:{sq}{sq},{kq}label{kq}:{sq}");
        post_label = format!("{sq},{kq}pattern{kq}:{sq}{sq},{kq}type{kq}:{sq}number{sq}}}");
        pre_value = format!(",{{{kq}v{kq}:");
        post_value = "}";
        post_line = "]}";
        data_begin = format!("\n  ],\n    {kq}rows{kq}:\n [\n");
        overflow_annotation = format!(
            ",{{{kq}v{kq}:{sq}RESET OR OVERFLOW{sq}}},{{{kq}v{kq}:{sq}The counters have been wrapped.{sq}}}"
        );
        normal_annotation = format!(",{{{kq}v{kq}:null}},{{{kq}v{kq}:null}}");
        out.extend_from_slice(format!("{{\n {kq}cols{kq}:\n [\n").as_bytes());
        out.extend_from_slice(
            format!(
                "        {{{kq}id{kq}:{sq}{sq},{kq}label{kq}:{sq}time{sq},{kq}pattern{kq}:{sq}{sq},{kq}type{kq}:\
                 {sq}datetime{sq}}},\n"
            )
            .as_bytes(),
        );
        out.extend_from_slice(
            format!(
                "        {{{kq}id{kq}:{sq}{sq},{kq}label{kq}:{sq}{sq},{kq}pattern{kq}:{sq}{sq},{kq}type{kq}:\
                 {sq}string{sq},{kq}p{kq}:{{{kq}role{kq}:{sq}annotation{sq}}}}},\n"
            )
            .as_bytes(),
        );
        out.extend_from_slice(
            format!(
                "        {{{kq}id{kq}:{sq}{sq},{kq}label{kq}:{sq}{sq},{kq}pattern{kq}:{sq}{sq},{kq}type{kq}:\
                 {sq}string{sq},{kq}p{kq}:{{{kq}role{kq}:{sq}annotationText{sq}}}}}"
            )
            .as_bytes(),
        );
        // Google wants its own keys.
        options &= !options::OBJECTSROWS;
    } else {
        (kq, sq) = ("\"", "\"");
        js_dates = options & options::GOOGLE_JSON != 0;
        dates_with_new = js_dates;
        pre_date = if options & options::OBJECTSROWS != 0 {
            "            {"
        } else {
            "            ["
        }
        .to_string();
        post_date = String::new();
        pre_label = ",\"".to_string();
        post_label = "\"".to_string();
        pre_value = ",".to_string();
        post_value = "";
        post_line = if options & options::OBJECTSROWS != 0 {
            "}"
        } else {
            "]"
        };
        data_begin = format!("],\n        {kq}data{kq}:[\n");
        out.extend_from_slice(format!("{{\n        {kq}labels{kq}:[{sq}time{sq}").as_bytes());
        if options & options::OBJECTSROWS != 0 {
            object_rows_time = format!("{kq}time{kq}: ");
        }
    }

    let mut exposed_count = 0;
    for c in 0..r.columns {
        if !exposed(r.od[c], options) {
            continue;
        }
        out.extend_from_slice(pre_label.as_bytes());
        if sq == "'" {
            single_quoted_escape(out, r.dn[c].as_bytes());
        } else {
            json_escape(out, r.dn[c].as_bytes());
        }
        out.extend_from_slice(post_label.as_bytes());
        exposed_count += 1;
    }
    if exposed_count == 0 {
        out.extend_from_slice(pre_label.as_bytes());
        out.extend_from_slice(b"no data");
        out.extend_from_slice(post_label.as_bytes());
    }
    out.extend_from_slice(data_begin.as_bytes());
    if exposed_count == 0 {
        out.extend_from_slice(finish.as_bytes());
        return;
    }

    let start = if options & options::REVERSED != 0 {
        0
    } else {
        r.rows.wrapping_sub(1)
    };
    for i in row_order(r.rows, options) {
        let base = i * r.columns;
        let now = r.t[i];
        if js_dates {
            let Some(tm) = localtime(now) else {
                continue;
            };
            if i != start {
                out.extend_from_slice(b",\n");
            }
            out.extend_from_slice(pre_date.as_bytes());
            if options & options::OBJECTSROWS != 0 {
                out.extend_from_slice(object_rows_time.as_bytes());
            }
            if dates_with_new {
                out.extend_from_slice(b"new ");
            }
            print_jsdate(out, tm.year, tm.month0, tm.mday, tm.hour, tm.min, tm.sec);
            out.extend_from_slice(post_date.as_bytes());
            if datatable {
                // Google supports one annotation per row.
                let reset = (0..r.columns).any(|c| {
                    r.od[c] & metric_status::QUERIED != 0 && r.o[base + c] & value_flags::RESET != 0
                });
                out.extend_from_slice(if reset {
                    overflow_annotation.as_bytes()
                } else {
                    normal_annotation.as_bytes()
                });
            }
        } else {
            if i != start {
                out.extend_from_slice(b",\n");
            }
            out.extend_from_slice(pre_date.as_bytes());
            if options & options::OBJECTSROWS != 0 {
                out.extend_from_slice(object_rows_time.as_bytes());
            }
            if options & options::RFC3339 != 0 {
                out.push(b'"');
                out.extend_from_slice(
                    rfc3339_datetime_utc((now as u64).wrapping_mul(1_000_000), 0).as_bytes(),
                );
                out.push(b'"');
            } else {
                print_netdata_double_or_null(out, now as f64);
                if options & options::MILLISECONDS != 0 {
                    out.extend_from_slice(b"000");
                }
            }
            out.extend_from_slice(post_date.as_bytes());
        }

        for c in 0..r.columns {
            if !exposed(r.od[c], options) {
                continue;
            }
            let n = if options & options::INTERNAL_AR != 0 {
                r.ar[base + c]
            } else {
                r.v[base + c]
            };
            out.extend_from_slice(pre_value.as_bytes());
            if options & options::OBJECTSROWS != 0 {
                out.push(b'"');
                json_escape(out, r.dn[c].as_bytes());
                out.extend_from_slice(b"\": ");
            }
            if r.o[base + c] & value_flags::EMPTY != 0 && options & options::INTERNAL_AR == 0 {
                out.extend_from_slice(if options & options::NULL2ZERO != 0 {
                    b"0"
                } else {
                    b"null"
                });
            } else {
                print_netdata_double_or_null(out, n);
            }
            out.extend_from_slice(post_value.as_bytes());
        }
        out.extend_from_slice(post_line.as_bytes());
    }
    out.extend_from_slice(finish.as_bytes());
}

/// `rrdr2csv()`: a header line (and markdown's `:---:` line), then one line per row; nothing at all without an
/// exposed column.
#[allow(clippy::too_many_arguments)]
pub fn rrdr2csv(
    r: &Rrdr,
    out: &mut Vec<u8>,
    format: Format,
    options: u64,
    startline: &str,
    separator: &str,
    endline: &str,
    betweenlines: &str,
) {
    let q: &[u8] = if options & options::LABEL_QUOTES != 0 {
        b"\""
    } else {
        b""
    };
    let header = |out: &mut Vec<u8>, first: &[u8], each: &dyn Fn(&mut Vec<u8>, usize)| -> usize {
        let mut i = 0;
        for c in 0..r.columns {
            if !exposed(r.od[c], options) {
                continue;
            }
            if i == 0 {
                out.extend_from_slice(startline.as_bytes());
                out.extend_from_slice(q);
                out.extend_from_slice(first);
                out.extend_from_slice(q);
            }
            out.extend_from_slice(separator.as_bytes());
            out.extend_from_slice(q);
            each(out, c);
            out.extend_from_slice(q);
            i += 1;
        }
        i
    };
    let exposed_count = header(out, b"time", &|out, c| {
        if format == Format::CsvJsonArray {
            json_escape(out, r.dn[c].as_bytes());
        } else {
            strcat(out, r.dn[c].as_bytes());
        }
    });
    if exposed_count == 0 {
        return;
    }
    out.extend_from_slice(endline.as_bytes());
    if format == Format::Markdown {
        header(out, b":---:", &|out, _| out.extend_from_slice(b":---:"));
        out.extend_from_slice(endline.as_bytes());
    }

    for i in row_order(r.rows, options) {
        let base = i * r.columns;
        out.extend_from_slice(betweenlines.as_bytes());
        out.extend_from_slice(startline.as_bytes());
        let now = r.t[i];
        if options & (options::SECONDS | options::MILLISECONDS) != 0 {
            print_netdata_double_or_null(out, now as f64);
            if options & options::MILLISECONDS != 0 {
                out.extend_from_slice(b"000");
            }
        } else {
            let Some(tm) = localtime(now) else {
                continue;
            };
            print_date(
                out,
                tm.year,
                tm.month0 + 1,
                tm.mday,
                tm.hour,
                tm.min,
                tm.sec,
            );
        }
        for c in 0..r.columns {
            if !exposed(r.od[c], options) {
                continue;
            }
            out.extend_from_slice(separator.as_bytes());
            if r.o[base + c] & value_flags::EMPTY != 0 {
                out.extend_from_slice(if options & options::NULL2ZERO != 0 {
                    b"0"
                } else {
                    b"null"
                });
            } else {
                print_netdata_double_or_null(out, r.v[base + c]);
            }
        }
        out.extend_from_slice(endline.as_bytes());
    }
}

/// `rrdr2value()`: one row reduced over its exposed, non-empty cells; `None` when there is none (`all_null`).
/// `r->d == 0` or a row out of range gives NaN without marking the row null, as in C.
pub fn rrdr2value(r: &Rrdr, i: usize, options: u64) -> (f64, bool) {
    if r.columns == 0 || r.rows == 0 || i >= r.rows {
        return (f64::NAN, false);
    }
    let base = i * r.columns;
    let (mut sum, mut min, mut max, mut dims) = (0.0, f64::NAN, f64::NAN, 0usize);
    for c in 0..r.columns {
        if !exposed(r.od[c], options) || r.o[base + c] & value_flags::EMPTY != 0 {
            continue;
        }
        let n = r.v[base + c];
        if dims == 0 {
            min = n;
            max = n;
        }
        sum += n;
        if n < min {
            min = n;
        }
        if n > max {
            max = n;
        }
        dims += 1;
    }
    if dims == 0 {
        let v = if options & options::NULL2ZERO != 0 {
            0.0
        } else {
            f64::NAN
        };
        return (v, true);
    }
    let mut v = if options & options::DIMS_MIN2MAX != 0 {
        max - min
    } else if options & options::DIMS_AVERAGE != 0 {
        sum / dims as f64
    } else if options & options::DIMS_MIN != 0 {
        min
    } else if options & options::DIMS_MAX != 0 {
        max
    } else {
        sum
    };
    if options & options::NULL2ZERO != 0 && !v.is_finite() {
        v = 0.0;
    }
    (v, false)
}

/// `rrdr2ssv()`: one reduced value per row; overwrites `view.min`/`view.max` with the reduced values.
pub fn rrdr2ssv(
    r: &mut Rrdr,
    out: &mut Vec<u8>,
    options: u64,
    prefix: &str,
    separator: &str,
    suffix: &str,
) {
    out.extend_from_slice(prefix.as_bytes());
    let start = if options & options::REVERSED != 0 {
        0
    } else {
        r.rows.wrapping_sub(1)
    };
    for i in row_order(r.rows, options) {
        let (v, all_null) = rrdr2value(r, i, options);
        if i != start {
            if r.view.min > v {
                r.view.min = v;
            }
            if r.view.max < v {
                r.view.max = v;
            }
            out.extend_from_slice(separator.as_bytes());
        } else {
            r.view.min = v;
            r.view.max = v;
        }
        if all_null {
            out.extend_from_slice(if options & options::NULL2ZERO != 0 {
                b"0"
            } else {
                b"null"
            });
        } else {
            print_netdata_double_or_null(out, v);
        }
    }
    out.extend_from_slice(suffix.as_bytes());
}

/// `MCP_QUERY_INFO_RESULT_SECTION` (`src/web/mcp/mcp.h`).
const MCP_QUERY_INFO_RESULT_SECTION: &str = "The 'result' section contains the actual time-series data points.\n\
Each point of each dimension is represented as an array of 3 values:\n  \
a) the value itself, aggregated as requested\n  \
b) the point anomaly rate percentage (% of anomalous samples vs total samples)\n  \
c) the point annotations, a combined bitmap of 1+2+4, where:\n     \
1 = empty data, value should be ignored\n     \
2 = counter has been reset or overflown, value may not be accurate\n     \
4 = partial data, at least one of the sources aggregated had gaps at that time\n\
Summarized data across the entire time-frame is provided at the 'view' section.";

/// `rrdr2json_v2()` for a result without group-by counts (a v1 query): `labels`, the point schema and one
/// `[time, [value, anomaly rate, annotations]...]` row per point.
pub fn rrdr2json_v2(r: &Rrdr, w: &mut JsonWriter, options: u64) {
    w.member_add_object(b"result");
    if options & options::MCP_INFO != 0 {
        w.member_add_string("info", MCP_QUERY_INFO_RESULT_SECTION);
    }
    w.member_add_array(Some(b"labels"));
    w.add_array_item_string("time");
    let mut exposed_count = 0;
    for c in 0..r.columns {
        if exposed(r.od[c], options) {
            w.add_array_item_string(&r.di[c]);
            exposed_count += 1;
        }
    }
    w.array_close();
    w.member_add_object(jskey(options, "point", "point_schema"));
    w.member_add_uint64("value", 0);
    w.member_add_uint64(jskey(options, "arp", "anomaly_rate_percent"), 1);
    w.member_add_uint64(jskey(options, "pa", "point_annotations_bitmap"), 2);
    w.object_close();
    w.member_add_array(Some(b"data"));
    if exposed_count != 0 {
        for i in row_order(r.rows, options) {
            let base = i * r.columns;
            w.add_array_item_array();
            if options & options::MILLISECONDS != 0 {
                w.add_array_item_time_ms(r.t[i]);
            } else {
                w.add_array_item_time_t_formatted(r.t[i], options & options::RFC3339 != 0);
            }
            for c in 0..r.columns {
                if !exposed(r.od[c], options) {
                    continue;
                }
                let o = r.o[base + c];
                w.add_array_item_array();
                if o & value_flags::EMPTY != 0 {
                    w.add_array_item_double(if options & options::NULL2ZERO != 0 {
                        0.0
                    } else {
                        f64::NAN
                    });
                } else {
                    w.add_array_item_double(r.v[base + c]);
                }
                w.add_array_item_double(r.ar[base + c]);
                w.add_array_item_uint64(u64::from(o));
                w.array_close();
            }
            w.array_close();
        }
    }
    w.array_close();
    w.object_close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rrdr::View;

    /// The spec's example (§9.10): columns `a`, `b` named `A`, `B`; rows at 100, 101, 102; `a` = 1, 2, 3 and
    /// `b` = 10, empty, 30.5.
    fn rrdr() -> Rrdr {
        let e = value_flags::EMPTY;
        let queried = metric_status::QUERIED;
        Rrdr {
            n: 3,
            rows: 3,
            columns: 2,
            t: vec![100, 101, 102],
            v: vec![1.0, 10.0, 2.0, 0.0, 3.0, 30.5],
            o: vec![0, 0, 0, e, 0, 0],
            ar: vec![0.0; 6],
            od: vec![queried, queried],
            di: vec!["a".into(), "b".into()],
            dn: vec!["A".into(), "B".into()],
            view: View {
                after: 100,
                before: 102,
                group: 1,
                update_every: 1,
                min: 0.0,
                max: 0.0,
                flags: 0,
            },
            queries_count: 2,
            result_points_generated: 6,
            db_points_read: 6,
            cardinality_folded: 0,
            cardinality_cut: 0.0,
            ..Rrdr::new(&crate::window::Window::default(), 0)
        }
    }

    fn text(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut out = Vec::new();
        f(&mut out);
        String::from_utf8(out).unwrap()
    }

    fn date(t: i64) -> String {
        let tm = localtime(t).unwrap();
        text(|o| print_date(o, tm.year, tm.month0 + 1, tm.mday, tm.hour, tm.min, tm.sec))
    }

    #[test]
    fn json_family_matches_the_spec() {
        let r = rrdr();
        assert_eq!(
            text(|o| rrdr2json(&r, o, 0, false)),
            "{\n        \"labels\":[\"time\",\"A\",\"B\"],\n        \"data\":[\n            [102,3,30.5],\n            \
             [101,2,null],\n            [100,1,10]\n        ]\n    }"
        );
        let objects = text(|o| rrdr2json(&r, o, options::OBJECTSROWS | options::NULL2ZERO, false));
        assert!(
            objects.contains("            {\"time\": 102,\"A\": 3,\"B\": 30.5},\n            {\"time\": 101,\"A\": 2,\"B\": 0},"),
            "{objects}"
        );
        let tm = localtime(102).unwrap();
        let jsdate =
            text(|o| print_jsdate(o, tm.year, tm.month0, tm.mday, tm.hour, tm.min, tm.sec));
        let table = text(|o| rrdr2json(&r, o, 0, true));
        assert!(
            table.contains(&format!(
                "        {{\"c\":[{{\"v\":\"{jsdate}\"}},{{\"v\":null}},{{\"v\":null}},{{\"v\":3}},{{\"v\":30.5}}]}}"
            )),
            "{table}"
        );
        let google = text(|o| rrdr2json(&r, o, options::GOOGLE_JSON, true));
        assert!(
            google.starts_with(
                "{\n cols:\n [\n        {id:'',label:'time',pattern:'',type:'datetime'},"
            ),
            "{google}"
        );
        let reversed = text(|o| rrdr2json(&r, o, options::REVERSED | options::MILLISECONDS, false));
        assert!(
            reversed.contains("[100000,1,10],\n            [101000,2,null]"),
            "{reversed}"
        );
    }

    #[test]
    fn a_reset_row_is_annotated_and_hidden_columns_are_skipped() {
        let mut r = rrdr();
        r.o[2] |= value_flags::RESET;
        r.od[1] |= metric_status::HIDDEN;
        let table = text(|o| rrdr2json(&r, o, 0, true));
        assert!(table.contains("{\"v\":\"RESET OR OVERFLOW\"},{\"v\":\"The counters have been wrapped.\"},{\"v\":2}]}"), "{table}");
        assert!(!table.contains("\"B\""), "{table}");
        r.od[0] |= metric_status::HIDDEN;
        assert_eq!(
            text(|o| rrdr2json(&r, o, 0, false)),
            "{\n        \"labels\":[\"time\",\"no data\"],\n        \"data\":[\n\n        ]\n    }"
        );
        assert_eq!(
            text(|o| rrdr2csv(&r, o, Format::Csv, 0, "", ",", "\r\n", "")),
            ""
        );
        // raw exposes a queried column even when hidden
        assert!(text(|o| rrdr2json(&r, o, options::RETURN_RAW, false)).contains("\"A\",\"B\""));
    }

    #[test]
    fn csv_family_matches_the_spec() {
        let r = rrdr();
        assert_eq!(
            text(|o| rrdr2csv(&r, o, Format::Csv, 0, "", ",", "\r\n", "")),
            format!(
                "time,A,B\r\n{},3,30.5\r\n{},2,null\r\n{},1,10\r\n",
                date(102),
                date(101),
                date(100)
            )
        );
        assert!(
            text(|o| rrdr2csv(&r, o, Format::Markdown, 0, "", "|", "\r\n", ""))
                .starts_with("time|A|B\r\n:---:|:---:|:---:\r\n")
        );
        let csv_options = options::LABEL_QUOTES | options::SECONDS;
        assert_eq!(
            text(|o| rrdr2csv(
                &r,
                o,
                Format::CsvJsonArray,
                csv_options,
                "[",
                ",",
                "]",
                ",\n"
            )),
            "[\"time\",\"A\",\"B\"],\n[102,3,30.5],\n[101,2,null],\n[100,1,10]"
        );
    }

    #[test]
    fn ssv_reduces_each_row_and_resets_min_max() {
        let mut r = rrdr();
        assert_eq!(text(|o| rrdr2ssv(&mut r, o, 0, "", " ", "")), "33.5 2 11");
        assert_eq!((r.view.min, r.view.max), (2.0, 33.5));
        assert_eq!(
            text(|o| rrdr2ssv(&mut r, o, options::DIMS_AVERAGE, "[", ",", "]")),
            "[16.75,2,5.5]"
        );
        assert_eq!(
            text(|o| rrdr2ssv(&mut r, o, options::DIMS_MIN2MAX, "", ",", "")),
            "27.5,0,9"
        );
        r.od[0] |= metric_status::HIDDEN;
        assert_eq!(
            text(|o| rrdr2ssv(&mut r, o, options::NULL2ZERO, "", ",", "")),
            "30.5,0,10"
        );
        assert!(r.view.min == 0.0 && r.view.max == 30.5);
    }

    #[test]
    fn json2_rows_carry_value_anomaly_rate_and_annotations() {
        let r = rrdr();
        let mut w = JsonWriter::new(netdata_agent_text::json::JsonOptions::MINIFY);
        rrdr2json_v2(&r, &mut w, options::REVERSED);
        w.finalize();
        assert_eq!(
            String::from_utf8(w.into_bytes()).unwrap(),
            "{\"result\":{\"labels\":[\"time\",\"a\",\"b\"],\"point\":{\"value\":0,\"arp\":1,\"pa\":2},\
             \"data\":[[100,[1,0,0],[10,0,0]],[101,[2,0,0],[null,0,1]],[102,[3,0,0],[30.5,0,0]]]}}"
        );
    }
}
