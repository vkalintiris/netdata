//! Health's DynCfg nodes against C's own `health_dyncfg.c` (`tests/oracle/gen-loop-vectors.c`, the mode `dyncfg`):
//! the payload's parser, the `userconfig` text and every command on the template and on a job. The scenarios that
//! change a host's alerts are replayed in `loop.rs`.

mod common;

use std::cell::RefCell;
use std::sync::Arc;

use common::{Core, OneHost, answer_fields, dyncfg_cases, oracle_config, rows, rule_fields, show, unescape};
use netdata_agent_dyncfg::model::Cmds;
use netdata_agent_health::Health;
use netdata_agent_health::dyncfg::{payload_parse, TEMPLATE_ID};
use netdata_agent_health::json::prototype_to_json;
use netdata_agent_health::pass::Idle;
use netdata_agent_health::prototype::Rule;
use netdata_agent_health::readfile::health_readfile;
use netdata_agent_ingest::jsonc::Presence;
use netdata_agent_nrpc::reply::{ContentType, Reply};
use netdata_agent_rrd::host::{Host, HostInfo};
use netdata_agent_rrd::mode::DbMode;

const NOW: i64 = 2_000_000_000;

fn host() -> Arc<Host> {
    let info = HostInfo {
        hostname: "oracle-host".into(),
        registry_hostname: "oracle-registry".into(),
        os: "linux".into(),
        timezone: "UTC".into(),
        abbrev_timezone: "UTC".into(),
        utc_offset: 0,
        program_name: "netdata".into(),
        program_version: "v0".into(),
        update_every: 1,
        db_mode: DbMode::Ram,
        history_entries: 60,
        health_enabled: true,
        system_info: Default::default(),
        replication_enabled: false,
        replication_period: 0,
        replication_step: 0,
        stream_send: None,
        cache_dir: None,
    };
    Arc::new(Host::new("11111111-2222-4333-8444-555555555555", true, info))
}

/// The messages of what `f` records, joined as the generator joins them (`-` for none).
fn messages_of<T>(f: impl FnOnce() -> T) -> (T, Vec<u8>) {
    let (result, records) = netdata_agent_log::capture(f);
    let messages: Vec<String> = records.into_iter().filter_map(|record| record.message).collect();
    (result, if messages.is_empty() { b"-".to_vec() } else { messages.join(" | ").into_bytes() })
}

/// Why Rust reads a payload otherwise than C, by decision: what marks the payload, and the decision.
const DECIDED: [(&[u8], &str); 18] = [
    // D46.1: what json-c reads and serde_json refuses is refused
    (b"/* a comment */", "a comment"),
    (b"// a comment", "a comment"),
    (b"'format_version'", "single quotes"),
    (b"}}}}],}", "a trailing comma"),
    (b":NaN", "NaN"),
    (b":Infinity", "Infinity"),
    (b":-Infinity", "Infinity"),
    (b":TRUE", "a literal in capitals"),
    (b":True", "a literal in capitals"),
    (b"\"after\":060", "a leading zero"),
    (b"\"after\":-60.", "a number that ends in its dot"),
    (b"\\ud800", "half a surrogate pair, which C reads as U+FFFD"),
    (b"sum\\u0000mary", "a NUL in a name"),
    // D89: a byte that is not UTF-8 is read as U+FFFD
    (b"\xff", "a byte that is not UTF-8"),
    // an integer past 64 bits is a double for serde_json; json-c saturates it
    (b"123456789012345678901234567890", "an integer past 64 bits"),
    (b"18446744073709551616", "an integer past 64 bits"),
    (b"\"time_group_value\":1e400", "a number past a double's range"),
    (b"\"multiplier\":1e400", "a number past a double's range"),
];

/// The place in `DECIDED` of the first mark the payload holds.
fn decided(payload: &[u8]) -> Option<usize> {
    DECIDED.iter().position(|(mark, _)| payload.windows(mark.len()).any(|window| window == *mark))
}

type Fields = Vec<Vec<u8>>;

/// What a parse gave, as the generator's rows hold it: the `parse` row's fields after the kind, and a `rule` row's
/// fields per rule.
type Parsed = (Fields, Vec<Fields>);

fn parsed_fields(result: &Result<Vec<Rule>, String>, messages: Vec<u8>) -> Parsed {
    match result {
        Err(error) => (vec![b"0".to_vec(), error.clone().into_bytes(), b"-".to_vec(), messages], Vec::new()),
        Ok(rules) => {
            let chain: Vec<&Rule> = rules.iter().collect();
            let name = rules[0].config.name.as_deref().unwrap_or(b"");
            let json = prototype_to_json(name, &chain, true);
            let rule_rows = rules
                .iter()
                .enumerate()
                .map(|(place, rule)| {
                    let mut fields = vec![place.to_string().into_bytes()];
                    let all = rule_fields(rule);
                    // without the hash and the rule's own JSON: a parsed rule has no hash yet
                    fields.extend_from_slice(&all[..all.len() - 2]);
                    fields
                })
                .collect();
            (vec![rules.len().to_string().into_bytes(), Vec::new(), json, messages], rule_rows)
        }
    }
}

/// C's `health_prototype_payload_parse()` over every payload of the list, in both modes: the rules it makes, with
/// every field of each, or its error text, and the records. The payloads Rust reads otherwise by decision are
/// named in `DECIDED`; each of them must differ from C, and none other may.
#[test]
fn payloads_parse_as_c() {
    let cases = dyncfg_cases("payloads.txt");
    let payloads: Vec<Vec<u8>> = cases.iter().map(|case| unescape(case)).collect();

    // C's rows by case and mode
    let mut expected: Vec<[Parsed; 2]> = vec![Default::default(); payloads.len()];
    for row in rows("payload.tsv") {
        assert!(row.fields.len() > 2, "payload.tsv:{}: C died on case {}", row.line, row.str(0));
        let case: usize = row.num(0);
        let mode = match row.str(1) {
            "required" => 0,
            "optional" => 1,
            other => panic!("payload.tsv:{}: mode {other}", row.line),
        };
        match row.str(2) {
            "parse" => expected[case][mode].0 = row.fields[3..].to_vec(),
            "rule" => expected[case][mode].1.push(row.fields[3..].to_vec()),
            other => panic!("payload.tsv:{}: kind {other}", row.line),
        }
    }

    let (mut checked, mut failures) = (0, Vec::new());
    let mut differing = [0usize; DECIDED.len()];
    for (case, payload) in payloads.iter().enumerate() {
        for (mode, presence) in [Presence::Required, Presence::Optional].into_iter().enumerate() {
            let (c_parse, c_rules) = &expected[case][mode];
            assert!(!c_parse.is_empty(), "case {case} has no row of mode {mode}");
            let (result, messages) = messages_of(|| payload_parse(payload, Some(b"p_name"), presence));
            let (mut parse, rules) = parsed_fields(&result, messages);
            // on success C's buffer may hold the texts of unknown option names, which nothing reads
            if result.is_ok() {
                parse[1].clone_from(&c_parse[1]);
            }
            let same = parse == *c_parse && rules == *c_rules;
            checked += 1;
            match (decided(payload), same) {
                // a decided difference need not show in both modes
                (None | Some(_), true) => {}
                (Some(mark), false) => differing[mark] += 1,
                (None, false) => {
                    let first_rule = c_rules.iter().zip(&rules).position(|(c, rust)| c != rust);
                    failures.push(format!(
                        "case {case} ({presence:?}) {}\n  C    {:?}\n  Rust {:?}\n  rules {} against {}, first {first_rule:?}: \
                         {:?}\n  {:?}",
                        String::from_utf8_lossy(&cases[case]),
                        show(c_parse),
                        show(&parse),
                        c_rules.len(),
                        rules.len(),
                        first_rule.map(|i| show(&c_rules[i])),
                        first_rule.map(|i| show(&rules[i])),
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} of {checked}:\n{}", failures.len(), failures.join("\n"));
    assert_eq!(checked, 2 * payloads.len());
    assert!(checked >= 1200, "{checked}");
    // every mark of the list is one C reads otherwise: a mark no payload differs by is no decision
    let same: Vec<String> = DECIDED
        .iter()
        .zip(differing)
        .filter(|(_, differing)| *differing == 0)
        .map(|((mark, why), _)| format!("{:?} ({why})", String::from_utf8_lossy(mark)))
        .collect();
    assert!(same.is_empty(), "marks no payload differs from C by: {same:?}");
    // how many parses differ by decision: a growth of this number is a new difference from C
    assert_eq!(differing.iter().sum::<usize>(), 58);
}

/// A store as the generator's tables have it: `base.conf` read into it.
fn health_with_base() -> Arc<Health> {
    let health = Health::init(oracle_config(), Box::new(|_| {}));
    assert!(health_readfile(&health, b"tests/corpus/dyncfg/base.conf", false));
    health
}

fn cmd_of(word: &str) -> Cmds {
    if word == "none" {
        return Cmds::NONE;
    }
    let cmd = Cmds::parse(word.as_bytes());
    assert_ne!(cmd, Cmds::NONE, "{word}");
    cmd
}

/// C's `dyncfg_health_cb()` for a `userconfig` of every payload of the list, sent to the template and to a job,
/// with localhost's default command unset (its health never ran) and set: the code, the content type, the cache
/// word, the expiry and the text.
#[test]
fn userconfig_prints_as_c() {
    let cases = dyncfg_cases("userconfig.txt");
    let payloads: Vec<Vec<u8>> = cases.iter().map(|case| unescape(case)).collect();
    let clock = || NOW;
    let mut checked = 0;
    let mut failures = Vec::new();
    for row in rows("userconfig.tsv") {
        assert!(row.fields.len() > 2, "userconfig.tsv:{}: C died on case {}", row.line, row.str(0));
        let case: usize = row.num(0);
        let job = match row.str(1) {
            "template" => false,
            "job" => true,
            other => panic!("userconfig.tsv:{}: node {other}", row.line),
        };
        let exec_set = row.flag(2);

        let health = health_with_base();
        let hosts = OneHost(host());
        if exec_set {
            // a host's first pass gives it the configuration's defaults
            health.host_link(&hosts.0, &clock, &|| true);
        }
        let (saved, user_disabled) = (Vec::new(), Vec::new());
        let calls: RefCell<Vec<Vec<Vec<u8>>>> = RefCell::default();
        let core = Core {
            health: &health,
            hosts: &hosts,
            env: &Idle,
            clock: &clock,
            saved: &saved,
            user_disabled: &user_disabled,
            cloud_has: true,
            record: &|call| calls.borrow_mut().push(call),
        };
        let id = if job { [TEMPLATE_ID, b":d_tpl"].concat() } else { TEMPLATE_ID.to_vec() };
        let mut reply = Reply::new(ContentType::TextPlain);
        let (code, messages) = messages_of(|| {
            health.dyncfg_callback(&core.ctx(), &mut reply, &id, Cmds::USERCONFIG, Some(b"u_name"), Some(&payloads[case]))
        });
        let mut fields = answer_fields(code, &reply, NOW);
        fields.push(messages);
        checked += 1;
        if decided(&payloads[case]).is_none() && fields != row.fields[3..] {
            failures.push(format!(
                "userconfig.tsv:{} case {case}\n  C    {:?}\n  Rust {:?}",
                row.line,
                show(&row.fields[3..]),
                show(&fields)
            ));
        }
        assert!(calls.borrow().is_empty(), "a userconfig calls nothing");
    }
    assert!(failures.is_empty(), "{} of {checked}:\n{}", failures.len(), failures.join("\n"));
    assert_eq!(checked, 4 * payloads.len());
}

/// C's `dyncfg_health_cb()` for every command on the template, on jobs and on ids that are none of health's, each
/// on the same store (`base.conf` and two added jobs) with a host whose health never ran: the answer, the records,
/// the calls into the core and to the Cloud's table, and the store's names afterwards.
#[test]
fn actions_answer_as_c() {
    let lines = dyncfg_cases("actions.txt");
    let clock = || NOW;
    let mut checked = 0;
    let mut failures = Vec::new();
    for row in rows("actions.tsv") {
        assert!(row.fields.len() > 2, "actions.tsv:{}: C died on case {}", row.line, row.str(0));
        let case: usize = row.num(0);
        let fields: Vec<&[u8]> = lines[case].split(|&b| b == b'\t').collect();
        assert_eq!(fields.len(), 4, "actions.txt case {case}");
        let id = unescape(fields[0]);
        let cmd = cmd_of(std::str::from_utf8(fields[1]).expect("an action"));
        let name = (fields[2] != b"-").then(|| unescape(fields[2]));
        let payload = (fields[3] != b"-").then(|| unescape(fields[3]));

        let health = health_with_base();
        let hosts = OneHost(host());
        let (saved, user_disabled) = (Vec::new(), Vec::new());
        let calls: RefCell<Vec<Vec<Vec<u8>>>> = RefCell::default();
        let core = Core {
            health: &health,
            hosts: &hosts,
            env: &Idle,
            clock: &clock,
            saved: &saved,
            user_disabled: &user_disabled,
            cloud_has: true,
            record: &|call| calls.borrow_mut().push(call),
        };
        for (job, file) in [("d_dyn", "one.json"), ("d_off", "off.json")] {
            let added = std::fs::read(format!("tests/corpus/dyncfg/{file}")).expect("a payload");
            let mut reply = Reply::new(ContentType::TextPlain);
            let code = health.dyncfg_callback(
                &core.ctx(),
                &mut reply,
                TEMPLATE_ID,
                Cmds::ADD,
                Some(job.as_bytes()),
                Some(&added),
            );
            assert!(code == 202 || code == 298, "{job}: {code}");
        }
        calls.borrow_mut().clear();

        let mut reply = Reply::new(ContentType::TextPlain);
        let (code, messages) = messages_of(|| {
            health.dyncfg_callback(&core.ctx(), &mut reply, &id, cmd, name.as_deref(), payload.as_deref())
        });
        let mut got = answer_fields(code, &reply, NOW);
        got.push(messages);
        // the calls: a call's fields separated by tabs, the calls by ` | `; `-` for none
        let made: Vec<Vec<u8>> = calls.borrow().iter().map(|call| call.join(&b'\t')).collect();
        let made = if made.is_empty() { b"-".to_vec() } else { made.join(&b" | "[..]) };
        got.extend(made.split(|&b| b == b'\t').map(<[u8]>::to_vec));
        let store: Vec<String> = health
            .prototypes()
            .iter()
            .map(|(name, prototype)| {
                format!("{}:{}:{}", String::from_utf8_lossy(name), u8::from(prototype.enabled()), prototype.rules().len())
            })
            .collect();
        got.push(store.join(" ").into_bytes());

        checked += 1;
        if got != row.fields[1..] {
            failures.push(format!(
                "actions.tsv:{} case {case} {}\n  C    {:?}\n  Rust {:?}",
                row.line,
                String::from_utf8_lossy(&lines[case]).chars().take(120).collect::<String>(),
                show(&row.fields[1..]),
                show(&got)
            ));
        }
    }
    assert!(failures.is_empty(), "{} of {checked}:\n{}", failures.len(), failures.join("\n"));
    assert_eq!(checked, lines.len());
    assert!(checked >= 100, "{checked}");
}
