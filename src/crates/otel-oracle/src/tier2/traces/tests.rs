use super::*;
use crate::corpus::{self, MeshParams};
use crate::model::spans_of_request;
use crate::tier2::judge;

const T0_S: u32 = 1_700_000_000;
const LIVE: usize = 2;

/// A mesh in two sealed units and a live one, a third each, with the last
/// request of the second sent again into the live unit, and the explorer's
/// tokens.
fn rows() -> (Vec<OracleSpan>, Plan) {
    let generated = corpus::generate(&MeshParams {
        traces: 40,
        start_ns: u64::from(T0_S) * 1_000_000_000,
        trace_spacing_ns: 700_000_000,
        seed: 17,
    });
    let requests = corpus::build_requests(&generated, 30);
    let third = requests.len() / 3;
    let mut rows = Vec::new();
    for (index, request) in requests.iter().enumerate() {
        rows.extend(spans_of_request(request, (index / third).min(LIVE)));
    }
    rows.extend(spans_of_request(&requests[2 * third - 1], LIVE));
    calc::add_derived(&mut rows, &|unit| Some(unit));
    let last_s = rows.iter().map(|row| row.start_ns).max().unwrap() / 1_000_000_000;
    let plan = Plan {
        after_s: T0_S,
        before_s: u32::try_from(last_s).unwrap() + 1,
        candidates: 3,
        scenarios: Vec::new(),
        requests: Vec::new(),
    };
    (rows, plan)
}

fn planned() -> (Vec<OracleSpan>, Plan) {
    let (rows, mut plan) = rows();
    add_traces(&mut plan, &rows, &|unit| unit == LIVE);
    (rows, plan)
}

/// What the agent would answer if it agreed with the calculator.
fn agreeing(rows: &[OracleSpan], request: &Request) -> Value {
    let Ask::Trace {
        trace_id,
        after_s,
        before_s,
        span_cap,
    } = &request.ask
    else {
        panic!("not a trace ask");
    };
    let want = assembly::assemble_trace(rows, *trace_id, *span_cap, &|_| true);
    let view = want.view(&want.first_choice());
    let mut spans = Vec::new();
    for (at, span) in view.spans.iter().enumerate() {
        let detail = &span.detail;
        let mut events = Vec::new();
        for event in &detail.events {
            events.push(json!({
                "time_unix_nano": event.time_unix_nano,
                "name": event.name,
                "dropped_attributes_count": event.dropped_attributes_count,
                "attributes": event.attributes,
            }));
        }
        let mut links = Vec::new();
        for link in &detail.links {
            links.push(json!({
                "trace_id": hex(&link.trace_id),
                "span_id": hex(&link.span_id),
                "trace_state": link.trace_state,
                "flags": link.flags,
                "dropped_attributes_count": link.dropped_attributes_count,
                "attributes": link.attributes,
            }));
        }
        let mut out = json!({
            "span_id": hex(&span.span_id.unwrap_or_default()),
            "start_ns": span.start_ns,
            "duration_ns": span.duration_ns,
            "self_duration_ns": view.self_ns[at],
            "error_origin": view.error_origin[at],
            "kind": detail.kind,
            "flags": detail.flags,
            "dropped_attributes_count": detail.dropped_attributes_count,
            "dropped_events_count": detail.dropped_events_count,
            "dropped_links_count": detail.dropped_links_count,
            "fields": span.fields.iter().collect::<Vec<_>>(),
            "events": events,
            "links": links,
        });
        if let Some(parent) = span.parent_span_id {
            out["parent_span_id"] = json!(hex(&parent));
        }
        spans.push(out);
    }
    let status = if want.truncated() {
        json!({"partial": [{"reason": "size_cap", "count": 1}]})
    } else {
        json!({"complete": true})
    };
    json!({
        "mode": "trace",
        "version": 1,
        "trace_id": hex(trace_id),
        "coverage": {"after": after_s, "before": before_s},
        "status": status,
        "items": {"returned": spans.len()},
        "summary_root": view.summary_root,
        "roots": view.roots,
        "children": view.children,
        "spans": spans,
        "field_kinds": {"fields": [], "event_attributes": [], "link_attributes": []},
        "log_streams": [],
    })
}

fn answers(plan: &Plan, rows: &[OracleSpan]) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for request in &plan.requests {
        out.insert(request.id.clone(), agreeing(rows, request));
    }
    out
}

fn request<'a>(plan: &'a Plan, id: &str) -> &'a Request {
    plan.requests.iter().find(|r| r.id == id).unwrap()
}

fn locator_names(finding: &Finding) -> Vec<String> {
    finding
        .at
        .iter()
        .map(|at| match at {
            Locator::Name(name) => name.clone(),
            Locator::Index(index) => index.to_string(),
            Locator::Value { field, .. } => field.clone(),
        })
        .collect()
}

#[test]
fn picks_each_kind_of_trace_once_over_the_grid() {
    let (rows, plan) = planned();
    let grid = Grid::for_window(plan.after_s, plan.before_s);

    let labels: Vec<&str> = plan.requests.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        labels,
        vec![
            "trace slowest entry",
            "trace error origin",
            "trace two units",
            "trace sealed and live",
            "trace resent",
            "trace largest",
            "trace largest capped",
        ]
    );
    let mut asked = BTreeSet::new();
    for request in &plan.requests {
        let Ask::Trace {
            trace_id,
            after_s,
            before_s,
            span_cap,
        } = &request.ask
        else {
            panic!("{} is not a trace ask", request.id);
        };
        assert!(
            asked.insert((*trace_id, *span_cap)),
            "{} asked twice",
            request.id
        );
        assert_eq!((*after_s, *before_s), (grid.after_s, grid.before_s));
        assert_eq!(
            request.body,
            json!({"trace": {"id": hex(trace_id), "after": grid.after_s,
                "before": grid.before_s, "span_cap": span_cap}})
        );
    }

    let Ask::Trace { trace_id, .. } = request(&plan, "trace largest").ask else {
        unreachable!()
    };
    let spans = assembly::assemble_trace(&rows, trace_id, usize::MAX, &|_| true)
        .items
        .len();
    assert!(matches!(
        request(&plan, "trace largest capped").ask,
        Ask::Trace { span_cap, .. } if span_cap == spans - 1
    ));
    let units_of = |id: &str| {
        let Ask::Trace { trace_id, .. } = request(&plan, id).ask else {
            unreachable!()
        };
        let mut units = BTreeSet::new();
        for row in rows.iter().filter(|row| row.trace_id == Some(trace_id)) {
            units.insert(row.unit);
        }
        units
    };
    assert_eq!(units_of("trace two units"), BTreeSet::from([0, 1]));
    assert!(units_of("trace sealed and live").contains(&LIVE));
    assert!(units_of("trace sealed and live").len() > 1);
}

#[test]
fn an_agreeing_answer_gives_no_finding() {
    let (rows, plan) = planned();
    let answers = answers(&plan, &rows);
    assert_eq!(
        answers["trace largest capped"]["status"],
        json!({"partial": [{"reason": "size_cap", "count": 1}]})
    );

    let (findings, checks) = judge_traces(&plan, &rows, &answers);

    assert_eq!(findings, Vec::new());
    let asked = plan.requests.len() as u64;
    assert_eq!(
        (checks["ORC-TRACE"], checks["ORC-STATUS"]),
        (
            CheckCount {
                compared: 7 * asked,
                differing: 0
            },
            CheckCount {
                compared: asked + 2,
                differing: 0
            }
        ),
        "echo, count and five views per ask; one status, three for the capped size_cap"
    );
}

#[test]
fn one_wrong_leaf_is_exactly_one_finding() {
    let (rows, plan) = planned();
    let good = answers(&plan, &rows);
    let id = "trace largest";
    let other_trace = "0102030405060708090a0b0c0d0e0f10";
    type Edit = fn(&mut Value);
    let cases: Vec<(&str, Edit, &str, Vec<&str>)> = vec![
        (
            "a duration",
            |a| {
                a["spans"][1]["duration_ns"] =
                    json!(a["spans"][1]["duration_ns"].as_i64().unwrap() + 1)
            },
            "ORC-TRACE",
            vec!["spans", "1", "duration"],
        ),
        (
            "a start",
            |a| a["spans"][1]["start_ns"] = json!(a["spans"][1]["start_ns"].as_i64().unwrap() + 1),
            "ORC-TRACE",
            vec!["spans", "1", "start"],
        ),
        (
            "a parent",
            |a| {
                a["spans"][1]
                    .as_object_mut()
                    .unwrap()
                    .remove("parent_span_id");
            },
            "ORC-TRACE",
            vec!["spans", "1", "parent span id"],
        ),
        (
            "flags",
            |a| a["spans"][1]["flags"] = json!(7),
            "ORC-TRACE",
            vec!["spans", "1", "flags"],
        ),
        (
            "an event",
            |a| {
                a["spans"][1]["events"].as_array_mut().unwrap().push(json!({
                    "time_unix_nano": 1, "name": "retry", "dropped_attributes_count": 0,
                    "attributes": []}));
            },
            "ORC-TRACE",
            vec!["spans", "1", "events"],
        ),
        (
            "a link",
            |a| {
                a["spans"][1]["links"].as_array_mut().unwrap().push(json!({
                    "trace_id": "0102030405060708090a0b0c0d0e0f10",
                    "span_id": "0102030405060708", "trace_state": "", "flags": 0,
                    "dropped_attributes_count": 0, "attributes": []}));
            },
            "ORC-TRACE",
            vec!["spans", "1", "links"],
        ),
        (
            "a field value",
            |a| a["spans"][1]["fields"][0][1] = json!("another value"),
            "ORC-TRACE",
            vec!["spans", "1", "*"],
        ),
        (
            "a self time",
            |a| {
                let n = a["spans"][1]["self_duration_ns"].as_i64().unwrap();
                a["spans"][1]["self_duration_ns"] = json!(n + 1);
            },
            "ORC-TRACE",
            vec!["self time", "1"],
        ),
        (
            "an absent self time",
            |a| {
                a["spans"][1]
                    .as_object_mut()
                    .unwrap()
                    .remove("self_duration_ns");
            },
            "ORC-TRACE",
            vec!["self time", "1"],
        ),
        (
            "an error origin",
            |a| {
                let origin = a["spans"][1]["error_origin"].as_bool().unwrap();
                a["spans"][1]["error_origin"] = json!(!origin);
            },
            "ORC-TRACE",
            vec!["error origin", "1"],
        ),
        (
            "a root",
            |a| a["roots"].as_array_mut().unwrap().push(json!(1)),
            "ORC-TRACE",
            vec!["roots", "1"],
        ),
        (
            "children",
            |a| a["children"][0] = json!([]),
            "ORC-TRACE",
            vec!["children", "0"],
        ),
        (
            "the summary root",
            |a| a["summary_root"] = json!(1),
            "ORC-TRACE",
            vec!["summary root", "0"],
        ),
        (
            "a size cap",
            |a| a["status"] = json!({"partial": [{"reason": "size_cap", "count": 1}]}),
            "ORC-STATUS",
            vec!["status", "size_cap"],
        ),
        (
            "the coverage",
            |a| a["coverage"]["after"] = json!(0),
            "ORC-TRACE",
            vec!["echo", "2"],
        ),
        (
            "the trace id",
            |a| a["trace_id"] = json!("0102030405060708090a0b0c0d0e0f10"),
            "ORC-TRACE",
            vec!["echo", "1"],
        ),
        (
            "the returned count",
            |a| a["items"]["returned"] = json!(1),
            "ORC-TRACE",
            vec!["echo", "4"],
        ),
        (
            "a span too few",
            |a| {
                let spans = a["spans"].as_array_mut().unwrap();
                spans.pop();
                a["items"]["returned"] = json!(a["spans"].as_array().unwrap().len());
            },
            "ORC-TRACE",
            vec!["spans"],
        ),
    ];
    assert_ne!(good[id]["trace_id"], json!(other_trace));
    assert!(good[id]["spans"][1]["parent_span_id"].is_string());
    for (name, edit, check, at) in cases {
        let mut answers = good.clone();
        edit(answers.get_mut(id).unwrap());
        let (findings, _) = judge_traces(&plan, &rows, &answers);
        assert_eq!(findings.len(), 1, "{name}: {findings:?}");
        assert_eq!(findings[0].check, check, "{name}");
        assert_eq!(findings[0].scenario, id, "{name}");
        let got = locator_names(&findings[0]);
        if at.last() == Some(&"*") {
            assert_eq!(got[..at.len() - 1], at[..at.len() - 1], "{name}");
        } else {
            assert_eq!(got, at, "{name}");
        }
    }
}

#[test]
fn a_missing_or_unreadable_answer_is_one_finding() {
    let (rows, plan) = planned();
    let good = answers(&plan, &rows);
    let id = "trace largest";
    type Edit = fn(&mut BTreeMap<String, Value>);
    let cases: [(&str, Edit, Vec<&str>); 3] = [
        (
            "missing",
            |answers| {
                answers.remove("trace largest");
            },
            vec!["trace largest"],
        ),
        (
            "not a trace answer",
            |answers| {
                answers.insert("trace largest".into(), json!({"mode": "explore"}));
            },
            vec!["trace largest"],
        ),
        (
            "a span id that is not hex",
            |answers| {
                answers.get_mut("trace largest").unwrap()["spans"][0]["span_id"] =
                    json!("zzzzzzzzzzzzzzzz");
            },
            vec!["spans"],
        ),
    ];
    for (name, edit, at) in cases {
        let mut answers = good.clone();
        edit(&mut answers);
        let (findings, _) = judge_traces(&plan, &rows, &answers);
        assert_eq!(findings.len(), 1, "{name}: {findings:?}");
        assert_eq!(findings[0].check, "ORC-ANSWER", "{name}");
        assert_eq!(findings[0].scenario, id, "{name}");
        assert_eq!(locator_names(&findings[0]), at, "{name}");
    }
}

#[test]
fn the_explore_judge_leaves_trace_asks_alone() {
    let (rows, plan) = planned();
    let (findings, checks) = judge(&plan, &rows, &BTreeMap::new());
    assert_eq!(findings, Vec::new());
    assert!(checks.is_empty());
}
