//! Which rule goes to which chart (`health_prototypes.c`): a rule's label texts compiled into pattern arrays, the
//! tests a rule passes against a host and a chart, and the order C applies the stored rules in.

use netdata_agent_rrd::labels::{Labels, PatternArray};
use netdata_agent_text::simple_pattern::{Separators, SimplePattern, SimplePatternMode};

use crate::prototype::{Prototype, Prototypes, Rule};

/// `simple_pattern_trim_around_equal()`: at each `=` one space before it and one after it go. The byte after that
/// is copied without a second look, so `a==b` and `a= =b` keep their second `=`.
fn trim_around_equal(src: &[u8]) -> Vec<u8> {
    let mut dst = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i] == b'=' {
            if dst.last() == Some(&b' ') {
                dst.pop();
            }
            dst.push(b'=');
            i += 1;
            if src.get(i) == Some(&b' ') {
                i += 1;
            }
            if i == src.len() {
                break;
            }
        }
        dst.push(src[i]);
        i += 1;
    }
    dst
}

/// One value token under `key`: the exact pattern `KEY=VALUE`, negative when the token starts with `!`.
fn add_value(array: &mut PatternArray, key: &[u8], data: &[u8]) {
    let (negative, value) = match data.strip_prefix(b"!") {
        Some(value) => (true, value),
        None => (false, data),
    };
    let mut pair = Vec::with_capacity(key.len() + value.len() + 3);
    if negative {
        pair.push(b'!');
    }
    pair.extend_from_slice(key);
    pair.push(b'=');
    pair.extend_from_slice(value);
    pair.push(b' ');
    array.add(key, SimplePattern::new(&pair, Separators::Whitespace, SimplePatternMode::Exact, true));
}

/// `trim_and_add_key_to_values(NULL, NULL, text)`: a rule's `host labels` or `chart labels` text as a pattern array,
/// `None` when the text holds no value (C's NULL array, which matches everything).
///
/// Byte by byte: `=` makes the token so far the current key; a space ends a value token, also an empty one; the
/// text's end ends a token that is not empty. The `!` belongs to the value: on a key it is part of the key's name.
pub fn label_patterns(text: &[u8]) -> Option<PatternArray> {
    let mut array = PatternArray::default();
    let mut key: Vec<u8> = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    for c in trim_around_equal(text) {
        match c {
            b'=' => key = std::mem::take(&mut data),
            b' ' => {
                add_value(&mut array, &key, &data);
                data.clear();
            }
            _ => data.push(c),
        }
    }
    if !data.is_empty() {
        add_value(&mut array, &key, &data);
    }
    (!array.is_empty()).then_some(array)
}

/// `prototype_matches_host()`: `[health] enabled alarms` on the rule's name, then the rule's host labels. A list
/// without a word (empty, blanks, a lone `!`) is C's NULL pattern, which lets every rule through. A host without a
/// label set (`None`) passes any pattern.
pub fn matches_host(enabled_alerts: &SimplePattern, host_labels: Option<&Labels>, rule: &Rule) -> bool {
    if !enabled_alerts.is_empty() && !enabled_alerts.matches(rule.config.name.as_deref().unwrap_or(b"")) {
        return false;
    }
    match (host_labels, &rule.r#match.host_labels_pattern) {
        (Some(labels), Some(pattern)) => pattern.label_match(labels, b'='),
        _ => true,
    }
}

/// What the matcher reads of a chart.
#[derive(Debug, Clone, Copy)]
pub struct ChartKey<'a> {
    pub id: &'a [u8],
    pub name: &'a [u8],
    pub context: &'a [u8],
    /// `None` for a chart without a label set.
    pub labels: Option<&'a Labels>,
}

/// `health_prototype_matches_rrdset()`: a template is for its context and an alarm for its chart, by id or by
/// name; a rule without `on` matches nothing. Then the rule's chart labels.
pub fn matches_chart(rule: &Rule, chart: &ChartKey<'_>) -> bool {
    let Some(on) = rule.r#match.on.as_deref() else {
        return false;
    };
    let on_matches = if rule.r#match.is_template { on == chart.context } else { on == chart.id || on == chart.name };
    if !on_matches {
        return false;
    }
    match (chart.labels, &rule.r#match.chart_labels_pattern) {
        (Some(labels), Some(pattern)) => pattern.label_match(labels, b'='),
        _ => true,
    }
}

/// `health_prototype_alerts_for_rrdset_incrementally()` without the linking: every stored rule that passes for the
/// chart, in the order C would link them: the prototypes in the store's order; of each enabled one, its alarm rules
/// first and then its templates, each kind in chain order. The alert store keeps the first of a name. Each rule
/// comes with its place in the chain of its name.
pub fn rules_for_chart<'a>(
    prototypes: &'a Prototypes,
    enabled_alerts: &SimplePattern,
    host_labels: Option<&Labels>,
    chart: &ChartKey<'_>,
) -> Vec<(usize, &'a Rule)> {
    let mut rules = Vec::new();
    for (_, prototype) in prototypes.iter() {
        rules.extend(prototype_rules_for_chart(prototype, enabled_alerts, host_labels, chart));
    }
    rules
}

/// `health_prototype_apply_to_rrdset()` without the linking: the rules of one prototype that pass for the chart,
/// its alarm rules first and then its templates; none of a disabled prototype.
pub fn prototype_rules_for_chart<'a>(
    prototype: &'a Prototype,
    enabled_alerts: &SimplePattern,
    host_labels: Option<&Labels>,
    chart: &ChartKey<'_>,
) -> Vec<(usize, &'a Rule)> {
    let mut rules = Vec::new();
    if !prototype.enabled() {
        return rules;
    }
    for want_template in [false, true] {
        for (index, rule) in prototype.rules().iter().enumerate() {
            if rule.r#match.enabled
                && rule.r#match.is_template == want_template
                && matches_host(enabled_alerts, host_labels, rule)
                && matches_chart(rule, chart)
            {
                rules.push((index, rule));
            }
        }
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_space_goes_on_each_side_of_an_equal_sign() {
        let cases: [(&str, &str); 12] = [
            ("a=b", "a=b"),
            ("a = b", "a=b"),
            ("a  =  b", "a = b"),
            ("a= b", "a=b"),
            ("a =b", "a=b"),
            ("a==b", "a==b"),
            ("a= =b", "a==b"),
            ("a = = b", "a== b"),
            ("a=", "a="),
            ("a= ", "a="),
            ("=", "="),
            (" = ", "="),
        ];
        for (text, trimmed) in cases {
            assert_eq!(String::from_utf8(trim_around_equal(text.as_bytes())).unwrap(), trimmed, "{text:?}");
        }
    }

    #[test]
    fn a_text_without_a_value_is_no_array() {
        assert!(label_patterns(b"").is_none());
        assert!(label_patterns(b"a=").is_none());
        assert!(label_patterns(b"a= ").is_none());
        assert!(label_patterns(b"a=b").is_some());
        // a space ends a token even when it is empty
        assert!(label_patterns(b"a=  ").is_some());
    }
}
