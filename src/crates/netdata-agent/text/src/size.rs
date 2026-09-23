//! Sizes and entry counts, mirroring `libnetdata/parsers/size.c` and
//! `libnetdata/parsers/entries.c` (the two files share one algorithm).
//!
//! Units are case-sensitive and the unit text is truncated to 3 bytes before
//! lookup, as in C (`10KiBs` is 10 KiB). Text after the unit is ignored.

use crate::c::{self, at, is_alpha, skip_spaces};
use crate::duration::round_to_u64;
use crate::parse::strtod;

/// One row of the C `size_units[]` / `entries_units[]` tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaleUnit {
    pub name: &'static str,
    /// 2 for binary, 10 for decimal units; formatting never mixes bases.
    pub base: u8,
    /// Used when formatting (besides the unit the value is expressed in).
    pub formatter: bool,
    pub multiplier: u64,
}

const fn unit(name: &'static str, base: u8, formatter: bool, multiplier: u64) -> ScaleUnit {
    ScaleUnit {
        name,
        base,
        formatter,
        multiplier,
    }
}

const KI: u64 = 1024;
const K: u64 = 1000;

/// The C `size_units[]` table (smaller to bigger).
pub const SIZE_UNITS: &[ScaleUnit] = &[
    unit("B", 2, true, 1),
    unit("k", 10, false, K),
    unit("K", 10, true, K),
    unit("KB", 10, false, K),
    unit("KiB", 2, true, KI),
    unit("M", 10, true, K * K),
    unit("MB", 10, false, K * K),
    unit("MiB", 2, true, KI * KI),
    unit("G", 10, true, K * K * K),
    unit("GB", 10, false, K * K * K),
    unit("GiB", 2, true, KI * KI * KI),
    unit("T", 10, true, K * K * K * K),
    unit("TB", 10, false, K * K * K * K),
    unit("TiB", 2, true, KI * KI * KI * KI),
    unit("P", 10, true, K * K * K * K * K),
    unit("PB", 10, false, K * K * K * K * K),
    unit("PiB", 2, true, KI * KI * KI * KI * KI),
];

const E: u64 = K * K * K * K * K * K;

/// The C `entries_units[]` table (smaller to bigger). `Z` and `Y` exceed
/// `u64`; C computes them with wrapping `ULL` arithmetic, and so does this.
pub const ENTRIES_UNITS: &[ScaleUnit] = &[
    unit("", 10, true, 1),
    unit("k", 10, false, K),
    unit("K", 10, true, K),
    unit("M", 10, true, K * K),
    unit("G", 10, true, K * K * K),
    unit("T", 10, true, K * K * K * K),
    unit("P", 10, true, K * K * K * K * K),
    unit("E", 10, true, E),
    unit("Z", 10, true, E.wrapping_mul(K)),
    unit("Y", 10, true, E.wrapping_mul(K).wrapping_mul(K)),
];

/// `size_find_unit()` / `entries_find_unit()`: exact match, the first table
/// entry for an empty unit.
fn find_unit(table: &'static [ScaleUnit], name: &[u8]) -> Option<&'static ScaleUnit> {
    let name = if name.is_empty() {
        table[0].name.as_bytes()
    } else {
        name
    };
    table.iter().find(|u| u.name.as_bytes() == name)
}

/// `*_round_to_resolution_dbl2()`: the value in `resolution` units, rounded to 2 decimals.
fn to_units_2_decimals(value: u64, resolution: u64) -> f64 {
    ((value as f64 / resolution as f64) * 100.0).round() / 100.0
}

/// `*_parse()`.
fn parse(table: &'static [ScaleUnit], text: &[u8], default_unit: &str) -> Option<u64> {
    let s = c::c_str(text);
    if s.is_empty() {
        return None;
    }
    let default = find_unit(table, default_unit.as_bytes())?;

    let mut i = skip_spaces(s, 0);
    if &s[i..] == b"off" {
        return Some(0);
    }

    let (value, used) = strtod(&s[i..]);
    if used == 0 || value < 0.0 {
        return None;
    }
    i = skip_spaces(s, i + used);

    let unit_start = i;
    while is_alpha(at(s, i)) {
        i += 1;
    }
    let unit = if i == unit_start {
        default
    } else {
        // the C unit buffer holds 3 characters
        find_unit(table, &s[unit_start..unit_start + (i - unit_start).min(3)])?
    };

    let scaled = round_to_u64(value * unit.multiplier as f64)?;
    let resolution = default.multiplier;
    let quotient = scaled / resolution;
    let remainder = scaled % resolution;
    Some(quotient + u64::from(remainder >= resolution - resolution / 2))
}

/// `*_snprintf()`.
fn format(table: &'static [ScaleUnit], value: u64, unit: &str, accurate: bool) -> Option<String> {
    if value == 0 {
        return Some("off".to_string());
    }
    let default = find_unit(table, unit.as_bytes())?;
    let amount = value.wrapping_mul(default.multiplier);

    // the biggest unit giving at most 2 fractional digits (without loss when `accurate`)
    let mut best = default;
    for su in table {
        let is_default = std::ptr::eq(su, default);
        if su.base != default.base
            || su.multiplier < default.multiplier
            || (!su.formatter && !is_default)
            || (amount < su.multiplier && !is_default)
        {
            continue;
        }
        let converted = to_units_2_decimals(amount, su.multiplier);
        let reversed = c::double_to_u64((converted * su.multiplier as f64).round());
        if converted > 1.0 && (!accurate || reversed == amount) {
            best = su;
        }
    }

    let converted = to_units_2_decimals(amount, best.multiplier);
    let text = if converted == c::double_to_u64(converted) as f64 {
        format!("{converted:.0}{}", best.name)
    } else if converted * 10.0 == c::double_to_u64(converted * 10.0) as f64 {
        format!("{converted:.1}{}", best.name)
    } else {
        format!("{converted:.2}{}", best.name)
    };
    Some(text)
}

/// `size_parse()`: `10MiB`, `1.5 GiB`, `4096` (in `default_unit`), `off`,
/// into a count of `default_unit` (rounded half up).
pub fn size_parse(text: &[u8], default_unit: &str) -> Option<u64> {
    parse(SIZE_UNITS, text, default_unit)
}

/// `size_snprintf()`: `value` (a count of `unit`) with the biggest unit of the
/// same base that needs at most 2 decimals; `off` for 0. `None` where C
/// returns `-3` (unknown unit). C truncates to its buffer; this does not.
pub fn size_to_string(value: u64, unit: &str, accurate: bool) -> Option<String> {
    format(SIZE_UNITS, value, unit, accurate)
}

/// `entries_parse()`: like [`size_parse`] with the `K`/`M`/`G`... decimal units.
pub fn entries_parse(text: &[u8], default_unit: &str) -> Option<u64> {
    parse(ENTRIES_UNITS, text, default_unit)
}

/// `entries_snprintf()`: like [`size_to_string`] with the entries units.
pub fn entries_to_string(value: u64, unit: &str, accurate: bool) -> Option<String> {
    format(ENTRIES_UNITS, value, unit, accurate)
}
