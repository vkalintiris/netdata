//! The tier-2 report. Live data carries real service names, operations and
//! ids, and the report may be shared, so every stored value and id in it is
//! replaced by a per-run alias (`resource.attributes.service.name#3`,
//! `trace#12`); the alias map is written only to the private run directory.
//! Enum labels, derived fields and field names stay readable. Before a report
//! is kept, [`leaks`] must find nothing in it.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::model::OracleSpan;

/// Per-run aliases for values and ids, numbered in order of first use.
#[derive(Debug, Clone, Default)]
pub struct Aliases {
    values: BTreeMap<String, BTreeMap<String, u32>>,
    traces: BTreeMap<[u8; 16], u32>,
    spans: BTreeMap<[u8; 8], u32>,
}

fn next(map_len: usize) -> u32 {
    u32::try_from(map_len).unwrap_or(u32::MAX).saturating_add(1)
}

impl Aliases {
    pub fn value(&mut self, field: &str, value: &str) -> String {
        let values = self.values.entry(field.to_string()).or_default();
        let len = values.len();
        let number = *values.entry(value.to_string()).or_insert_with(|| next(len));
        format!("{field}#{number}")
    }

    pub fn trace(&mut self, id: [u8; 16]) -> String {
        let len = self.traces.len();
        let number = *self.traces.entry(id).or_insert_with(|| next(len));
        format!("trace#{number}")
    }

    pub fn span(&mut self, id: [u8; 8]) -> String {
        let len = self.spans.len();
        let number = *self.spans.entry(id).or_insert_with(|| next(len));
        format!("span#{number}")
    }

    /// The map from aliases back to raw values: private, never in the report.
    pub fn map(&self) -> serde_json::Value {
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut values = serde_json::Map::new();
        for (field, numbered) in &self.values {
            for (value, number) in numbered {
                values.insert(format!("{field}#{number}"), value.clone().into());
            }
        }
        for (id, number) in &self.traces {
            values.insert(format!("trace#{number}"), hex(id).into());
        }
        for (id, number) in &self.spans {
            values.insert(format!("span#{number}"), hex(id).into());
        }
        serde_json::Value::Object(values)
    }
}

/// What a finding compares; values and ids render through the aliases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    Count(u64),
    Ns(i64),
    Flag(bool),
    /// A readable name: a field name, a tier, a reason, an enum label.
    Name(String),
    Value {
        field: String,
        value: String,
    },
    Trace([u8; 16]),
    Span([u8; 8]),
    Missing,
}

/// Where in an answer a finding is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Locator {
    Name(String),
    Index(usize),
    Value { field: String, value: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The oracle check, e.g. `ORC-HIST`.
    pub check: &'static str,
    /// The scenario, built from readable names only.
    pub scenario: String,
    pub at: Vec<Locator>,
    pub want: Subject,
    pub got: Subject,
}

impl Subject {
    fn render(&self, aliases: &mut Aliases) -> String {
        match self {
            Subject::Count(n) => n.to_string(),
            Subject::Ns(n) => format!("{n} ns"),
            Subject::Flag(b) => b.to_string(),
            Subject::Name(name) => name.clone(),
            Subject::Value { field, value } => aliases.value(field, value),
            Subject::Trace(id) => aliases.trace(*id),
            Subject::Span(id) => aliases.span(*id),
            Subject::Missing => "missing".to_string(),
        }
    }
}

impl Locator {
    fn render(&self, aliases: &mut Aliases) -> String {
        match self {
            Locator::Name(name) => name.clone(),
            Locator::Index(index) => index.to_string(),
            Locator::Value { field, value } => aliases.value(field, value),
        }
    }
}

/// How many values a check compared and how many differed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CheckCount {
    pub compared: u64,
    pub differing: u64,
}

/// The run's shape, for `summary.json` and the head of the report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Named commits (plugin, oracle) as short hashes.
    pub commits: BTreeMap<String, String>,
    /// Capture and membership counts (records, spans, units by kind, ...).
    pub counts: BTreeMap<String, u64>,
    /// Windows judged, as absolute seconds.
    pub windows: Vec<(u32, u32)>,
    pub checks: BTreeMap<String, CheckCount>,
}

/// The markdown report (the first `first_n` findings in full) and the summary
/// as JSON.
pub fn render(
    summary: &Summary,
    findings: &[Finding],
    first_n: usize,
    aliases: &mut Aliases,
) -> (String, serde_json::Value) {
    let mut out = String::from("# Tier-2 comparison\n\n");
    for (name, commit) in &summary.commits {
        out.push_str(&format!("- {name}: `{commit}`\n"));
    }
    for (name, count) in &summary.counts {
        out.push_str(&format!("- {name}: {count}\n"));
    }
    for (after, before) in &summary.windows {
        out.push_str(&format!("- window: {after} .. {before}\n"));
    }
    out.push_str("\n| check | compared | differing |\n|---|---|---|\n");
    for (check, count) in &summary.checks {
        out.push_str(&format!(
            "| {check} | {} | {} |\n",
            count.compared, count.differing
        ));
    }
    out.push_str(&format!("\n## Findings ({})\n\n", findings.len()));
    for finding in findings.iter().take(first_n) {
        let at: Vec<String> = finding.at.iter().map(|l| l.render(aliases)).collect();
        out.push_str(&format!(
            "- {} {} at {}: want {}, got {}\n",
            finding.check,
            finding.scenario,
            at.join(" / "),
            finding.want.render(aliases),
            finding.got.render(aliases),
        ));
    }
    let json = serde_json::to_value(summary).unwrap_or_default();
    (out, json)
}

/// Fields whose values are labels the plugin or the protocol defines.
const READABLE_FIELDS: [&str; 2] = ["kind", "status_code"];

/// The `top_n` most frequent values of every field that can hold data from
/// the traced services (not `_`-derived fields or enum labels), leaving out
/// numbers, booleans and values shorter than 4 characters.
pub fn sensitive_values(spans: &[OracleSpan], top_n: usize) -> BTreeSet<String> {
    let mut counts: BTreeMap<&str, BTreeMap<&str, u64>> = BTreeMap::new();
    for span in spans {
        for (field, values) in &span.fields {
            if field.starts_with('_') || READABLE_FIELDS.contains(&field) {
                continue;
            }
            let per_field = counts.entry(field).or_default();
            for value in values {
                *per_field.entry(value).or_default() += 1;
            }
        }
    }
    let mut out = BTreeSet::new();
    for per_field in counts.into_values() {
        let mut ranked: Vec<(&str, u64)> = per_field.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        for (value, _) in ranked.into_iter().take(top_n) {
            let plain = value.parse::<f64>().is_ok() || value == "true" || value == "false";
            if !plain && value.chars().count() >= 4 {
                out.insert(value.to_string());
            }
        }
    }
    out
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Raw values of `sensitive` found in `text` as whole words, and any 16- or
/// 32-character hex word (an id).
pub fn leaks(text: &str, sensitive: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in sensitive {
        for (at, _) in text.match_indices(value.as_str()) {
            let before = text[..at].chars().next_back();
            let after = text[at + value.len()..].chars().next();
            if !before.is_some_and(is_word) && !after.is_some_and(is_word) {
                out.push(value.clone());
                break;
            }
        }
    }
    for word in text.split(|c: char| !c.is_ascii_hexdigit()) {
        if (word.len() == 16 || word.len() == 32) && word.bytes().all(|b| b.is_ascii_hexdigit()) {
            out.push(word.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVICE: &str = "resource.attributes.service.name";

    fn span(service: &str, name: &str) -> OracleSpan {
        let fields = [
            (SERVICE, service),
            ("name", name),
            ("status_code", "ERROR"),
            ("_role", "root"),
            ("attributes.retries", "1234"),
        ]
        .into_iter()
        .collect();
        OracleSpan {
            trace_id: None,
            span_id: None,
            parent_span_id: None,
            start_ns: 0,
            duration_ns: 0,
            fields,
            unit: 0,
        }
    }

    #[test]
    fn aliases_number_values_per_field_and_ids_in_order_of_use() {
        let mut aliases = Aliases::default();

        assert_eq!(aliases.value(SERVICE, "checkout"), format!("{SERVICE}#1"));
        assert_eq!(aliases.value(SERVICE, "payment"), format!("{SERVICE}#2"));
        assert_eq!(aliases.value(SERVICE, "checkout"), format!("{SERVICE}#1"));
        assert_eq!(aliases.value("name", "checkout"), "name#1");
        assert_eq!(aliases.trace([7; 16]), "trace#1");
        assert_eq!(aliases.span([9; 8]), "span#1");
        assert_eq!(aliases.map()[format!("{SERVICE}#2")], "payment");
        assert_eq!(aliases.map()["trace#1"], "07".repeat(16));
    }

    #[test]
    fn the_sensitive_set_holds_top_service_values_only() {
        let spans = [
            span("checkout", "PlaceOrder"),
            span("checkout", "GetCart"),
            span("ad", "GetAds"),
        ];

        let sensitive = sensitive_values(&spans, 1);

        assert_eq!(
            sensitive,
            BTreeSet::from(["checkout".to_string(), "GetAds".to_string()])
        );
    }

    #[test]
    fn a_rendered_report_shows_aliases_and_never_the_raw_values() {
        let spans = [
            span("checkout", "PlaceOrder"),
            span("checkout", "PlaceOrder"),
        ];
        let sensitive = sensitive_values(&spans, 5);
        let trace = [0xab; 16];
        let findings = [Finding {
            check: "ORC-HIST",
            scenario: "F2 entry spans of the top service".to_string(),
            at: vec![
                Locator::Name("histogram".to_string()),
                Locator::Index(12),
                Locator::Value {
                    field: SERVICE.to_string(),
                    value: "checkout".to_string(),
                },
            ],
            want: Subject::Count(4),
            got: Subject::Trace(trace),
        }];
        let summary = Summary {
            checks: BTreeMap::from([(
                "ORC-HIST".to_string(),
                CheckCount {
                    compared: 60,
                    differing: 1,
                },
            )]),
            ..Summary::default()
        };
        let mut aliases = Aliases::default();

        let (report, json) = render(&summary, &findings, 10, &mut aliases);

        assert!(report.contains(&format!("{SERVICE}#1")), "{report}");
        assert!(report.contains("trace#1"), "{report}");
        assert_eq!(leaks(&report, &sensitive), Vec::<String>::new(), "{report}");
        assert_eq!(json["checks"]["ORC-HIST"]["differing"], 1);

        let leaked = format!("{report}\nPlaceOrder and {}", "ab".repeat(16));
        assert_eq!(
            leaks(&leaked, &sensitive),
            vec!["PlaceOrder".to_string(), "ab".repeat(16)]
        );
        assert_eq!(leaks("PlaceOrders", &sensitive), Vec::<String>::new());
    }
}
