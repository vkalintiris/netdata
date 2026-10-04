//! `${family}` and `${label:NAME}` in an alert's `info` and `summary` (`rrdcalc.c`
//! `rrdcalc_replace_variables_with_rrdset_labels()`).

use netdata_agent_rrd::labels::Labels;

/// `RRDCALC_VAR_MAX`: a token is copied into a buffer of this size, so 99 of its bytes are looked at.
const VAR_MAX: usize = 100;
/// `RRDCALC_VAR_FAMILY`.
const VAR_FAMILY: &[u8] = b"${family}";
/// `RRDCALC_VAR_LABEL`.
const VAR_LABEL: &[u8] = b"${label:";

/// The text with the chart's family and label values in place of their tokens; `None` for an empty result (C's NULL
/// string, for which the alert shows the configured text).
///
/// The scan goes from `$` to `$` and ends for good at the first one that is not followed by `{`. A token is the
/// bytes up to its `}`, 99 at most: a longer one is never replaced. `${family}` always is; `${label:NAME}` only when
/// the chart has that label (an unknown one stays as written). The scan goes on after the replaced text, or one byte
/// after a `$` that was left.
pub fn replace_variables_with_labels(line: &[u8], family: &[u8], labels: Option<&Labels>) -> Option<Vec<u8>> {
    if line.is_empty() {
        return None;
    }
    let mut text = line.to_vec();
    let mut pos = 0;
    while let Some(offset) = text[pos..].iter().position(|&c| c == b'$') {
        let at = pos + offset;
        if text.get(at + 1) != Some(&b'{') {
            break;
        }
        let limit = (at + VAR_MAX - 1).min(text.len());
        let token = match text[at..limit].iter().position(|&c| c == b'}') {
            Some(close) => &text[at..=at + close],
            None => &text[at..limit],
        };
        let replacement: Option<Vec<u8>> = if token == VAR_FAMILY {
            Some(family.to_vec())
        } else if token.starts_with(VAR_LABEL) && token.len() > VAR_LABEL.len() && token.ends_with(b"}") {
            labels.and_then(|labels| labels.get(&token[VAR_LABEL.len()..token.len() - 1])).map(<[u8]>::to_vec)
        } else {
            None
        };
        pos = at + 1;
        if let Some(value) = replacement {
            let token_len = token.len();
            pos = at + value.len();
            text.splice(at..at + token_len, value);
        }
    }
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use netdata_agent_rrd::labels::SRC_CONFIG;

    use super::*;

    fn labels() -> Labels {
        let mut labels = Labels::default();
        labels.add(b"device", b"sda", SRC_CONFIG);
        labels.add(b"nested", b"${family}", SRC_CONFIG);
        labels.add(b"dollar", b"a$b", SRC_CONFIG);
        labels
    }

    fn replaced(line: &str, family: &str) -> Option<String> {
        replace_variables_with_labels(line.as_bytes(), family.as_bytes(), Some(&labels()))
            .map(|text| String::from_utf8(text).unwrap())
    }

    #[test]
    fn the_family_and_known_labels_replace_their_tokens() {
        // what the label set holds after its sanitizer
        let stored = |name: &[u8]| String::from_utf8(labels().get(name).unwrap().to_vec()).unwrap();
        let (nested, dollar) = (stored(b"nested"), format!("{} disk", stored(b"dollar")));
        let cases: [(&str, &str, Option<&str>); 16] = [
            ("", "disk", None),
            ("plain", "disk", Some("plain")),
            ("on ${family}", "disk", Some("on disk")),
            ("${family} and ${family}", "disk", Some("disk and disk")),
            ("${label:device} of ${family}", "disk", Some("sda of disk")),
            // an unknown label stays as written, and the scan goes on
            ("${label:nope} of ${family}", "disk", Some("${label:nope} of disk")),
            ("${label:} x", "disk", Some("${label:} x")),
            // any other token stays too
            ("${other} ${family}", "disk", Some("${other} disk")),
            // a `$` that opens no token ends the scan for good
            ("$x ${family}", "disk", Some("$x ${family}")),
            ("cost $ ${family}", "disk", Some("cost $ ${family}")),
            ("${family} $", "disk", Some("disk $")),
            // an empty family empties its token; an empty result is no text
            ("${family}", "", None),
            ("a${family}b", "", Some("ab")),
            // the scan goes on after the replaced text: what a value holds is not looked at again
            ("${label:nested}", "disk", Some(&nested)),
            // a value with a `$` inside: the scan resumes after the value
            ("${label:dollar} ${family}", "disk", Some(&dollar)),
            ("${family", "disk", Some("${family")),
        ];
        for (line, family, expected) in cases {
            assert_eq!(replaced(line, family).as_deref(), expected, "{line:?}");
        }
    }

    #[test]
    fn a_token_is_read_99_bytes_deep() {
        // `${label:` and a name that puts the closing brace at byte 99 of the token: the last that is looked at
        let name = "n".repeat(99 - "${label:".len() - 1);
        let mut labels = Labels::default();
        labels.add(name.as_bytes(), b"v", SRC_CONFIG);
        let fits = format!("${{label:{name}}}");
        assert_eq!(fits.len(), 99);
        assert_eq!(replace_variables_with_labels(fits.as_bytes(), b"f", Some(&labels)), Some(b"v".to_vec()));

        // one byte more and the brace is not seen: the token is never replaced
        let name = format!("{name}n");
        let mut labels = Labels::default();
        labels.add(name.as_bytes(), b"v", SRC_CONFIG);
        let long = format!("${{label:{name}}}");
        assert_eq!(long.len(), 100);
        assert_eq!(replace_variables_with_labels(long.as_bytes(), b"f", Some(&labels)), Some(long.into_bytes()));
    }

    #[test]
    fn without_a_label_set_only_the_family_is_replaced() {
        assert_eq!(
            replace_variables_with_labels(b"${label:device} ${family}", b"disk", None),
            Some(b"${label:device} disk".to_vec())
        );
    }
}
