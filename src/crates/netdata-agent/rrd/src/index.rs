//! A dictionary of the objects of a contexts tree or of a host (C's `DICTIONARY`): its items in creation order, which
//! the walks follow, with an index by id.

use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct Index<T> {
    ordered: Vec<Arc<T>>,
    by_id: HashMap<String, usize>,
}

impl<T> Default for Index<T> {
    fn default() -> Self {
        Index {
            ordered: Vec::new(),
            by_id: HashMap::new(),
        }
    }
}

impl<T> Index<T> {
    pub(crate) fn get(&self, id: &str) -> Option<Arc<T>> {
        self.by_id.get(id).map(|&i| Arc::clone(&self.ordered[i]))
    }

    pub(crate) fn insert(&mut self, id: &str, item: Arc<T>) {
        self.by_id.insert(id.to_string(), self.ordered.len());
        self.ordered.push(item);
    }

    /// Takes the item out, the others keeping their order.
    pub(crate) fn remove(&mut self, id: &str) -> Option<Arc<T>> {
        let i = self.by_id.remove(id)?;
        let item = self.ordered.remove(i);
        for position in self.by_id.values_mut() {
            if *position > i {
                *position -= 1;
            }
        }
        Some(item)
    }

    /// The items in creation order.
    pub(crate) fn items(&self) -> &[Arc<T>] {
        &self.ordered
    }

    /// Keeps the items `keep` accepts, in order; the number removed.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&Arc<T>) -> bool) -> usize {
        let mut ids: Vec<Option<String>> = vec![None; self.ordered.len()];
        for (id, &i) in &self.by_id {
            ids[i] = Some(id.clone());
        }
        self.by_id.clear();
        let mut removed = 0;
        for (item, id) in std::mem::take(&mut self.ordered).into_iter().zip(ids) {
            if keep(&item) {
                if let Some(id) = id {
                    self.by_id.insert(id, self.ordered.len());
                }
                self.ordered.push(item);
            } else {
                removed += 1;
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(index: &Index<String>) -> Vec<&str> {
        index.items().iter().map(|s| s.as_str()).collect()
    }

    #[test]
    fn removal_keeps_the_order_and_the_positions() {
        let mut index = Index::default();
        for id in ["a", "b", "c", "d"] {
            index.insert(id, Arc::new(id.to_string()));
        }
        assert_eq!(index.remove("b").as_deref().map(String::as_str), Some("b"));
        assert_eq!(index.remove("b"), None);
        assert_eq!(ids(&index), ["a", "c", "d"]);
        assert_eq!(index.get("d").as_deref().map(String::as_str), Some("d"));
        assert_eq!(index.retain(|s| s.as_str() != "a"), 1);
        assert_eq!(ids(&index), ["c", "d"]);
        assert_eq!(index.get("c").as_deref().map(String::as_str), Some("c"));
        assert_eq!(index.get("a"), None);
        index.insert("e", Arc::new("e".to_string()));
        assert_eq!(index.get("e").as_deref().map(String::as_str), Some("e"));
    }
}
