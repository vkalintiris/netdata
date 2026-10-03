//! `src/libnetdata/simple_hashtable/simple_hashtable.h`: open addressing over a slot array, as C lays it out, so a
//! walk of the slots gives C's order. A key's slot is its hash modulo the size, then the hash's high half plus one,
//! then the next slots; a deleted slot is a tombstone, reused only when the probe ends on an empty one; a lookup may
//! grow the table (half of it live, or every slot used) or rebuild it at its size (a probe longer than half the table
//! while a third of it is tombstones). Keys are kept beside the values (C reads them through its value).

/// `SIMPLE_HASHTABLE_HASH_SECOND_HASH_SHIFTS`.
const SECOND_HASH_SHIFTS: u32 = 32;

#[derive(Debug, Clone)]
enum Slot<K, V> {
    Unset,
    Deleted,
    Data { hash: u64, key: K, value: V },
}

/// `SIMPLE_HASHTABLE` with keys compared (`SIMPLE_HASHTABLE_COMPARE_KEYS_FUNCTION`).
#[derive(Debug)]
pub(crate) struct SimpleHashtable<K, V> {
    slots: Vec<Slot<K, V>>,
    used: usize,
    deleted: usize,
    needs_cleanup: bool,
    collisions: usize,
}

impl<K: Clone + PartialEq, V: Clone> SimpleHashtable<K, V> {
    /// `simple_hashtable_init()`.
    pub(crate) fn new(size: usize) -> Self {
        let size = size.max(1);
        SimpleHashtable { slots: vec![Slot::Unset; size], used: 0, deleted: 0, needs_cleanup: false, collisions: 0 }
    }

    fn size(&self) -> usize {
        self.slots.len()
    }

    /// `SIMPLE_HASHTABLE_NEEDS_RESIZE()`.
    fn needs_resize(&self) -> bool {
        let size = self.size();
        self.used >= size || self.used - self.deleted >= size / 2 + (size & 1)
    }

    /// `simple_hashtable_can_use_slot()`.
    fn can_use(&self, slot: usize, hash: u64, key: &K) -> bool {
        match &self.slots[slot] {
            Slot::Unset => true,
            Slot::Deleted => false,
            Slot::Data { hash: h, key: k, .. } => *h == hash && k == key,
        }
    }

    fn is_deleted(&self, slot: usize) -> bool {
        matches!(self.slots[slot], Slot::Deleted)
    }

    fn is_unset(&self, slot: usize) -> bool {
        matches!(self.slots[slot], Slot::Unset)
    }

    /// `simple_hashtable_get_slot()`: the key's slot, or where it would go (the first tombstone met before an empty
    /// slot); with `resize`, the table may grow or be rebuilt first.
    pub(crate) fn get_slot(&mut self, hash: u64, key: &K, resize: bool) -> usize {
        let mut slot = (hash % self.size() as u64) as usize;
        let mut deleted = self.is_deleted(slot).then_some(slot);
        if self.can_use(slot, hash, key) {
            return match deleted {
                Some(d) if self.is_unset(slot) => d,
                _ => slot,
            };
        }
        self.collisions += 1;
        if resize && (self.needs_cleanup || self.needs_resize()) {
            self.resize();
            deleted = None;
            slot = (hash % self.size() as u64) as usize;
            if self.can_use(slot, hash, key) {
                return slot;
            }
            self.collisions += 1;
        }
        slot = ((hash >> SECOND_HASH_SHIFTS).wrapping_add(1) % self.size() as u64) as usize;
        if deleted.is_none() && self.is_deleted(slot) {
            deleted = Some(slot);
        }
        let started = slot;
        let collisions_started = self.collisions;
        while !self.can_use(slot, hash, key) {
            slot = (slot + 1) % self.size();
            if deleted.is_none() && self.is_deleted(slot) {
                deleted = Some(slot);
            }
            self.collisions += 1;
            if slot == started {
                if let Some(d) = deleted {
                    return d;
                }
                // full without a tombstone: only a lookup that may resize gets here (C fatal()s otherwise)
                self.resize();
                return self.get_slot(hash, key, false);
            }
        }
        if self.collisions - collisions_started > self.size() / 2 && self.deleted >= self.size() / 3 {
            self.needs_cleanup = true;
        }
        match deleted {
            Some(d) if self.is_unset(slot) => d,
            _ => slot,
        }
    }

    /// The value at a slot, if it holds one.
    pub(crate) fn value(&self, slot: usize) -> Option<&V> {
        match &self.slots[slot] {
            Slot::Data { value, .. } => Some(value),
            _ => None,
        }
    }

    /// `simple_hashtable_set_slot()`.
    pub(crate) fn set_slot(&mut self, slot: usize, hash: u64, key: K, value: V) {
        match self.slots[slot] {
            Slot::Unset => self.used += 1,
            Slot::Deleted => self.deleted -= 1,
            Slot::Data { .. } => {}
        }
        self.slots[slot] = Slot::Data { hash, key, value };
    }

    /// `simple_hashtable_del_slot()`: the slot becomes a tombstone.
    pub(crate) fn del_slot(&mut self, slot: usize) -> bool {
        if !matches!(self.slots[slot], Slot::Data { .. }) {
            return false;
        }
        self.deleted += 1;
        self.slots[slot] = Slot::Deleted;
        true
    }

    /// `simple_hashtable_resize()`: twice the size less one past 16 slots when half full, else the same size; the
    /// values rehashed in slot order, the tombstones dropped.
    fn resize(&mut self) {
        let size = self.size();
        let new_size = if self.needs_resize() { size * 2 - usize::from(size > 16) } else { size };
        let old = std::mem::replace(&mut self.slots, vec![Slot::Unset; new_size]);
        let mut used = 0;
        for slot in old {
            if let Slot::Data { hash, key, value } = slot {
                let at = self.get_slot(hash, &key, false);
                self.slots[at] = Slot::Data { hash, key, value };
                used += 1;
            }
        }
        self.used = used;
        self.deleted = 0;
        self.needs_cleanup = false;
    }

    /// `SIMPLE_HASHTABLE_FOREACH_READ_ONLY`: the values in slot order.
    pub(crate) fn values(&self) -> impl Iterator<Item = (&K, &V)> {
        self.slots.iter().filter_map(|s| match s {
            Slot::Data { key, value, .. } => Some((key, value)),
            _ => None,
        })
    }
}
