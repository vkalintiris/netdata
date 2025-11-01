#![allow(dead_code)]

use super::bitmap::Bitmap;
use super::field_types::{FieldName, FieldValuePair};
use super::file_index::FileIndex;
#[cfg(feature = "allocative")]
use allocative::Allocative;
use std::hash::{Hash, Hasher};

/// Represents what a filter expression can match against.
///
/// This enum distinguishes between:
/// - Matching a field name (e.g., "PRIORITY" matches any PRIORITY value)
/// - Matching a specific field=value pair (e.g., "PRIORITY=error")
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub enum FilterTarget {
    /// Match any entry that has this field, regardless of value
    Field(FieldName),
    /// Match entries where this specific field=value pair exists
    Pair(FieldValuePair),
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub enum FilterExpr<T> {
    None,
    Match(T),
    Conjunction(Vec<Self>),
    Disjunction(Vec<Self>),
}

impl Eq for FilterExpr<FilterTarget> {}

impl Hash for FilterExpr<FilterTarget> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);

        match self {
            FilterExpr::None => {}
            FilterExpr::Match(target) => target.hash(state),
            FilterExpr::Conjunction(filters) => filters.hash(state),
            FilterExpr::Disjunction(filters) => filters.hash(state),
        }
    }
}

impl FilterExpr<FilterTarget> {
    /// Create a filter that matches any entry with the given field name.
    ///
    /// Example: `match_field_name(FieldName::new("PRIORITY"))` matches any entry
    /// that has a PRIORITY field, regardless of its value.
    pub fn match_field_name(name: FieldName) -> Self {
        FilterExpr::Match(FilterTarget::Field(name))
    }

    /// Create a filter that matches a specific field=value pair.
    ///
    /// Example: `match_field_value_pair(pair)` where pair is "PRIORITY=error"
    /// matches only entries where PRIORITY equals "error".
    pub fn match_field_value_pair(pair: FieldValuePair) -> Self {
        FilterExpr::Match(FilterTarget::Pair(pair))
    }

    pub fn and(filters: Vec<Self>) -> Self {
        // Flatten any nested conjunctions and remove None filters
        let mut flattened = Vec::new();
        for filter in filters {
            match filter {
                FilterExpr::Conjunction(inner) => flattened.extend(inner),
                FilterExpr::None => continue,
                other => flattened.push(other),
            }
        }

        match flattened.len() {
            0 => FilterExpr::None,
            1 => flattened.into_iter().next().unwrap(),
            _ => FilterExpr::Conjunction(flattened),
        }
    }

    pub fn or(filters: Vec<Self>) -> Self {
        // Flatten any nested disjunctions and remove None filters
        let mut flattened = Vec::new();
        for filter in filters {
            match filter {
                FilterExpr::Disjunction(inner) => flattened.extend(inner),
                FilterExpr::None => continue,
                other => flattened.push(other),
            }
        }

        match flattened.len() {
            0 => FilterExpr::None,
            1 => flattened.into_iter().next().unwrap(),
            _ => FilterExpr::Disjunction(flattened),
        }
    }

    /// Combines this filter with another using AND logic
    pub fn and_with(self, other: Self) -> Self {
        Self::and(vec![self, other])
    }

    /// Combines this filter with another using OR logic
    pub fn or_with(self, other: Self) -> Self {
        Self::or(vec![self, other])
    }

    /// Convert a FilterExpr<FilterTarget> to FilterExpr<Bitmap> using the file index
    pub fn resolve(&self, file_index: &FileIndex) -> FilterExpr<Bitmap> {
        match self {
            FilterExpr::None => FilterExpr::None,
            FilterExpr::Match(target) => match target {
                FilterTarget::Field(field_name) => {
                    // Find all field=value pairs with matching field name
                    let matches: Vec<_> = file_index
                        .bitmaps()
                        .iter()
                        .filter(|(pair, _)| pair.field() == field_name.as_str())
                        .map(|(_, bitmap)| FilterExpr::Match(bitmap.clone()))
                        .collect();

                    match matches.len() {
                        0 => FilterExpr::None,
                        1 => matches.into_iter().next().unwrap(),
                        _ => FilterExpr::Disjunction(matches),
                    }
                }
                FilterTarget::Pair(pair) => {
                    // Lookup specific field=value pair
                    if let Some(bitmap) = file_index.bitmaps().get(pair) {
                        FilterExpr::Match(bitmap.clone())
                    } else {
                        FilterExpr::None
                    }
                }
            },
            FilterExpr::Conjunction(filters) => {
                let mut resolved = Vec::with_capacity(filters.len());
                for filter in filters {
                    let r = filter.resolve(file_index);
                    if matches!(r, FilterExpr::None) {
                        return FilterExpr::None;
                    }
                    resolved.push(r);
                }

                match resolved.len() {
                    0 => FilterExpr::None,
                    1 => resolved.into_iter().next().unwrap(),
                    _ => FilterExpr::Conjunction(resolved),
                }
            }
            FilterExpr::Disjunction(filters) => {
                let mut resolved = Vec::with_capacity(filters.len());
                for filter in filters {
                    let r = filter.resolve(file_index);
                    if !matches!(r, FilterExpr::None) {
                        resolved.push(r);
                    }
                }

                match resolved.len() {
                    0 => FilterExpr::None,
                    1 => resolved.into_iter().next().unwrap(),
                    _ => FilterExpr::Disjunction(resolved),
                }
            }
        }
    }

    pub fn contains_field(&self, field_name: &FieldName) -> bool {
        match self {
            FilterExpr::None => false,
            FilterExpr::Match(target) => match target {
                FilterTarget::Field(name) => name == field_name,
                FilterTarget::Pair(pair) => pair.field() == field_name.as_str(),
            },
            FilterExpr::Conjunction(filters) | FilterExpr::Disjunction(filters) => {
                filters.iter().any(|fe| fe.contains_field(field_name))
            }
        }
    }

    pub fn contains_pair(&self, pair: &FieldValuePair) -> bool {
        match self {
            FilterExpr::None => false,
            FilterExpr::Match(target) => match target {
                FilterTarget::Field(_) => false,
                FilterTarget::Pair(p) => p == pair,
            },
            FilterExpr::Conjunction(filters) | FilterExpr::Disjunction(filters) => {
                filters.iter().any(|fe| fe.contains_pair(pair))
            }
        }
    }
}

impl std::fmt::Display for FilterExpr<FilterTarget> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterExpr::None => write!(f, "None"),
            FilterExpr::Match(target) => match target {
                FilterTarget::Field(name) => write!(f, "{}", name),
                FilterTarget::Pair(pair) => write!(f, "{}", pair),
            },
            FilterExpr::Conjunction(filters) => {
                write!(f, "(")?;
                for (i, filter) in filters.iter().enumerate() {
                    if i > 0 {
                        write!(f, " AND ")?;
                    }
                    write!(f, "{}", filter)?;
                }
                write!(f, ")")
            }
            FilterExpr::Disjunction(filters) => {
                write!(f, "(")?;
                for (i, filter) in filters.iter().enumerate() {
                    if i > 0 {
                        write!(f, " OR ")?;
                    }
                    write!(f, "{}", filter)?;
                }
                write!(f, ")")
            }
        }
    }
}

impl std::fmt::Display for FilterExpr<String> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterExpr::None => write!(f, "None"),
            FilterExpr::Match(s) => write!(f, "{}", s),
            FilterExpr::Conjunction(filters) => {
                write!(f, "(")?;
                for (i, filter) in filters.iter().enumerate() {
                    if i > 0 {
                        write!(f, " AND ")?;
                    }
                    write!(f, "{}", filter)?;
                }
                write!(f, ")")
            }
            FilterExpr::Disjunction(filters) => {
                write!(f, "(")?;
                for (i, filter) in filters.iter().enumerate() {
                    if i > 0 {
                        write!(f, " OR ")?;
                    }
                    write!(f, "{}", filter)?;
                }
                write!(f, ")")
            }
        }
    }
}

impl FilterExpr<Bitmap> {
    /// Get all entry indices that match this filter expression
    pub fn evaluate(&self) -> Bitmap {
        match self {
            Self::None => Bitmap::new(),
            Self::Match(bitmap) => bitmap.clone(),
            Self::Conjunction(filter_exprs) => {
                if filter_exprs.is_empty() {
                    return Bitmap::new();
                }

                let mut result = filter_exprs[0].evaluate();
                for expr in filter_exprs.iter().skip(1) {
                    result &= expr.evaluate();
                    if result.is_empty() {
                        break; // Early termination for empty conjunction
                    }
                }
                result
            }
            Self::Disjunction(filter_exprs) => {
                let mut result = Bitmap::new();
                for expr in filter_exprs.iter() {
                    result |= expr.evaluate();
                }
                result
            }
        }
    }

    /// Count the number of matching entries
    pub fn count(&self) -> u64 {
        self.evaluate().len()
    }

    /// Check if there are any matching entries
    pub fn has_matches(&self) -> bool {
        match self {
            Self::None => false,
            Self::Match(bitmap) => !bitmap.is_empty(),
            Self::Conjunction(filter_exprs) => filter_exprs.iter().all(|expr| expr.has_matches()),
            Self::Disjunction(filter_exprs) => filter_exprs.iter().any(|expr| expr.has_matches()),
        }
    }
}
