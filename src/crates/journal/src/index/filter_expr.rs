#![allow(dead_code)]

use super::bitmap::Bitmap;
use super::file_index::FileIndex;
#[cfg(feature = "allocative")]
use allocative::Allocative;
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub enum FilterExpr<T> {
    None,
    Match(T),
    Conjunction(Vec<Self>),
    Disjunction(Vec<Self>),
}

impl Eq for FilterExpr<String> {}

impl Hash for FilterExpr<String> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);

        match self {
            FilterExpr::None => {}
            FilterExpr::Match(s) => s.hash(state),
            FilterExpr::Conjunction(filters) => filters.hash(state),
            FilterExpr::Disjunction(filters) => filters.hash(state),
        }
    }
}

impl FilterExpr<String> {
    pub fn match_str(s: impl Into<String>) -> Self {
        FilterExpr::Match(s.into())
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

    /// Convert a FilterExpr<String> to FilterExpr<Bitmap> using the file index
    pub fn resolve(&self, file_index: &FileIndex) -> FilterExpr<Bitmap> {
        match self {
            FilterExpr::None => FilterExpr::None,
            FilterExpr::Match(field_value) => {
                // Check if this is a complete field=value or just a field key
                if field_value.contains('=') {
                    // Complete field=value pair
                    if let Some(bitmap) = file_index.bitmaps().get(field_value) {
                        FilterExpr::Match(bitmap.clone())
                    } else {
                        FilterExpr::None
                    }
                } else {
                    // Just a field key - find all matching field=value pairs
                    let prefix = format!("{}=", field_value);
                    let matches: Vec<_> = file_index
                        .bitmaps()
                        .iter()
                        .filter(|(key, _)| key.starts_with(&prefix))
                        .map(|(_, bitmap)| FilterExpr::Match(bitmap.clone()))
                        .collect();

                    match matches.len() {
                        0 => FilterExpr::None,
                        1 => matches.into_iter().next().unwrap(),
                        _ => FilterExpr::Disjunction(matches),
                    }
                }
            }
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

    pub fn contains(&self, s: &str) -> bool {
        match self {
            FilterExpr::None => false,
            FilterExpr::Match(field_value) => {
                if field_value == s {
                    true
                } else {
                    false
                }
            }
            FilterExpr::Conjunction(filters) | FilterExpr::Disjunction(filters) => {
                for fe in filters {
                    if fe.contains(s) {
                        return true;
                    }
                }

                false
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
