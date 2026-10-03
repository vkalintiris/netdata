//! `netdata-api-calls` (`src/libnetdata/query_progress/progress.c` `progress_function_result()`): the progress
//! table's rows in C's order, rendered under its lock, then their columns.

use netdata_agent_nrpc::reply::{ContentType, Reply};
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::rrdf::{self, Field, FieldType, Filter, Summary, Transform, Visual, opts, sort};
use netdata_agent_web::progress::{RowView, Table, Transaction};

use super::PROGRESS_HELP;
use crate::access_log::{logged_url, mode_name};
use crate::acl;

/// A row's severity (`progress_function_result()`'s row options).
fn severity(row: &RowView<'_>) -> &'static str {
    if row.finished_ut == 0 {
        return "notice";
    }
    match row.response_code {
        304 | 409 | 499 => "debug",
        500..=599 => "error",
        400..=499 => "warning",
        300..=399 => "notice",
        _ => "normal",
    }
}

/// A row's twelve cells.
fn row(w: &mut JsonWriter, tx: &Transaction, row: &RowView<'_>, duration_ut: u64) {
    let finished = row.finished_ut != 0;
    w.add_array_item_array();
    w.add_array_item_uuid_compact(Some(tx));
    w.add_array_item_uint64(row.started_ut);
    w.add_array_item_string(mode_name(row.mode));
    // the STREAM key masked, as in the access log (D176.7, D31); C shows it raw
    w.add_array_item_string(logged_url(row.query, row.mode));
    if !row.client.is_empty() {
        w.add_array_item_string(row.client);
    } else if row.acl & acl::bits::ACLK != 0 {
        w.add_array_item_string("ACLK");
    } else if row.acl & acl::bits::WEBRTC != 0 {
        w.add_array_item_string("WEBRTC");
    } else {
        w.add_array_item_string("unknown");
    }
    if finished {
        w.add_array_item_string("finished");
        // not a format string in C: its two percent signs print
        w.add_array_item_string("100.00 %%");
    } else {
        w.add_array_item_string("in-progress");
        let progress = if row.all != 0 {
            format!("{:.2} %", row.done as f64 * 100.0 / row.all as f64)
        } else {
            row.done.to_string()
        };
        w.add_array_item_string(progress);
    }
    w.add_array_item_double(duration_ut as f64 / 1000.0);
    if finished {
        w.add_array_item_uint64(u64::from(row.response_code));
        w.add_array_item_uint64(u64::from(row.response_size));
        w.add_array_item_uint64(u64::from(row.sent_size));
    } else {
        w.add_array_item_null();
        w.add_array_item_null();
        w.add_array_item_null();
    }
    w.add_array_item_object();
    w.member_add_string("severity", severity(row));
    w.object_close();
    w.array_close();
}

/// A column with C's defaults for this table: no units, no maximum, no pointer, no default value.
fn column<'a>(id: usize, key: &'a str, name: &'a str, kind: FieldType, transform: Transform) -> Field<'a> {
    Field {
        id,
        key: key.as_bytes(),
        name: name.as_bytes(),
        kind,
        visual: Visual::Value,
        transform,
        decimal_points: 0,
        units: None,
        max: f64::NAN,
        sort: sort::ASCENDING,
        pointer_to: None,
        summary: Summary::Count,
        filter: Filter::None,
        options: opts::VISIBLE,
        default_value: None,
    }
}

/// `progress_function_result()`: pretty JSON, not cacheable; `expires` a second past the clock read under the lock.
pub(super) fn render(table: &Table, reply: &mut Reply, hostname: &str) -> u16 {
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_string("hostname", hostname);
    w.member_add_uint64("status", 200);
    w.member_add_string("type", "table");
    w.member_add_time_t("update_every", 1);
    w.member_add_boolean("has_history", false);
    w.member_add_string("help", PROGRESS_HELP);
    w.member_add_array(Some(b"data"));
    let (now_ut, max_duration_ut, max_size, max_sent) = table.visit(|now_ut, rows| {
        let (mut max_duration_ut, mut max_size, mut max_sent) = (0u64, 0u32, 0u32);
        for (tx, r) in rows {
            let finished = r.finished_ut != 0;
            // C's unsigned subtraction
            let duration_ut = if finished { r.duration_ut } else { now_ut.wrapping_sub(r.started_ut) };
            max_duration_ut = max_duration_ut.max(duration_ut);
            if finished {
                max_size = max_size.max(r.response_size);
                max_sent = max_sent.max(r.sent_size);
            }
            row(&mut w, tx, &r, duration_ut);
        }
        (now_ut, max_duration_ut, max_size, max_sent)
    });
    w.array_close();
    w.member_add_object("columns");
    let columns = [
        Field { options: opts::VISIBLE | opts::UNIQUE_KEY, ..column(0, "Transaction", "Transaction ID", FieldType::String, Transform::None) },
        Field {
            sort: sort::DESCENDING,
            summary: Summary::Max,
            ..column(1, "Started", "Query Start Timestamp", FieldType::Timestamp, Transform::DatetimeUsec)
        },
        Field { filter: Filter::MultiSelect, ..column(2, "Method", "Request Method", FieldType::String, Transform::None) },
        Field {
            options: opts::VISIBLE | opts::FULL_WIDTH | opts::WRAP,
            ..column(3, "Query", "Query", FieldType::String, Transform::None)
        },
        Field { filter: Filter::MultiSelect, ..column(4, "Client", "Client", FieldType::String, Transform::None) },
        Field { filter: Filter::MultiSelect, ..column(5, "Status", "Query Status", FieldType::String, Transform::None) },
        Field { sort: sort::DESCENDING, ..column(6, "Progress", "Query Progress", FieldType::String, Transform::None) },
        Field {
            decimal_points: 2,
            units: Some(b"ms"),
            max: max_duration_ut as f64 / 1000.0,
            sort: sort::DESCENDING,
            summary: Summary::Max,
            filter: Filter::Range,
            ..column(7, "Duration", "Query Duration", FieldType::Duration, Transform::DurationS)
        },
        Field {
            sort: sort::DESCENDING,
            filter: Filter::MultiSelect,
            ..column(8, "Response", "Query Response Code", FieldType::Integer, Transform::None)
        },
        Field {
            units: Some(b"bytes"),
            max: f64::from(max_size),
            sort: sort::DESCENDING,
            summary: Summary::Sum,
            filter: Filter::Range,
            options: opts::NONE,
            ..column(9, "Size", "Query Response Size", FieldType::Integer, Transform::None)
        },
        Field {
            units: Some(b"bytes"),
            max: f64::from(max_sent),
            sort: sort::DESCENDING,
            summary: Summary::Sum,
            filter: Filter::Range,
            options: opts::NONE,
            ..column(10, "Sent", "Query Response Final Size", FieldType::Integer, Transform::None)
        },
        Field {
            visual: Visual::RowOptions,
            sort: sort::FIXED,
            options: opts::DUMMY,
            ..column(11, "rowOptions", "rowOptions", FieldType::None, Transform::None)
        },
    ];
    for field in &columns {
        rrdf::add_field(&mut w, field);
    }
    w.object_close();
    w.member_add_string("default_sort_column", "Started");
    w.member_add_time_t("expires", (now_ut / 1_000_000) as i64 + 1);
    w.finalize();
    // buffer_json_initialize(): JSON, not cacheable
    reply.body = w.into_bytes();
    reply.content_type = ContentType::ApplicationJson;
    reply.expires = 0;
    reply.cacheable = false;
    200
}
