//! The parsers of a `health.d` line's value: `lookup`, `delay`, `repeat`, `options`, `every` (`health_config.c`),
//! and the two delay helpers (`health_internals.h`).
//!
//! C splits these values in place into tokens, a run of bytes that are not `isspace()` each, and reads them in
//! pairs. A value that starts with whitespace has an empty first token, which ends the reading.

use std::borrow::Cow;

use netdata_agent_log::netdata_log_error_errno;
use netdata_agent_query::tables::{TIME_GROUPINGS, TimeGrouping, options};
use netdata_agent_text::c::{self, at, c_str, is_space, skip_spaces};
use netdata_agent_text::duration::duration_parse_seconds;
use netdata_agent_text::parse::{str2ndd, strtof};

use crate::prototype::AlertConfig;
use crate::tables::{ACTION_OPTION_NO_CLEAR_NOTIFICATION, GroupCondition};

/// Bytes as a record prints them.
pub(crate) fn lossy(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

/// C's tokens of a value: each call gives the bytes up to the next whitespace and steps over that whitespace.
struct Tokens<'a> {
    s: &'a [u8],
    pos: usize,
}

impl<'a> Tokens<'a> {
    fn new(s: &'a [u8], pos: usize) -> Self {
        Tokens { s, pos }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.s.len()
    }

    fn next(&mut self) -> &'a [u8] {
        let start = self.pos;
        while self.pos < self.s.len() && !is_space(self.s[self.pos]) {
            self.pos += 1;
        }
        let token = &self.s[start..self.pos];
        while self.pos < self.s.len() && is_space(self.s[self.pos]) {
            self.pos += 1;
        }
        token
    }
}

/// `health_delay_apply_multiplier()`: a delay times the multiplier in C's `float`, bounded by `maximum`.
pub fn delay_apply_multiplier(delay: i32, multiplier: f32, maximum: i32) -> i32 {
    let float_delay = delay as f32;

    // "prove the range widely, then preserve the existing float multiplication and truncation"
    let wide = f64::from(float_delay) * f64::from(multiplier);
    if wide > f64::from(i32::MAX) {
        return maximum;
    }
    if wide < f64::from(i32::MIN) {
        return i32::MIN;
    }

    let multiplied = float_delay * multiplier;
    if f64::from(multiplied) > f64::from(maximum) {
        return maximum;
    }
    if f64::from(multiplied) < f64::from(i32::MIN) {
        return i32::MIN;
    }
    c::double_to_i32(f64::from(multiplied))
}

/// `health_delay_product_exceeds()`: whether a delay times the multiplier is above `value`.
fn delay_product_exceeds(delay: i32, multiplier: f32, value: i32) -> bool {
    let float_delay = delay as f32;
    let wide = f64::from(float_delay) * f64::from(multiplier);
    if wide > f64::from(f32::MAX) {
        return true;
    }
    if wide < -f64::from(f32::MAX) {
        return false;
    }
    (value as f32) < float_delay * multiplier
}

/// `health_parse_delay()`. `up` and `down` that are not given become 0 and the multiplier 1; `max` keeps its value
/// unless a delay times the multiplier is above it.
pub fn parse_delay(
    line: usize,
    filename: &[u8],
    value: &[u8],
    up: &mut i32,
    down: &mut i32,
    max: &mut i32,
    multiplier: &mut f32,
) -> bool {
    let invalid = |value: &[u8], key: &[u8]| {
        netdata_log_error_errno!(
            "Health configuration at line {line} of file '{}': invalid value '{}' for '{}' keyword",
            lossy(filename),
            lossy(value),
            lossy(key)
        );
    };
    let (mut given_up, mut given_down, mut given_max, mut given_multiplier) = (false, false, false, false);

    let mut tokens = Tokens::new(c_str(value), 0);
    while !tokens.at_end() {
        let key = tokens.next();
        if key.is_empty() {
            break;
        }
        let value = tokens.next();

        if key.eq_ignore_ascii_case(b"up") {
            match duration_parse_seconds(value) {
                Some(seconds) => (*up, given_up) = (seconds, true),
                None => invalid(value, key),
            }
        } else if key.eq_ignore_ascii_case(b"down") {
            match duration_parse_seconds(value) {
                Some(seconds) => (*down, given_down) = (seconds, true),
                None => invalid(value, key),
            }
        } else if key.eq_ignore_ascii_case(b"multiplier") {
            *multiplier = strtof(value).0;
            // the rejected value is already written, so it counts as not given: the default below replaces it
            given_multiplier = !(multiplier.is_nan() || multiplier.is_infinite() || *multiplier <= 0.0);
            if !given_multiplier {
                invalid(value, key);
            }
        } else if key.eq_ignore_ascii_case(b"max") {
            match duration_parse_seconds(value) {
                Some(seconds) => (*max, given_max) = (seconds, true),
                None => invalid(value, key),
            }
        } else {
            netdata_log_error_errno!(
                "Health configuration at line {line} of file '{}': unknown keyword '{}'",
                lossy(filename),
                lossy(key)
            );
        }
    }

    if !given_up {
        *up = 0;
    }
    if !given_down {
        *down = 0;
    }
    if !given_multiplier {
        *multiplier = 1.0;
    }
    if !given_max {
        if delay_product_exceeds(*up, *multiplier, *max) {
            *max = delay_apply_multiplier(*up, *multiplier, i32::MAX);
        }
        if delay_product_exceeds(*down, *multiplier, *max) {
            *max = delay_apply_multiplier(*down, *multiplier, i32::MAX);
        }
    }
    true
}

/// `health_parse_options()`: tokens of at most 100 bytes (a longer one is split).
pub fn parse_options(value: &[u8]) -> u8 {
    let s = c_str(value);
    let mut bits = 0;
    let mut i = 0;
    while i < s.len() {
        while i < s.len() && is_space(s[i]) {
            i += 1;
        }
        let start = i;
        while i < s.len() && i - start < 100 && !is_space(s[i]) {
            i += 1;
        }
        let option = &s[start..i];
        if option.is_empty() {
            continue;
        }
        if option.eq_ignore_ascii_case(b"no-clear-notification") || option.eq_ignore_ascii_case(b"no-clear") {
            bits |= ACTION_OPTION_NO_CLEAR_NOTIFICATION;
        } else {
            netdata_log_error_errno!("Ignoring unknown alarm option '{}'", lossy(option));
        }
    }
    bits
}

/// `health_parse_update_every()`: a duration of 0 seconds or more.
pub fn parse_update_every(value: &[u8]) -> Option<i32> {
    duration_parse_seconds(value).filter(|&seconds| seconds >= 0)
}

/// `health_parse_repeat()`.
pub fn parse_repeat(line: usize, filename: &[u8], value: &[u8], warn_repeat_every: &mut u32, crit_repeat_every: &mut u32) -> bool {
    let mut tokens = Tokens::new(c_str(value), 0);
    while !tokens.at_end() {
        let key = tokens.next();
        if key.is_empty() {
            break;
        }
        let value = tokens.next();

        if key.eq_ignore_ascii_case(b"off") {
            *warn_repeat_every = 0;
            *crit_repeat_every = 0;
            return true;
        }
        let target = if key.eq_ignore_ascii_case(b"warning") {
            &mut *warn_repeat_every
        } else if key.eq_ignore_ascii_case(b"critical") {
            &mut *crit_repeat_every
        } else {
            // any other key is skipped with its value, silently
            continue;
        };
        match duration_parse_seconds(value) {
            None => netdata_log_error_errno!(
                "Health configuration at line {line} of file '{}': invalid value '{}' for '{}' keyword",
                lossy(filename),
                lossy(value),
                lossy(key)
            ),
            Some(seconds) if seconds < 0 => netdata_log_error_errno!(
                "Health configuration at line {line} of file '{}': negative value '{}' for '{}' keyword",
                lossy(filename),
                lossy(value),
                lossy(key)
            ),
            Some(seconds) => *target = seconds as u32,
        }
    }
    true
}

/// `time_grouping_parse(name, RRDR_GROUPING_UNDEFINED)`: exact names only.
fn time_grouping(name: &[u8]) -> Option<TimeGrouping> {
    TIME_GROUPINGS.iter().find(|(n, _)| n.as_bytes() == name).map(|&(_, grouping)| grouping)
}

/// `health_parse_db_lookup()`: `METHOD[(CONDITION VALUE)] AFTER [at BEFORE] [every DURATION] [OPTIONS] [of DIMENSIONS]`.
/// False is C's 0. A lookup that fails keeps what it had set by then; the grouping method is not reset at its start.
pub fn parse_db_lookup(line: usize, filename: &[u8], value: &[u8], ac: &mut AlertConfig) -> bool {
    ac.dimensions = None;
    ac.after = 0;
    ac.before = 0;
    ac.update_every = 0;
    ac.options = 0;
    ac.time_group_condition = GroupCondition::Equal;
    ac.time_group_value = f64::NAN;

    let s = c_str(value);
    let file = lossy(filename);

    // first is the group method: up to whitespace or a parenthesis
    let mut i = 0;
    while i < s.len() && !is_space(s[i]) && s[i] != b'(' {
        i += 1;
    }
    let method = &s[..i];
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    if i == s.len() {
        netdata_log_error_errno!(
            "Health configuration invalid chart calculation at line {line} of file '{file}': expected group method followed by the 'after' time, but got '{}'",
            lossy(method)
        );
        return false;
    }

    let group_options = s[i] == b'(';
    if group_options {
        i += 1;
    }

    ac.time_group = time_grouping(method);
    let Some(grouping) = ac.time_group else {
        netdata_log_error_errno!(
            "Health configuration at line {line} of file '{file}': invalid group method '{}'",
            lossy(method)
        );
        return false;
    };

    if group_options {
        i = skip_spaces(s, i);

        match at(s, i) {
            b'!' => {
                i += 1;
                if at(s, i) == b'=' {
                    i += 1;
                }
                ac.time_group_condition = GroupCondition::NotEqual;
            }
            b'<' => {
                i += 1;
                ac.time_group_condition = match at(s, i) {
                    b'>' => {
                        i += 1;
                        GroupCondition::NotEqual
                    }
                    b'=' => {
                        i += 1;
                        GroupCondition::LessEqual
                    }
                    _ => GroupCondition::Less,
                };
            }
            b'>' => {
                i += 1;
                ac.time_group_condition = if at(s, i) == b'=' {
                    i += 1;
                    GroupCondition::GreaterEqual
                } else {
                    GroupCondition::Greater
                };
            }
            // the explicit equal: `=`, `==` or `:`
            b'=' | b':' => {
                i += 1;
                if at(s, i) == b'=' {
                    i += 1;
                }
                ac.time_group_condition = GroupCondition::Equal;
            }
            _ => {}
        }
        i = skip_spaces(s, i);

        let digit = |i: usize| at(s, i).is_ascii_digit();
        let first = at(s, i);
        if first == b')' {
            // empty options, as `countif()`: the defaults apply
        } else if digit(i)
            || (first == b'.' && digit(i + 1))
            || ((first == b'-' || first == b'+') && (digit(i + 1) || (at(s, i + 1) == b'.' && digit(i + 2))))
        {
            let (number, used) = str2ndd(&s[i..]);
            ac.time_group_value = number;
            i = skip_spaces(s, i + used);
            if at(s, i) != b')' {
                netdata_log_error_errno!(
                    "Health configuration at line {line} of file '{file}': missing closing parenthesis after number in aggregation method on '{}'",
                    lossy(method)
                );
                return false;
            }
        } else if first != 0 {
            netdata_log_error_errno!(
                "Health configuration at line {line} of file '{file}': invalid character '{}' in aggregation method options on '{}'",
                lossy(&[first]),
                lossy(method)
            );
            return false;
        } else {
            netdata_log_error_errno!(
                "Health configuration at line {line} of file '{file}': missing closing parenthesis after aggregation method on '{}'",
                lossy(method)
            );
            return false;
        }
        i += 1;
    }

    if ac.time_group_value.is_nan() {
        match grouping {
            TimeGrouping::Countif => ac.time_group_value = 0.0,
            TimeGrouping::TrimmedMean | TimeGrouping::TrimmedMedian => ac.time_group_value = 5.0,
            TimeGrouping::Percentile => ac.time_group_value = 95.0,
            _ => {}
        }
    }

    // then is the 'after' time
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    let mut tokens = Tokens::new(s, i);
    let after = tokens.next();
    match duration_parse_seconds(after) {
        Some(seconds) => ac.after = seconds,
        None => {
            netdata_log_error_errno!(
                "Health configuration at line {line} of file '{file}': invalid duration '{}' after group method",
                lossy(after)
            );
            return false;
        }
    }

    // the default frequency, when its positive magnitude fits an int
    if ac.after >= -i32::MAX {
        ac.update_every = ac.after.abs();
    }

    // the optional parameters
    while !tokens.at_end() {
        let key = tokens.next();
        if key.is_empty() {
            break;
        }
        let is = |name: &[u8]| key.eq_ignore_ascii_case(name);

        if is(b"at") || is(b"every") {
            let value = tokens.next();
            let parsed = if is(b"at") { duration_parse_seconds(value) } else { parse_update_every(value) };
            let Some(seconds) = parsed else {
                netdata_log_error_errno!(
                    "Health configuration at line {line} of file '{file}': invalid duration '{}' for '{}' keyword",
                    lossy(value),
                    lossy(key)
                );
                return false;
            };
            if is(b"at") {
                ac.before = seconds;
            } else {
                ac.update_every = seconds;
            }
        } else if is(b"absolute") || is(b"abs") || is(b"absolute_sum") {
            ac.options |= options::ABSOLUTE;
        } else if is(b"min2max") {
            ac.options |= options::DIMS_MIN2MAX;
        } else if is(b"average") {
            ac.options |= options::DIMS_AVERAGE;
        } else if is(b"min") {
            ac.options |= options::DIMS_MIN;
        } else if is(b"max") {
            ac.options |= options::DIMS_MAX;
        } else if is(b"sum") {
            // the default
        } else if is(b"null2zero") {
            ac.options |= options::NULL2ZERO;
        } else if is(b"percentage") {
            ac.options |= options::PERCENTAGE;
        } else if is(b"unaligned") {
            ac.options |= options::NOT_ALIGNED;
        } else if is(b"anomaly-bit") {
            ac.options |= options::ANOMALY_BIT;
        } else if is(b"match-ids") || is(b"match_ids") {
            ac.options |= options::MATCH_IDS;
        } else if is(b"match-names") || is(b"match_names") {
            ac.options |= options::MATCH_NAMES;
        } else if is(b"of") {
            // the rest is the dimensions, unless it is `all`; ` foreach` ends them and the options go on after it
            let rest = &s[tokens.pos..];
            let mut foreach = None;
            if !rest.is_empty() && !rest.eq_ignore_ascii_case(b"all") {
                foreach = c::find_ignore_case(rest, b" foreach");
                let dimensions = &rest[..foreach.unwrap_or(rest.len())];
                // an empty text is no STRING
                ac.dimensions = (!dimensions.is_empty()).then(|| dimensions.to_vec());
            }
            match foreach {
                None => break,
                Some(at) => tokens.pos += at + 1,
            }
        } else {
            netdata_log_error_errno!(
                "Health configuration at line {line} of file '{file}': unknown keyword '{}'",
                lossy(key)
            );
        }
    }

    true
}
