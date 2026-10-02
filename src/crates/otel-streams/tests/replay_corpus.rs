//! Replay captured JSONL corpora through the real event→LogRecord path.
//!
//! Corpora captured by `src/bin/capture.rs` hold the raw source JSON that
//! `certstream::connect`/`jetstream::connect` hand through their channels;
//! production bridges turn each `(event, raw_json)` pair into an OTel
//! `LogRecord` via the `Source` trait (`runner::run`). These tests replay
//! corpus lines through that exact path, proving a frozen capture still
//! parses with the current source modules.
//!
//! Two layers:
//!
//! 1. `replay_embedded_fixtures` (always runs): minimal hand-written lines
//!    covering every corpus shape (certstream raw, jetstream raw, mixed
//!    envelope) exercise the same deserialization `connect()` performs.
//! 2. `replay_captured_corpus` (env-gated): replays up to
//!    `CORPUS_REPLAY_LIMIT` (default 1000) lines of the gzipped JSONL at
//!    `CORPUS_REPLAY_FILE` through the same path; used to validate real
//!    frozen captures. Skips when the variable is unset.

use std::io::{BufRead, BufReader};

use flate2::read::GzDecoder;
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use otel_streams::Source;
use otel_streams::certstream::{Certstream, Message};
use otel_streams::jetstream::{Event, Jetstream};

/// Which source a corpus line came from, and its raw event JSON.
enum Replayed {
    Certstream(serde_json::Value),
    Jetstream(serde_json::Value),
}

/// Parse one corpus line and identify its source, unwrapping the mixed
/// envelope when present. Mirrors the capture formats documented on
/// `src/bin/capture.rs`: raw certstream/jetstream JSON per line, or
/// `{"_capture_src":"...","event":{...}}` for the mixed corpus.
fn parse_line(line: &str) -> Replayed {
    let value: serde_json::Value =
        serde_json::from_str(line).expect("corpus line is valid JSON");

    if let Some(tag) = value.get("_capture_src").and_then(|t| t.as_str()) {
        let event = value
            .get("event")
            .cloned()
            .expect("mixed envelope carries `event`");
        return match tag {
            "certstream" => Replayed::Certstream(event),
            "jetstream" => Replayed::Jetstream(event),
            other => panic!("unknown _capture_src tag: {other}"),
        };
    }

    if value.get("message_type").is_some() {
        Replayed::Certstream(value)
    } else if value.get("kind").is_some() {
        Replayed::Jetstream(value)
    } else {
        panic!("line matches no known corpus shape: {value}");
    }
}

/// Replay one corpus line through the production conversion path — the same
/// deserialization `connect()` performs, then the `Source` trait call
/// `runner::run` makes for every event.
fn replay_line(line: &str) -> LogRecord {
    match parse_line(line) {
        Replayed::Certstream(value) => {
            let message: Message = serde_json::from_value(value.clone())
                .expect("certstream line deserializes like connect() does");
            let Message::CertificateUpdate(data) = message else {
                panic!("non-certificate_update lines are never captured");
            };
            <Certstream as Source>::event_to_log_record(&data, &value)
        }
        Replayed::Jetstream(value) => {
            let event: Event = serde_json::from_value(value.clone())
                .expect("jetstream line deserializes like connect() does");
            <Jetstream as Source>::event_to_log_record(&event, &value)
        }
    }
}

#[test]
fn replay_embedded_fixtures() {
    let certstream_line = r#"{"message_type":"certificate_update","data":{"update_type":"X509LogEntryType","cert_index":123456,"cert_link":"http://ct.example-test.example/logs/x/","seen":1762000000.5,"source":{"name":"example-test-ct","url":"http://ct.example-test.example/logs/x/"},"leaf_cert":{"subject":{"aggregated":"CN=example-test.example","CN":"example-test.example","O":"Example Test Org","C":"US"},"issuer":{"aggregated":"CN=Example Test CA","CN":"Example Test CA","O":null,"C":null},"all_domains":["example-test.example","www.example-test.example"],"fingerprint":"00:11","serial_number":"00:AA","not_before":1761000000.0,"not_after":1793000000.0,"signature_algorithm":"sha256WithRSAEncryption","extensions":{"subjectAltName":["DNS:example-test.example"]}}}}"#;
    let jetstream_commit_line = r#"{"did":"did:plc:example00000000000000000","time_us":1762000000000000,"kind":"commit","commit":{"rev":"example-rev","operation":"create","collection":"app.bsky.feed.post","rkey":"3lExampleRkey000","record":{"$type":"app.bsky.feed.post","createdAt":"2026-10-02T12:00:00.000Z","text":"example test post text"},"cid":"bafyexample"}}"#;
    let jetstream_account_line = r#"{"did":"did:plc:example33333333333333333","time_us":1762000001000000,"kind":"account","account":{"active":false,"did":"did:plc:example33333333333333333","seq":7,"time":"2026-10-02T12:00:01.000Z","status":"deactivated"}}"#;
    let mixed_certstream_line = format!(
        r#"{{"_capture_src":"certstream","event":{certstream_line}}}"#
    );
    let mixed_jetstream_line = format!(
        r#"{{"_capture_src":"jetstream","event":{jetstream_commit_line}}}"#
    );

    let certstream = replay_line(certstream_line);
    assert_eq!(certstream.event_name, "certificate_update");
    assert!(certstream.body.is_some(), "certstream body carries raw JSON");

    let commit = replay_line(jetstream_commit_line);
    assert_eq!(commit.event_name, "commit");
    assert!(
        commit.attributes.iter().any(|kv| kv.key == "bluesky.did"),
        "jetstream commit promotes did to attributes"
    );

    let account = replay_line(jetstream_account_line);
    assert_eq!(account.event_name, "account");

    let mixed_a = replay_line(&mixed_certstream_line);
    assert_eq!(mixed_a.event_name, "certificate_update");
    let mixed_b = replay_line(&mixed_jetstream_line);
    assert_eq!(mixed_b.event_name, "commit");
}

#[test]
fn replay_captured_corpus() {
    let Ok(path) = std::env::var("CORPUS_REPLAY_FILE") else {
        eprintln!("skipping: CORPUS_REPLAY_FILE is not set");
        return;
    };
    let limit: usize = std::env::var("CORPUS_REPLAY_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);

    let file = std::fs::File::open(&path).unwrap();
    let reader = BufReader::new(GzDecoder::new(file));
    let mut replayed = 0usize;

    for line in reader.lines() {
        let line = line.unwrap_or_else(|e| {
            panic!("reading {path}: {e} (corpus files must be gzip-compressed JSONL)")
        });
        if line.trim().is_empty() {
            continue;
        }
        let record = replay_line(&line);
        assert!(record.body.is_some());
        replayed += 1;
        if replayed >= limit {
            break;
        }
    }

    assert!(replayed > 0, "no lines replayed from {path}");
    eprintln!("replayed {replayed} lines from {path}");
}
