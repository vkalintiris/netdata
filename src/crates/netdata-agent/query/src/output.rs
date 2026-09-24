//! Executing a query target and rendering it in the requested format (`data_query_execute()`,
//! `src/web/api/formatters/rrd2json.c`). Spec §9.1.

use netdata_agent_text::json::JsonWriter;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::status;

use crate::execute::{Control, run_v1};
use crate::format::{rrdr2csv, rrdr2json, rrdr2json_v2, rrdr2ssv};
use crate::jsonwrap::{begin_v1, end_v1};
use crate::rrdr::{Rrdr, result_flags};
use crate::tables::{Format, options};
use crate::target::QueryTarget;
use crate::window::Window;

/// What `data_query_execute()` leaves for the handler.
#[derive(Debug)]
pub struct DataResponse {
    pub code: u16,
    pub content_type: ContentType,
    pub body: Vec<u8>,
    /// `buffer_cacheable()` (true) or `buffer_no_cacheable()` (false) from the result's time flags.
    pub cacheable: Option<bool>,
    /// `*latest_timestamp`: the result's `before` when it has rows.
    pub latest_timestamp: Option<i64>,
}

/// A wrapped result whose value the writer quotes: `buffer_json_member_add_string_open/close()` around raw text.
fn wrapped_string(w: &mut JsonWriter, render: impl FnOnce(&mut Vec<u8>)) {
    w.member_add_key_only("result");
    let quote = w.value_quote().to_vec();
    w.buffer_mut().extend_from_slice(&quote);
    render(w.buffer_mut());
    w.buffer_mut().extend_from_slice(&quote);
}

/// A wrapped result that is a JSON array filled with raw text.
fn wrapped_array(w: &mut JsonWriter, render: impl FnOnce(&mut Vec<u8>)) {
    w.member_add_array(Some(b"result"));
    render(w.buffer_mut());
    w.array_close();
}

/// `data_query_execute()` for a v1 query: executes `qt` and renders the result. `window.options` loses NONZERO
/// when no executed metric is nonzero, before rendering.
pub fn data_query_execute(
    qt: &mut QueryTarget,
    window: &mut Window,
    control: &Control,
) -> DataResponse {
    let mut r = run_v1(qt, window, control);
    // A cancelled query leaves the response's initial content type.
    let mut response = DataResponse {
        code: status::OK,
        content_type: ContentType::TextPlain,
        body: Vec::new(),
        cacheable: None,
        latest_timestamp: None,
    };
    if r.view.flags & result_flags::CANCEL != 0 {
        response.code = status::CLIENT_CLOSED_REQUEST;
        return response;
    }
    if r.view.flags & result_flags::RELATIVE != 0 {
        response.cacheable = Some(false);
    } else if r.view.flags & result_flags::ABSOLUTE != 0 {
        response.cacheable = Some(true);
    }
    if r.rows > 0 {
        response.latest_timestamp = Some(r.view.before);
    }
    let format = qt.request.format;
    let options = window.options;
    let wrap = options & options::JSON_WRAP != 0;
    let received = control.received;
    let wrapped =
        |r: &mut Rrdr, qt: &QueryTarget, render: &mut dyn FnMut(&mut JsonWriter, &mut Rrdr)| {
            let mut w = begin_v1(r, qt, options);
            render(&mut w, r);
            end_v1(&mut w, r, qt, received);
            w.into_bytes()
        };
    let ssv = |separator: &'static str| {
        move |r: &mut Rrdr, out: &mut Vec<u8>| rrdr2ssv(r, out, options, "", separator, "")
    };
    let (content_type, body) = match format {
        Format::Ssv | Format::SsvComma => {
            let render = ssv(if format == Format::Ssv { " " } else { "," });
            if wrap {
                let body = wrapped(&mut r, qt, &mut |w, r| {
                    wrapped_string(w, |out| render(r, out))
                });
                (ContentType::ApplicationJson, body)
            } else {
                let mut body = Vec::new();
                render(&mut r, &mut body);
                (ContentType::TextPlain, body)
            }
        }
        Format::Array => {
            if wrap {
                let render = ssv(",");
                let body = wrapped(&mut r, qt, &mut |w, r| {
                    wrapped_array(w, |out| render(r, out))
                });
                (ContentType::ApplicationJson, body)
            } else {
                let mut body = Vec::new();
                rrdr2ssv(&mut r, &mut body, options, "[", ",", "]");
                (ContentType::ApplicationJson, body)
            }
        }
        Format::Csv | Format::Markdown | Format::Tsv => {
            let separator = match format {
                Format::Csv => ",",
                Format::Markdown => "|",
                _ => "\t",
            };
            if wrap {
                let body = wrapped(&mut r, qt, &mut |w, r| {
                    wrapped_string(w, |out| {
                        rrdr2csv(r, out, format, options, "", separator, "\\n", "")
                    })
                });
                (ContentType::ApplicationJson, body)
            } else {
                let mut body = Vec::new();
                rrdr2csv(&r, &mut body, format, options, "", separator, "\r\n", "");
                (ContentType::TextPlain, body)
            }
        }
        Format::CsvJsonArray => {
            // JSON has no date type: this format always prints numeric times.
            let mut csv_options = options | options::LABEL_QUOTES;
            if csv_options & options::MILLISECONDS == 0 {
                csv_options |= options::SECONDS;
            }
            let render = |r: &Rrdr, out: &mut Vec<u8>| {
                rrdr2csv(r, out, format, csv_options, "[", ",", "]", ",\n")
            };
            let body = if wrap {
                wrapped(&mut r, qt, &mut |w, r| {
                    wrapped_array(w, |out| render(r, out))
                })
            } else {
                let mut body = b"[\n".to_vec();
                render(&r, &mut body);
                body.extend_from_slice(b"\n]");
                body
            };
            (ContentType::ApplicationJson, body)
        }
        Format::Html => {
            if wrap {
                let body = wrapped(&mut r, qt, &mut |w, r| {
                    wrapped_string(w, |out| {
                        out.extend_from_slice(
                            b"<html>\\n<center>\\n<table border=\\\"0\\\" cellpadding=\\\"5\\\" cellspacing=\\\"5\\\">\\n",
                        );
                        rrdr2csv(
                            r,
                            out,
                            format,
                            options,
                            "<tr><td>",
                            "</td><td>",
                            "</td></tr>\\n",
                            "",
                        );
                        out.extend_from_slice(b"</table>\\n</center>\\n</html>\\n");
                    })
                });
                (ContentType::ApplicationJson, body)
            } else {
                let mut body =
                    b"<html>\n<center>\n<table border=\"0\" cellpadding=\"5\" cellspacing=\"5\">\n"
                        .to_vec();
                rrdr2csv(
                    &r,
                    &mut body,
                    format,
                    options,
                    "<tr><td>",
                    "</td><td>",
                    "</td></tr>\n",
                    "",
                );
                body.extend_from_slice(b"</table>\n</center>\n</html>\n");
                (ContentType::TextHtml, body)
            }
        }
        Format::Datasource | Format::Datatable | Format::Jsonp | Format::Json => {
            let datatable = matches!(format, Format::Datasource | Format::Datatable);
            // The wrapper's buffer_json_initialize() makes any wrapped response JSON.
            let content_type = if !wrap && matches!(format, Format::Datasource | Format::Jsonp) {
                ContentType::ApplicationXJavascript
            } else {
                ContentType::ApplicationJson
            };
            let body = if wrap {
                wrapped(&mut r, qt, &mut |w, r| {
                    w.member_add_key_only("result");
                    rrdr2json(r, w.buffer_mut(), options, datatable);
                    if format == Format::Json && options & options::RETURN_JWAR != 0 {
                        w.member_add_key_only("anomaly_rates");
                        rrdr2json(r, w.buffer_mut(), options | options::INTERNAL_AR, false);
                    }
                })
            } else {
                let mut body = Vec::new();
                rrdr2json(&r, &mut body, options, datatable);
                body
            };
            (content_type, body)
        }
        Format::Json2 => {
            let body = wrapped(&mut r, qt, &mut |w, r| rrdr2json_v2(r, w, options));
            (ContentType::ApplicationJson, body)
        }
    };
    response.content_type = content_type;
    response.body = body;
    response
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::testing::{T0, host, v1_target};

    fn respond(query: &str) -> DataResponse {
        let h = host();
        let (mut qt, mut window) = v1_target(&h, query);
        let control = Control {
            received: Instant::now(),
            interrupted: &|| false,
        };
        data_query_execute(&mut qt, &mut window, &control)
    }

    #[test]
    fn jsonwrap_v1_members_follow_c() {
        let r = respond(&format!("after={T0}&before={}&options=jsonwrap", T0 + 6));
        let body = String::from_utf8(r.body).unwrap();
        let id = format!(
            "context://hosts:child/contexts:ctx.a/instances:*/dimensions:*/after:{T0}/before:{}/points:0/group:average\
             /options:jsonwrap",
            T0 + 6
        );
        let head = format!(
            "{{\n    \"api\":1,\n    \"id\":\"{id}\",\n    \"name\":\"{id}\",\n    \"view_update_every\":1,\n    \
             \"update_every\":1,\n    \"first_entry\":{T0},\n    \"last_entry\":{last},\n    \"after\":{T0},\n    \
             \"before\":{last},\n    \"group\":\"average\",\n    \"options\":[\"jsonwrap\",\"natural-points\"],\n    \
             \"dimension_names\":[\"d\"],\n    \"dimension_ids\":[\"d\"],\n    \"functions\":[],\n    \
             \"chart_ids\":[\"t.a\"],\n    \"latest_values\":[0],\n    \"view_latest_values\":[40],\n    \
             \"dimensions\":1,\n    \"points\":7,\n    \"format\":\"json\",\n    \"db_points_per_tier\":[7],\n    \
             \"result\":{{\n        \"labels\":[\"time\",\"d\"],\n        \"data\":[\n            [{last},40],\n",
            last = T0 + 6
        );
        assert!(body.starts_with(&head), "{body}");
        assert!(
            body.contains("            [1700000000,null]\n        ]\n    },\n    \"min\":0,\n    \"max\":40,\n    \"timings\":{\n        \"prep_ms\":"),
            "{body}"
        );
        assert!(body.ends_with("\n    }\n}\n"), "{body}");
        assert_eq!(
            (r.code, r.content_type, r.cacheable),
            (200, ContentType::ApplicationJson, Some(true))
        );
        assert_eq!(r.latest_timestamp, Some(T0 + 6));
    }

    #[test]
    fn wrapped_ssv_reports_the_reduced_min_max() {
        let r = respond(&format!(
            "after={T0}&before={}&points=3&format=ssv&options=jsonwrap,minify",
            T0 + 6
        ));
        let body = String::from_utf8(r.body).unwrap();
        assert!(
            body.contains(",\"result\":\"40 25 10\",\"min\":10,\"max\":40,"),
            "{body}"
        );
        let r = respond(&format!(
            "after={T0}&before={}&points=3&format=array",
            T0 + 6
        ));
        assert_eq!(
            (r.body.as_slice(), r.content_type),
            (&b"[40,25,10]"[..], ContentType::ApplicationJson)
        );
        let r = respond(&format!(
            "after={T0}&before={}&points=3&format=ssvcomma",
            T0 + 6
        ));
        assert_eq!(
            (r.body.as_slice(), r.content_type),
            (&b"40,25,10"[..], ContentType::TextPlain)
        );
    }

    #[test]
    fn formats_pick_their_content_types() {
        let q = format!("after={T0}&before={}&points=2", T0 + 6);
        let cases = [
            ("csv", ContentType::TextPlain),
            ("tsv", ContentType::TextPlain),
            ("markdown", ContentType::TextPlain),
            ("html", ContentType::TextHtml),
            ("datatable", ContentType::ApplicationJson),
            ("datasource", ContentType::ApplicationXJavascript),
            ("jsonp", ContentType::ApplicationXJavascript),
            ("csvjsonarray", ContentType::ApplicationJson),
            ("json2", ContentType::ApplicationJson),
        ];
        for (format, content_type) in cases {
            let r = respond(&format!("{q}&format={format}"));
            assert_eq!(r.content_type, content_type, "{format}");
        }
        let r = respond(&format!("{q}&format=csvjsonarray"));
        assert_eq!(
            String::from_utf8(r.body).unwrap(),
            format!("[\n[\"time\",\"d\"],\n[{},40],\n[{},25]\n]", T0 + 7, T0 + 4)
        );
        let r = respond(&format!("{q}&format=csv&options=jsonwrap,seconds,minify"));
        let body = String::from_utf8(r.body).unwrap();
        assert!(
            body.contains(&format!(
                ",\"result\":\"time,d\\n{},40\\n{},25\\n\",",
                T0 + 7,
                T0 + 4
            )),
            "{body}"
        );
        assert_eq!(r.content_type, ContentType::ApplicationJson);
    }

    #[test]
    fn a_cancelled_query_answers_499_without_a_body() {
        let r = respond(&format!("after={T0}&before={}&timeout=-1", T0 + 6));
        assert_eq!((r.code, r.body.len(), r.cacheable), (499, 0, None));
        assert_eq!(r.content_type, ContentType::TextPlain);
    }
}
