//! Selection matchers and bitmap algebra.
//!
//! A [`Selection`] is `{field, values}`; within a single selection the
//! values are OR'd, and a slice of selections is AND'd across fields —
//! matching the Netdata facet-engine semantics documented in the
//! `otel-signal-viewer-plugin` reference.
//!
//! The resolver walks the per-field [tier](Tier) (low / mid / high) and
//! reads each `field=value` bitmap from the appropriate SFST chunk, then
//! composes them via [`BitmapSet::and`] and [`BitmapSet::or`].

use std::collections::{BTreeMap, HashMap, HashSet};

use log_index::fst_builder::{BitmapValue, FieldEntry, FieldTier};
use log_index::reader::IndexReader;

/// One selection: a field, plus one or more values OR'd together.
#[derive(Debug, Clone)]
pub struct Selection {
    /// Field name (e.g. `service.name`).
    pub field: String,
    /// Values to match for this field. Combined with OR.
    pub values: Vec<String>,
}

/// Where in the SFST file a field's bitmaps live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Field is in the primary FST (low cardinality).
    Primary,
    /// Field has its own mid-cardinality FST chunk.
    Mid,
    /// Field has its own high-cardinality blob chunk.
    High,
    /// Field is absent from the file's field table.
    Missing,
}

impl Tier {
    fn from_field_tier(t: FieldTier) -> Self {
        match t {
            FieldTier::Low => Tier::Primary,
            FieldTier::Mid => Tier::Mid,
            FieldTier::High => Tier::High,
        }
    }
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tier::Primary => f.write_str("primary"),
            Tier::Mid => f.write_str("mid"),
            Tier::High => f.write_str("high"),
            Tier::Missing => f.write_str("missing"),
        }
    }
}

/// Statistics for one resolved [`Selection`].
#[derive(Debug, Clone)]
pub struct SelectionStats {
    /// The field name from the original selection.
    pub field: String,
    /// Where the bitmaps for this field came from.
    pub tier: Tier,
    /// The chunk index for `Mid` / `High` tiers; `None` for `Primary` / `Missing`.
    pub chunk_index: Option<u16>,
    /// Number of positions matched after OR'ing this selection's values.
    pub hits: u64,
}

/// Outcome of resolving a slice of selections against one SFST file.
#[derive(Debug)]
pub struct Resolution {
    /// Final bitmap after AND'ing all selections.
    pub bitmap: BitmapSet,
    /// Per-selection diagnostics, in the order selections were passed in.
    pub per_selection: Vec<SelectionStats>,
}

/// A bitmap descriptor paired with its tree-bytes payload.
///
/// Wraps `treight::Bitmap`'s split (descriptor + external `Vec<u8>`)
/// behind a single owning value so callers don't have to thread the
/// `&[u8]` argument through every operation.
#[derive(Debug, Clone)]
pub struct BitmapSet {
    desc: treight::Bitmap,
    data: Vec<u8>,
}

impl BitmapSet {
    /// Empty set over a universe of `universe_size` positions.
    pub fn empty(universe_size: u32) -> Self {
        Self {
            desc: treight::Bitmap::empty(universe_size),
            data: Vec::new(),
        }
    }

    /// Full set (every position from 0 to `universe_size` is matched).
    pub fn full(universe_size: u32) -> Self {
        Self {
            desc: treight::Bitmap::full(universe_size),
            data: Vec::new(),
        }
    }

    /// Borrow an SFST [`BitmapValue`] into an owned `BitmapSet` (clones data).
    pub fn from_bitmap_value(bv: &BitmapValue) -> Self {
        Self {
            desc: bv.desc,
            data: bv.data.clone(),
        }
    }

    /// Number of set positions.
    pub fn cardinality(&self) -> u64 {
        self.desc.len(&self.data)
    }

    /// `true` if no positions are set.
    pub fn is_empty(&self) -> bool {
        self.desc.is_empty(&self.data)
    }

    /// The descriptor's universe size.
    pub fn universe_size(&self) -> u32 {
        self.desc.universe_size()
    }

    /// Set intersection.
    pub fn and(&self, other: &BitmapSet) -> BitmapSet {
        let mut data = Vec::new();
        let desc = self.desc.and(&self.data, &other.desc, &other.data, &mut data);
        Self { desc, data }
    }

    /// Set union.
    pub fn or(&self, other: &BitmapSet) -> BitmapSet {
        let mut data = Vec::new();
        let desc = self.desc.or(&self.data, &other.desc, &other.data, &mut data);
        Self { desc, data }
    }

    /// Iterate set positions in ascending order.
    pub fn iter(&self) -> treight::BitmapIter<'_> {
        self.desc.iter(&self.data)
    }
}

/// Resolve a slice of selections against an opened SFST.
///
/// `universe_size` should be the file's `total_logs`. Selections with
/// no matching values produce a [`SelectionStats`] entry with
/// `hits = 0` and force the final result toward an empty bitmap (since
/// the resolver AND's across selections).
pub fn resolve(
    index: &IndexReader<'_>,
    field_table: &[FieldEntry],
    selections: &[Selection],
    universe_size: u32,
) -> Result<Resolution, sfst::Error> {
    if selections.is_empty() {
        return Ok(Resolution {
            bitmap: BitmapSet::full(universe_size),
            per_selection: Vec::new(),
        });
    }

    let lookup = build_field_lookup(field_table);

    let mut accumulator: Option<BitmapSet> = None;
    let mut per_selection = Vec::with_capacity(selections.len());

    for selection in selections {
        let stats_field = selection.field.clone();

        let (tier, chunk_index, field_set) =
            match lookup.get(selection.field.as_str()) {
                None => (Tier::Missing, None, BitmapSet::empty(universe_size)),
                Some(&(field_tier, chunk_idx)) => {
                    let tier = Tier::from_field_tier(field_tier);
                    let set = resolve_one(index, selection, field_tier, chunk_idx, universe_size)?;
                    let chunk = match field_tier {
                        FieldTier::Low => None,
                        FieldTier::Mid | FieldTier::High => Some(chunk_idx),
                    };
                    (tier, chunk, set)
                }
            };

        per_selection.push(SelectionStats {
            field: stats_field,
            tier,
            chunk_index,
            hits: field_set.cardinality(),
        });

        accumulator = Some(match accumulator {
            None => field_set,
            Some(acc) => acc.and(&field_set),
        });
    }

    Ok(Resolution {
        bitmap: accumulator.unwrap_or_else(|| BitmapSet::empty(universe_size)),
        per_selection,
    })
}

fn build_field_lookup(field_table: &[FieldEntry]) -> HashMap<&str, (FieldTier, u16)> {
    let mut lookup = HashMap::with_capacity(field_table.len());
    let mut chunk_idx = 0u16;
    for entry in field_table {
        match entry.tier {
            FieldTier::Low => {
                lookup.insert(entry.name.as_str(), (FieldTier::Low, 0));
            }
            FieldTier::Mid | FieldTier::High => {
                lookup.insert(entry.name.as_str(), (entry.tier, chunk_idx));
                chunk_idx += 1;
            }
        }
    }
    lookup
}

fn resolve_one(
    index: &IndexReader<'_>,
    selection: &Selection,
    tier: FieldTier,
    chunk_index: u16,
    universe_size: u32,
) -> Result<BitmapSet, sfst::Error> {
    let mut acc = BitmapSet::empty(universe_size);

    match tier {
        FieldTier::Low => {
            for value in &selection.values {
                let kv = format!("{}={}", selection.field, value);
                if let Some(bv) = index.primary_lookup(kv.as_bytes()) {
                    acc = acc.or(&BitmapSet::from_bitmap_value(bv));
                }
            }
        }
        FieldTier::Mid => {
            let fst = index.load_mid_field(chunk_index)?;
            for value in &selection.values {
                let kv = format!("{}={}", selection.field, value);
                if let Some(bv) = fst.get(kv.as_bytes()) {
                    acc = acc.or(&BitmapSet::from_bitmap_value(bv));
                }
            }
        }
        FieldTier::High => {
            let entries = index.load_high_field(chunk_index)?;
            let targets: HashSet<String> = selection
                .values
                .iter()
                .map(|v| format!("{}={}", selection.field, v))
                .collect();
            for (key, bv) in &entries {
                if targets.contains(key) {
                    acc = acc.or(&BitmapSet::from_bitmap_value(bv));
                }
            }
        }
    }

    Ok(acc)
}

/// Parse `--select FIELD=VALUE` raw strings into grouped [`Selection`]s.
///
/// Multiple flags with the same field name are merged: their values
/// become the `Selection::values` vector (which the resolver OR's).
/// Selections are returned in alphabetical field order so output is
/// deterministic across runs.
pub fn parse_selections(raw: &[String]) -> Result<Vec<Selection>, ParseError> {
    let mut by_field: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for s in raw {
        let (field, value) = s
            .split_once('=')
            .ok_or_else(|| ParseError::MissingEquals(s.clone()))?;
        if field.is_empty() {
            return Err(ParseError::EmptyField(s.clone()));
        }
        by_field
            .entry(field.to_string())
            .or_default()
            .push(value.to_string());
    }
    Ok(by_field
        .into_iter()
        .map(|(field, values)| Selection { field, values })
        .collect())
}

/// Failure parsing a `--select` argument.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// The selection string had no `=` separator.
    #[error("invalid --select `{0}`: expected FIELD=VALUE")]
    MissingEquals(String),
    /// The field side of the `=` was empty.
    #[error("invalid --select `{0}`: empty field name")]
    EmptyField(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap_with(positions: &[u32], universe: u32) -> BitmapSet {
        let mut data = Vec::new();
        let desc =
            treight::Bitmap::from_sorted_iter(positions.iter().copied(), universe, &mut data);
        BitmapSet { desc, data }
    }

    fn collect(set: &BitmapSet) -> Vec<u32> {
        set.iter().collect()
    }

    #[test]
    fn empty_set_has_no_positions() {
        let s = BitmapSet::empty(100);
        assert_eq!(s.cardinality(), 0);
        assert!(s.is_empty());
        assert!(collect(&s).is_empty());
    }

    #[test]
    fn full_set_covers_universe() {
        let s = BitmapSet::full(10);
        assert_eq!(s.cardinality(), 10);
        assert!(!s.is_empty());
        assert_eq!(collect(&s), (0u32..10).collect::<Vec<_>>());
    }

    #[test]
    fn and_intersects() {
        let a = bitmap_with(&[1, 3, 5, 7], 10);
        let b = bitmap_with(&[2, 3, 5, 8], 10);
        let c = a.and(&b);
        assert_eq!(collect(&c), vec![3, 5]);
        assert_eq!(c.cardinality(), 2);
    }

    #[test]
    fn or_unions() {
        let a = bitmap_with(&[1, 3], 10);
        let b = bitmap_with(&[2, 3, 4], 10);
        let c = a.or(&b);
        assert_eq!(collect(&c), vec![1, 2, 3, 4]);
        assert_eq!(c.cardinality(), 4);
    }

    #[test]
    fn and_with_empty_is_empty() {
        let a = bitmap_with(&[1, 2, 3], 10);
        let e = BitmapSet::empty(10);
        let c = a.and(&e);
        assert!(c.is_empty());
    }

    #[test]
    fn or_with_empty_is_self() {
        let a = bitmap_with(&[1, 2, 3], 10);
        let e = BitmapSet::empty(10);
        let c = a.or(&e);
        assert_eq!(collect(&c), vec![1, 2, 3]);
    }

    #[test]
    fn and_with_full_is_self() {
        let a = bitmap_with(&[1, 2, 3], 10);
        let f = BitmapSet::full(10);
        let c = a.and(&f);
        assert_eq!(collect(&c), vec![1, 2, 3]);
    }

    #[test]
    fn parse_groups_values_by_field() {
        let raw = vec![
            "level=ERROR".to_string(),
            "service=api".to_string(),
            "level=WARN".to_string(),
        ];
        let selections = parse_selections(&raw).unwrap();
        assert_eq!(selections.len(), 2);
        // BTreeMap order: alphabetical → level, service.
        assert_eq!(selections[0].field, "level");
        assert_eq!(selections[0].values, vec!["ERROR", "WARN"]);
        assert_eq!(selections[1].field, "service");
        assert_eq!(selections[1].values, vec!["api"]);
    }

    #[test]
    fn parse_rejects_missing_equals() {
        let err = parse_selections(&["levelERROR".to_string()]).unwrap_err();
        assert!(matches!(err, ParseError::MissingEquals(_)));
    }

    #[test]
    fn parse_rejects_empty_field() {
        let err = parse_selections(&["=value".to_string()]).unwrap_err();
        assert!(matches!(err, ParseError::EmptyField(_)));
    }

    #[test]
    fn parse_allows_empty_value() {
        // `service.namespace=` matches lines with an empty namespace.
        let sel = parse_selections(&["foo=".to_string()]).unwrap();
        assert_eq!(sel[0].field, "foo");
        assert_eq!(sel[0].values, vec![""]);
    }

    #[test]
    fn parse_allows_equals_in_value() {
        // split_once on first `=` so `foo=a=b` -> ("foo", "a=b").
        let sel = parse_selections(&["foo=a=b".to_string()]).unwrap();
        assert_eq!(sel[0].field, "foo");
        assert_eq!(sel[0].values, vec!["a=b"]);
    }
}
