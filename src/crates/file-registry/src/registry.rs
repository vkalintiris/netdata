use std::borrow::Borrow;
use std::collections::BTreeMap;

/// An ordered keyed collection with per-entry metadata.
///
/// Generic over the key type so callers can pick the natural identifier for
/// their domain (sequence numbers for WAL/SFST, on-disk paths for catalog
/// files keyed by `(date, machine, boot, max_seq)`). Path derivation and
/// directory scanning are not part of this type — callers that need them
/// own a [`FileDir`](crate::FileDir) alongside.
pub struct FileRegistry<K, M> {
    files: BTreeMap<K, M>,
}

impl<K: Ord, M> FileRegistry<K, M> {
    pub fn new() -> Self {
        Self {
            files: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, key: K, entry: M) -> Option<M> {
        self.files.insert(key, entry)
    }

    pub fn remove<Q>(&mut self, key: &Q) -> Option<M>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.files.remove(key)
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&M>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.files.get(key)
    }

    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut M>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.files.get_mut(key)
    }

    pub fn contains<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.files.contains_key(key)
    }
}

impl<K, M> FileRegistry<K, M> {
    pub fn values(&self) -> impl Iterator<Item = &M> {
        self.files.values()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &M)> {
        self.files.iter()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl<K: Ord, M> Default for FileRegistry<K, M> {
    fn default() -> Self {
        Self::new()
    }
}
