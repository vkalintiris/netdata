//! `stream_sender_execute_commands()` (`src/streaming/stream-sender-execute.c`): what a parent sends down to its
//! child, line by line with C-string semantics, including the deferred bodies of `FUNCTION_PAYLOAD` and `JSON`.
//! Map: `knowledge/map-m7-commit4-runtime.md` §5.

use std::sync::Arc;

use netdata_agent_log::{Field, Priority, Source, Value, nd_log, push};
use netdata_agent_nrpc::call::{CallSpec, Calls, Done, ProgressCb};
use netdata_agent_nrpc::reply::{ContentType, Payload, Reply};
use netdata_agent_pluginsd_proto::{MAX_DEFERRED_SIZE, Words};
use netdata_agent_text::c::c_str;
use netdata_agent_text::print::print_uuid_lower_compact;

use super::dispatch::Dispatched;
use super::{Traffic, shown, text};
use crate::caps;

const FUNCTION_PAYLOAD_END: &str = "FUNCTION_PAYLOAD_END";
const JSON_PAYLOAD_END: &str = "JSON_PAYLOAD_END";

/// What a deferred body runs at its end keyword.
#[derive(Debug)]
enum Action {
    /// `execute_deferred_function()`: missing words are empty; the body's content type is its sixth word's.
    Function {
        transaction: Vec<u8>,
        timeout: Vec<u8>,
        function: Vec<u8>,
        access: Vec<u8>,
        source: Vec<u8>,
        content_type: ContentType,
    },
    /// `execute_deferred_json()`.
    Json { keyword: Vec<u8> },
}

/// `s->thread.defer`.
#[derive(Debug)]
struct Deferred {
    end_keyword: &'static str,
    payload: Vec<u8>,
    action: Action,
}

/// The executor's state of a dispatched connection.
#[derive(Debug, Default)]
pub(crate) struct Executor {
    defer: Option<Deferred>,
}

/// `stream_sender_defer_payload_append()`: false when the body passes 100 MiB, after C's record.
fn append(d: &mut Dispatched, line: &[u8], newline: bool) -> bool {
    let hostname = d.host.hostname();
    let Some(defer) = d.executor.defer.as_mut() else {
        return true;
    };
    let add = line.len() + usize::from(newline);
    let current = defer.payload.len();
    if add > MAX_DEFERRED_SIZE || current > MAX_DEFERRED_SIZE - add {
        // under C's REQUEST callback, which has no words here and leaves its separator
        let _request = request_field(None);
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "STREAM SND '{hostname}' [to {}]: deferred payload is too big ({} bytes, limit {MAX_DEFERRED_SIZE} bytes) \
             while waiting for keyword '{}'. Restarting sender connection.",
            d.remote_ip,
            current.saturating_add(add),
            defer.end_keyword
        );
        return false;
    }
    defer.payload.extend_from_slice(line);
    if newline {
        defer.payload.push(b'\n');
    }
    true
}

/// `stream_sender_execute_commands()` over the bytes received: complete lines run, the rest waits. False when the
/// connection must restart (a deferred body too big).
pub(crate) fn execute(d: &mut Dispatched) -> bool {
    let end = d.read_len;
    let mut start = 0;
    while start < end {
        // strchr() over the NUL-terminated buffer: a NUL ends the scan as the buffer's end does
        let rest = &d.rbuf[start..end];
        let stop = rest.iter().position(|&c| c == b'\n' || c == 0);
        let Some(at) = stop.filter(|&at| rest[at] == b'\n') else {
            if let Some(defer) = &d.executor.defer {
                // stream_sender_defer_is_end_keyword_prefix() over the whole remainder
                if defer.end_keyword.as_bytes().starts_with(rest) {
                    break;
                }
                let line = rest.to_vec();
                if !append(d, &line, false) {
                    return false;
                }
                start = end;
            }
            break;
        };
        let line = rest[..at].to_vec();
        start += at + 1;
        if let Some(defer) = &d.executor.defer {
            if line == defer.end_keyword.as_bytes() {
                if let Some(defer) = d.executor.defer.take() {
                    run_deferred(d, defer);
                }
            } else if !append(d, &line, true) {
                return false;
            }
            continue;
        }
        command(d, &line);
    }
    if start < end {
        d.rbuf.copy_within(start..end, 0);
        d.read_len = end - start;
    } else {
        d.read_len = 0;
    }
    true
}

/// `ND_LOG_FIELD_CB(NDF_REQUEST, line_splitter_reconstruct_line, ...)`: the line's words, or none (the callback
/// returns false and C's logfmt keeps the field's separator).
fn request_field(words: Option<Vec<u8>>) -> netdata_agent_log::FrameGuard {
    push(vec![(
        Field::Request,
        Value::lazy(move |out| match &words {
            Some(w) => {
                out.extend_from_slice(w);
                true
            }
            None => false,
        }),
    )])
}

/// The body's action at its end keyword; the records carry no REQUEST (the words were reset after its first line).
fn run_deferred(d: &mut Dispatched, defer: Deferred) {
    let _request = request_field(None);
    match defer.action {
        Action::Function { transaction, timeout, function, access, source, content_type } => {
            let call = Call {
                command: FUNCTION_PAYLOAD_END,
                transaction: Some(&transaction),
                timeout: Some(&timeout),
                function: Some(&function),
                access: Some(&access),
                source: Some(&source),
            };
            execute_function(d, &call, Some(Payload { body: defer.payload, content_type }));
        }
        Action::Json { keyword } => {
            if keyword == b"STREAM_PATH" {
                // stream_path_set_from_json(…, from_parent = true): a changed path goes to the child only
                if netdata_agent_ingest::stream_path::set_from_json(&d.host, &defer.payload)
                    && let Some(localhost) = d.sender.connector.localhost()
                {
                    netdata_agent_ingest::stream_path::send_to_child(&d.host, &localhost);
                }
            } else {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "STREAM SND '{}' [to {}]: unknown JSON keyword '{}' with payload: {}",
                    d.host.hostname(),
                    d.remote_ip,
                    text(&keyword),
                    text(c_str(&defer.payload))
                );
            }
        }
    }
}

/// One command line, under the REQUEST field of its words.
fn command(d: &mut Dispatched, line: &[u8]) {
    let words = Words::split_whitespace(line);
    let _request = request_field((!words.is_empty()).then(|| words.reconstruct()));
    let word = |i: usize| words.get(i).map(<[u8]>::to_vec);
    let hostname = d.host.hostname();
    match words.get(0) {
        Some(b"FUNCTION") => {
            let call = Call {
                command: "FUNCTION",
                transaction: words.get(1),
                timeout: words.get(2),
                function: words.get(3),
                access: words.get(4),
                source: words.get(5),
            };
            execute_function(d, &call, None);
        }
        Some(b"FUNCTION_PAYLOAD") => {
            d.executor.defer = Some(Deferred {
                end_keyword: FUNCTION_PAYLOAD_END,
                payload: Vec::new(),
                action: Action::Function {
                    transaction: word(1).unwrap_or_default(),
                    timeout: word(2).unwrap_or_default(),
                    function: word(3).unwrap_or_default(),
                    access: word(4).unwrap_or_default(),
                    source: word(5).unwrap_or_default(),
                    content_type: ContentType::from_name(words.get(6).unwrap_or_default()),
                },
            });
        }
        Some(b"FUNCTION_CANCEL") | Some(b"FUNCTION_PROGRESS") => {
            netdata_agent_log::logger(Source::Access, Priority::Debug, 0, &netdata_agent_log::here!(), None);
            if let Some(transaction) = words.get(1).filter(|t| !t.is_empty()) {
                let transaction = String::from_utf8_lossy(transaction);
                if words.get(0) == Some(b"FUNCTION_CANCEL") {
                    Calls::process().cancel(&transaction);
                } else {
                    Calls::process().progress(&transaction);
                }
            }
        }
        Some(b"REPLAY_CHART") => {
            d.sender.replication_counter_in();
            let (chart, start, after, before) = (words.get(1), words.get(2), words.get(3), words.get(4));
            let (Some(chart), Some(start), Some(after), Some(before)) = (chart, start, after, before) else {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "STREAM REPLAY ERROR '{hostname}' [send to {}] REPLAY_CHART command is incomplete (chart={}, \
                     start_streaming={}, after={}, before={})",
                    d.remote_ip,
                    shown(words.get(1)),
                    shown(words.get(2)),
                    shown(words.get(3)),
                    shown(words.get(4))
                );
                return;
            };
            let (after, before) = match (replay_endpoint(after), replay_endpoint(before)) {
                (Some(a), Some(b)) => (a, b),
                _ => (0, 0),
            };
            let start = netdata_agent_ingest::parse_enable_streaming(Some(start));
            let chart = String::from_utf8_lossy(chart).into_owned();
            d.sender.connector.replication().request_add(&d.sender, chart, after, before, start);
        }
        Some(b"NODE_ID") => {
            let (claim, node, url) = (word(1), word(2), word(3));
            super::hooks::node_and_claim_id_from_parent(d, claim.as_deref(), node.as_deref(), url.as_deref());
        }
        Some(b"JSON") => {
            d.executor.defer = Some(Deferred {
                end_keyword: JSON_PAYLOAD_END,
                payload: Vec::new(),
                action: Action::Json { keyword: word(1).unwrap_or_default() },
            });
        }
        first => {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM SND '{hostname}' [to {}] received unknown command over connection: {}",
                d.remote_ip,
                first.map_or_else(|| "(unset)".to_string(), text)
            );
        }
    }
}

/// `stream_sender_parse_replay_endpoint()`: a strict unsigned decimal within `time_t`.
fn replay_endpoint(value: &[u8]) -> Option<i64> {
    if !value.first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let (v, used, erange) = netdata_agent_text::parse::strtoull10(value);
    (!erange && used == value.len() && v <= i64::MAX as u64).then_some(v as i64)
}

/// A parent's call as its words came.
struct Call<'a> {
    /// The keyword the incomplete record names (`FUNCTION`, or the payload's end keyword).
    command: &'a str,
    transaction: Option<&'a [u8]>,
    timeout: Option<&'a [u8]>,
    function: Option<&'a [u8]>,
    access: Option<&'a [u8]>,
    source: Option<&'a [u8]>,
}

/// `execute_commands_function()`: an access record, then the call on this host's methods, without waiting (a parent
/// may call restricted ones; a timeout that is not positive is 10 s); the answer goes up as the parent's transaction,
/// and the plugin's progress with PROGRESS, each while the host's metadata may stream.
fn execute_function(d: &mut Dispatched, call: &Call<'_>, payload: Option<Payload>) {
    netdata_agent_log::logger(Source::Access, Priority::Info, 0, &netdata_agent_log::here!(), None);
    let set = |w: Option<&[u8]>| w.is_some_and(|w| !w.is_empty());
    if !set(call.transaction) || !set(call.timeout) || !set(call.function) {
        let shown = |w: Option<&[u8]>| w.map_or_else(|| "(unset)".to_string(), text);
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "STREAM SND '{}' [to {}]: {} execution command is incomplete (transaction = '{}', timeout = '{}', \
             function = '{}'). Ignoring it.",
            d.host.hostname(),
            d.remote_ip,
            call.command,
            shown(call.transaction),
            shown(call.timeout),
            shown(call.function)
        );
        return;
    }
    let transaction = call.transaction.unwrap_or_default();
    let timeout_s = match netdata_agent_text::parse::str2i(call.timeout.unwrap_or_default()) {
        t if t <= 0 => netdata_agent_nrpc::TIMEOUT_DEFAULT,
        t => t,
    };
    // stream_execute_function_callback(): the transaction as it came
    let done: Done = {
        let (sender, host, transaction) = (Arc::clone(&d.sender), Arc::clone(&d.host), transaction.to_vec());
        Box::new(move |reply: Reply, code| {
            if !host.can_stream_metadata() {
                return;
            }
            let mut out = b"FUNCTION_RESULT_BEGIN \"".to_vec();
            out.extend_from_slice(&transaction);
            out.extend_from_slice(
                format!("\" {code} \"{}\" {}\n", reply.content_type.name(), reply.expires).as_bytes(),
            );
            out.extend_from_slice(&reply.body);
            out.extend_from_slice(b"\nFUNCTION_RESULT_END\n");
            sender.commit(&out, Traffic::Functions);
        })
    };
    // stream_execute_function_progress_callback(), a pair with its data: only to a parent that takes PROGRESS
    let progress: Option<ProgressCb> = (d.capabilities & caps::PROGRESS != 0).then(|| {
        let (sender, host) = (Arc::clone(&d.sender), Arc::clone(&d.host));
        Arc::new(move |call_id: &[u8; 16], done: usize, all: usize| {
            if !host.can_stream_metadata() {
                return;
            }
            let mut out = b"FUNCTION_PROGRESS '".to_vec();
            print_uuid_lower_compact(&mut out, call_id);
            out.extend_from_slice(format!("' {done} {all}\n").as_bytes());
            sender.commit(&out, Traffic::Functions);
        }) as ProgressCb
    });
    let hostname = d.host.hostname();
    Calls::process().call(CallSpec {
        owner: Some((d.host.functions(), &hostname)),
        cmd: call.function.unwrap_or_default(),
        source: call.source.unwrap_or_default(),
        user_access: netdata_agent_nrpc::access::from_hex_mapping_old_roles(call.access.unwrap_or_default()),
        timeout_s,
        wait: false,
        allow_restricted: true,
        call_id: Some(transaction),
        payload,
        reply: Reply::new(ContentType::TextPlain),
        done: Some(done),
        progress,
        is_cancelled: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_endpoints_are_strict_decimals() {
        assert_eq!(replay_endpoint(b"1700000000"), Some(1_700_000_000));
        assert_eq!(replay_endpoint(b"0"), Some(0));
        assert_eq!(replay_endpoint(b"12a"), None);
        assert_eq!(replay_endpoint(b" 12"), None);
        assert_eq!(replay_endpoint(b"-1"), None);
        assert_eq!(replay_endpoint(b"99999999999999999999"), None);
    }
}
