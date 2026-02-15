use std::io;
use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Sub, SubAssign};

/// Compute the number of levels in the tree8 for a given universe size.
///
/// Returns 0 for universe_size == 0.
pub fn ceil_log8(universe_size: u32) -> u32 {
    if universe_size == 0 {
        return 0;
    }

    let mut levels: u32 = 1;
    let mut n = universe_size - 1;
    while n >> 3 != 0 {
        n >>= 3;
        levels += 1;
    }

    levels
}

/// Compute the exact tree8 data size for a sorted sequence of values,
/// without actually building the tree.
///
/// This is useful for deciding whether converting a roaring bitmap to tree8
/// would save space:
///
/// ```ignore
/// let tree8_bytes = tree8::estimate_data_size(universe_size, roaring_bm.iter());
/// let roaring_bytes = roaring_bm.serialized_size();
/// if tree8_bytes < roaring_bytes { /* convert */ }
/// ```
///
/// The values **must** be yielded in ascending order (as roaring iterators do).
/// The result equals the `data().len()` of the `RawBitmap` that would be
/// built from these values.
pub fn estimate_data_size(universe_size: u32, sorted_values: impl Iterator<Item = u32>) -> usize {
    let levels = ceil_log8(universe_size);
    if levels == 0 {
        return 0;
    }

    // Track the last-seen node index at each level (excluding root).

    const MAX_INNER_LEVELS: usize = 11;
    let mut prev_node = [u32::MAX; MAX_INNER_LEVELS];
    let inner_levels = levels as usize - 1;

    let mut total: usize = 0;
    let mut any = false;

    for v in sorted_values {
        any = true;

        for (k, prev) in prev_node[..inner_levels].iter_mut().enumerate() {
            let node = v >> (3 * (k + 1));

            if node != *prev {
                total += 1;
                *prev = node;
            }
        }
    }

    if any {
        total += 1; // root byte
    }

    total
}

/// A compressed bitmap using an 8-way bit-tree.
///
/// Each internal node is a single byte whose 8 bits indicate which of its
/// 8 children are present. Empty subtrees are pruned entirely. The serialized
/// form IS the in-memory form — point queries traverse it in O(levels).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "allocative", derive(allocative::Allocative))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RawBitmap {
    universe_size: u32,
    levels: u32,
    data: Vec<u8>,
}

impl RawBitmap {
    /// Create an empty bitmap for the given universe size.
    pub fn empty(universe_size: u32) -> Self {
        Self {
            universe_size,
            levels: ceil_log8(universe_size),
            data: Vec::new(),
        }
    }

    /// Build a `RawBitmap` directly from a sorted iterator of values.
    ///
    /// Values **must** be in ascending order (duplicates are tolerated).
    /// Builds the depth-first pre-order serialization in a single pass,
    /// pushing new node bytes and back-patching child bits as groups change.
    ///
    /// Cost: O(N × levels) time, O(output_size) space — no bitvec allocation.
    pub fn from_sorted_iter(iter: impl Iterator<Item = u32>, universe_size: u32) -> Self {
        let levels = ceil_log8(universe_size);
        if levels == 0 {
            return Self::empty(universe_size);
        }

        let mut data = Vec::new();
        let mut prev_group = [u32::MAX; 11];
        let mut node_pos = [0usize; 11];

        for v in iter {
            // From root (highest level) down to leaf (level 0):
            // if the node group changed, push a new node byte.
            // Then OR the child bit into the current node at each level.
            for dl in (0..levels).rev() {
                let group = v.checked_shr(3 * (dl + 1)).unwrap_or(0);

                if group != prev_group[dl as usize] {
                    prev_group[dl as usize] = group;
                    node_pos[dl as usize] = data.len();
                    data.push(0);
                }

                data[node_pos[dl as usize]] |= 1u8 << child_index(dl, v);
            }
        }

        Self {
            universe_size,
            levels,
            data,
        }
    }

    /// Build a `RawBitmap` with all values in the given range set.
    ///
    /// Values outside `0..universe_size` are clamped/ignored.
    pub fn from_range(range: impl std::ops::RangeBounds<u32>, universe_size: u32) -> Self {
        use std::ops::Bound;

        let start = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n.saturating_add(1),
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&n) => n.saturating_add(1),
            Bound::Excluded(&n) => n,
            Bound::Unbounded => universe_size,
        };
        let end = end.min(universe_size);

        if start >= end {
            return Self::empty(universe_size);
        }

        Self::from_sorted_iter(start..end, universe_size)
    }

    /// The universe size (exclusive upper bound on values).
    pub fn universe_size(&self) -> u32 {
        self.universe_size
    }

    /// The number of levels in the tree.
    pub fn levels(&self) -> u32 {
        self.levels
    }

    /// The raw tree8 data bytes.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Create a cursor for reading nodes sequentially.
    fn nodes(&self) -> NodeReader<'_> {
        NodeReader {
            nodes: &self.data,
            pos: 0,
        }
    }

    /// Test whether `value` is in the bitmap.
    ///
    /// Matches `is_hit_1` from GNU idutils `src/fid.c:260-278`.
    pub fn contains(&self, value: u32) -> bool {
        if value >= self.universe_size || self.data.is_empty() {
            return false;
        }

        contains_inner(&mut self.nodes(), self.levels, value)
    }

    /// Iterate over set bits in ascending order.
    pub fn iter(&self) -> Iter<'_> {
        if self.data.is_empty() {
            return Iter::empty();
        }

        let mut iter = Iter::empty();
        iter.data = &self.data;

        let child_level = self.levels - 1;
        if child_level == 0 {
            // Single-level tree: root byte IS the leaf.
            iter.leaf_bits = self.data[0];
            iter.pos = 1;
        } else {
            // Multi-level tree: push root frame and descend to first leaf.
            let root = self.data[0];
            iter.pos = 1;
            iter.stack[0] = IterFrame {
                node_bits: root,
                next_child: 0,
                base: 0,
                child_level,
            };
            iter.stack_len = 1;
            iter.advance_to_next_leaf();
        }

        iter
    }

    /// Count the number of set bits (population count).
    ///
    /// Walks the tree and sums leaf byte popcounts directly.
    pub fn len(&self) -> u64 {
        if self.data.is_empty() {
            return 0;
        }

        self.nodes().skip_subtree(self.levels)
    }

    /// Returns `true` if no bits are set.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The smallest set value, or `None` if empty.
    pub fn min(&self) -> Option<u32> {
        if self.data.is_empty() {
            return None;
        }

        Some(self.nodes().min_value(self.levels))
    }

    /// The largest set value, or `None` if empty.
    pub fn max(&self) -> Option<u32> {
        if self.data.is_empty() {
            return None;
        }

        Some(self.nodes().max_value(self.levels))
    }

    /// Count the number of set bits within a range.
    ///
    /// Walks only the subtrees that overlap the range — subtrees fully inside
    /// the range are counted via popcount without per-value work, and subtrees
    /// fully outside are skipped entirely.
    pub fn range_cardinality(&self, range: impl std::ops::RangeBounds<u32>) -> u64 {
        use std::ops::Bound;

        if self.data.is_empty() {
            return 0;
        }

        let start = match range.start_bound() {
            Bound::Included(&n) => n as u64,
            Bound::Excluded(&n) => n as u64 + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&n) => n as u64 + 1,
            Bound::Excluded(&n) => n as u64,
            Bound::Unbounded => self.universe_size as u64,
        };

        if start >= end {
            return 0;
        }

        range_count(&mut self.nodes(), self.levels, 0, start, end)
    }

    /// Serialize to a writer.
    ///
    /// Wire format: `[universe_size: u32 LE][data_len: u32 LE][data bytes]`.
    pub fn serialize_into<W: io::Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_all(&self.universe_size.to_le_bytes())?;
        writer.write_all(&(self.data.len() as u32).to_le_bytes())?;
        writer.write_all(&self.data)?;
        Ok(())
    }

    /// Deserialize from a reader.
    pub fn deserialize_from<R: io::Read>(mut reader: R) -> io::Result<Self> {
        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf)?;
        let universe_size = u32::from_le_bytes(buf);

        reader.read_exact(&mut buf)?;
        let data_len = u32::from_le_bytes(buf) as usize;

        let mut data = vec![0u8; data_len];
        reader.read_exact(&mut data)?;

        let levels = ceil_log8(universe_size);

        Ok(Self {
            universe_size,
            levels,
            data,
        })
    }

    /// The number of bytes this bitmap occupies when serialized.
    pub fn serialized_size(&self) -> usize {
        8 + self.data.len()
    }

    /// The number of heap-allocated bytes used by this bitmap.
    pub fn heap_bytes(&self) -> usize {
        self.data.len()
    }

    /// Insert a value into the bitmap. No-op if already present.
    ///
    /// Walks the tree to the target leaf. If the path exists, the leaf bit is
    /// OR-ed in place. If the path is missing at some level, the ancestor node
    /// byte is patched and a new path (at most `levels` bytes) is spliced in.
    ///
    /// Panics if `value >= universe_size`.
    pub fn insert(&mut self, value: u32) {
        assert!(
            value < self.universe_size,
            "value {value} out of bounds for universe_size {}",
            self.universe_size
        );

        if self.data.is_empty() {
            // Build the full root-to-leaf path.
            self.data.reserve(self.levels as usize);
            for dl in (0..self.levels).rev() {
                self.data.push(1u8 << child_index(dl, value));
            }
            return;
        }

        let mut pos = 0;
        for dl in (0..self.levels).rev() {
            let child = child_index(dl, value);

            if dl == 0 {
                // Leaf: set the bit (idempotent).
                self.data[pos] |= 1u8 << child;
                return;
            }

            let node = self.data[pos];
            if node & (1u8 << child) != 0 {
                // Child exists — skip past the node byte and preceding siblings.
                pos += 1;
                for sibling in 0..child {
                    if node & (1u8 << sibling) != 0 {
                        pos = skip_subtree_at(&self.data, pos, dl);
                    }
                }
            } else {
                // Child missing — set the bit and splice in a new path.
                self.data[pos] |= 1u8 << child;
                pos += 1;
                for sibling in 0..child {
                    if node & (1u8 << sibling) != 0 {
                        pos = skip_subtree_at(&self.data, pos, dl);
                    }
                }
                // Build the remaining path: inner nodes at dl-1 .. 1, leaf at 0.
                let path_len = dl as usize;
                let mut new_path = [0u8; 11];
                for (i, l) in (0..dl).rev().enumerate() {
                    new_path[i] = 1u8 << child_index(l, value);
                }
                self.data
                    .splice(pos..pos, new_path[..path_len].iter().copied());
                return;
            }
        }
    }

    /// Remove a value from the bitmap. No-op if not present.
    ///
    /// Walks the tree to the target leaf. If the value is present, its bit is
    /// cleared. If the leaf becomes zero, the empty node chain is drained and
    /// ancestor child bits are cleared, cascading upward until an ancestor with
    /// remaining children is found.
    ///
    /// Panics if `value >= universe_size`.
    pub fn remove(&mut self, value: u32) {
        assert!(
            value < self.universe_size,
            "value {value} out of bounds for universe_size {}",
            self.universe_size
        );

        if self.data.is_empty() {
            return;
        }

        let depth = self.levels as usize;
        let mut positions = [0usize; 11];
        let mut children = [0u8; 11];

        // Walk root → leaf, recording position and child index at each depth.
        let mut pos = 0;
        for d in 0..depth {
            let dl = self.levels - 1 - d as u32;
            let child = child_index(dl, value);
            let node = self.data[pos];

            positions[d] = pos;
            children[d] = child;

            if node & (1u8 << child) == 0 {
                // Value not present.
                return;
            }

            if dl == 0 {
                break;
            }

            // Advance past node byte and preceding siblings.
            pos += 1;
            for sibling in 0..child {
                if node & (1u8 << sibling) != 0 {
                    pos = skip_subtree_at(&self.data, pos, dl);
                }
            }
        }

        // Clear the bit in the leaf.
        let leaf_depth = depth - 1;
        let leaf_pos = positions[leaf_depth];
        self.data[leaf_pos] &= !(1u8 << children[leaf_depth]);

        if self.data[leaf_pos] != 0 {
            return;
        }

        // Leaf is empty — cascade upward, clearing child bits and finding
        // the highest ancestor that becomes empty. The chain of empty nodes
        // is always contiguous in the serialized data.
        let mut remove_start = leaf_pos;
        for d in (0..leaf_depth).rev() {
            let parent_pos = positions[d];
            self.data[parent_pos] &= !(1u8 << children[d]);
            if self.data[parent_pos] != 0 {
                break;
            }
            remove_start = parent_pos;
        }

        self.data.drain(remove_start..leaf_pos + 1);
    }

    /// Remove all values in the given range from the bitmap.
    ///
    /// Walks the tree once, copying subtrees outside the range and pruning
    /// subtrees inside it. Partial-overlap nodes are recursed into, and leaf
    /// bytes are masked. Cost is proportional to the tree size, not the
    /// universe size.
    pub fn remove_range(&mut self, range: impl std::ops::RangeBounds<u32>) {
        use std::ops::Bound;

        if self.data.is_empty() {
            return;
        }

        let start = match range.start_bound() {
            Bound::Included(&n) => n as u64,
            Bound::Excluded(&n) => n as u64 + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&n) => n as u64 + 1,
            Bound::Excluded(&n) => n as u64,
            Bound::Unbounded => self.universe_size as u64,
        };

        if start >= end {
            return;
        }

        let mut out = Vec::with_capacity(self.data.len());
        remove_range_subtree(&mut self.nodes(), self.levels, 0, start, end, &mut out);
        self.data = out;
    }

    /// Remove all values from the bitmap.
    pub fn clear(&mut self) {
        self.data.clear();
    }
}

/// Iterator over set bits of a `RawBitmap`.
///
/// Uses a stack-based DFS traversal over the compressed tree, avoiding
/// materialization of a full bitvec. Cost is proportional to the number
/// of tree nodes visited, not the universe size.
pub struct Iter<'a> {
    data: &'a [u8],
    pos: usize,
    stack: [IterFrame; 10],
    stack_len: usize,
    /// Remaining set bits in the current leaf byte.
    leaf_bits: u8,
    /// Base value for the current leaf (value = leaf_base + bit index).
    leaf_base: u32,
}

#[derive(Default, Clone, Copy)]
struct IterFrame {
    node_bits: u8,
    next_child: u8,
    base: u32,
    child_level: u32,
}

impl<'a> Iter<'a> {
    fn empty() -> Self {
        Self {
            data: &[],
            pos: 0,
            stack: [IterFrame::default(); 10],
            stack_len: 0,
            leaf_bits: 0,
            leaf_base: 0,
        }
    }

    /// Advance the DFS to the next leaf byte. Returns `true` if a leaf was found.
    fn advance_to_next_leaf(&mut self) -> bool {
        while self.stack_len > 0 {
            let frame = &mut self.stack[self.stack_len - 1];

            // Find next set child starting from next_child.
            if frame.next_child >= 8 {
                self.stack_len -= 1;
                continue;
            }
            let remaining = frame.node_bits >> frame.next_child;
            if remaining == 0 {
                self.stack_len -= 1;
                continue;
            }

            let skip = remaining.trailing_zeros() as u8;
            let child = frame.next_child + skip;
            frame.next_child = child + 1;

            let new_base = frame.base + child_offset(frame.child_level, child);
            let child_level = frame.child_level;

            if child_level == 1 {
                // Child is a leaf byte.
                self.leaf_bits = self.data[self.pos];
                self.pos += 1;
                self.leaf_base = new_base;
                return true;
            } else {
                // Child is an inner node — push and keep descending.
                let node_byte = self.data[self.pos];
                self.pos += 1;
                self.stack[self.stack_len] = IterFrame {
                    node_bits: node_byte,
                    next_child: 0,
                    base: new_base,
                    child_level: child_level - 1,
                };
                self.stack_len += 1;
            }
        }
        false
    }
}

impl Iterator for Iter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        loop {
            if self.leaf_bits != 0 {
                let bit = self.leaf_bits.trailing_zeros();
                self.leaf_bits &= self.leaf_bits - 1; // clear lowest set bit
                return Some(self.leaf_base + bit);
            }
            if !self.advance_to_next_leaf() {
                return None;
            }
        }
    }
}

impl<'a> IntoIterator for &'a RawBitmap {
    type Item = u32;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

impl BitOr for &RawBitmap {
    type Output = RawBitmap;
    fn bitor(self, rhs: Self) -> RawBitmap {
        assert_eq!(
            self.universe_size, rhs.universe_size,
            "universe_size mismatch: {} vs {}",
            self.universe_size, rhs.universe_size
        );

        if self.data.is_empty() {
            return rhs.clone();
        }
        if rhs.data.is_empty() {
            return self.clone();
        }

        let mut out = Vec::with_capacity(self.data.len().max(rhs.data.len()));
        let mut ra = self.nodes();
        let mut rb = rhs.nodes();
        union_subtree(&mut ra, &mut rb, self.levels, &mut out);

        RawBitmap {
            universe_size: self.universe_size,
            levels: self.levels,
            data: out,
        }
    }
}

impl BitAnd for &RawBitmap {
    type Output = RawBitmap;
    fn bitand(self, rhs: Self) -> RawBitmap {
        assert_eq!(
            self.universe_size, rhs.universe_size,
            "universe_size mismatch: {} vs {}",
            self.universe_size, rhs.universe_size
        );

        if self.data.is_empty() || rhs.data.is_empty() {
            return RawBitmap::empty(self.universe_size);
        }

        let mut out = Vec::new();
        let mut ra = self.nodes();
        let mut rb = rhs.nodes();
        intersect_subtree(&mut ra, &mut rb, self.levels, &mut out);

        RawBitmap {
            universe_size: self.universe_size,
            levels: self.levels,
            data: out,
        }
    }
}

impl Sub for &RawBitmap {
    type Output = RawBitmap;
    fn sub(self, rhs: Self) -> RawBitmap {
        assert_eq!(
            self.universe_size, rhs.universe_size,
            "universe_size mismatch: {} vs {}",
            self.universe_size, rhs.universe_size
        );

        if self.data.is_empty() {
            return RawBitmap::empty(self.universe_size);
        }
        if rhs.data.is_empty() {
            return self.clone();
        }

        let mut out = Vec::with_capacity(self.data.len());
        let mut ra = self.nodes();
        let mut rb = rhs.nodes();
        difference_subtree(&mut ra, &mut rb, self.levels, &mut out);

        RawBitmap {
            universe_size: self.universe_size,
            levels: self.levels,
            data: out,
        }
    }
}

impl BitXor for &RawBitmap {
    type Output = RawBitmap;
    fn bitxor(self, rhs: Self) -> RawBitmap {
        assert_eq!(
            self.universe_size, rhs.universe_size,
            "universe_size mismatch: {} vs {}",
            self.universe_size, rhs.universe_size
        );

        if self.data.is_empty() {
            return rhs.clone();
        }
        if rhs.data.is_empty() {
            return self.clone();
        }

        let mut out = Vec::with_capacity(self.data.len().max(rhs.data.len()));
        let mut ra = self.nodes();
        let mut rb = rhs.nodes();
        symmetric_difference_subtree(&mut ra, &mut rb, self.levels, &mut out);

        RawBitmap {
            universe_size: self.universe_size,
            levels: self.levels,
            data: out,
        }
    }
}

impl BitOrAssign<&RawBitmap> for RawBitmap {
    fn bitor_assign(&mut self, rhs: &RawBitmap) {
        *self = &*self | rhs;
    }
}

impl BitAndAssign<&RawBitmap> for RawBitmap {
    fn bitand_assign(&mut self, rhs: &RawBitmap) {
        *self = &*self & rhs;
    }
}

impl SubAssign<&RawBitmap> for RawBitmap {
    fn sub_assign(&mut self, rhs: &RawBitmap) {
        *self = &*self - rhs;
    }
}

impl BitXorAssign<&RawBitmap> for RawBitmap {
    fn bitxor_assign(&mut self, rhs: &RawBitmap) {
        *self = &*self ^ rhs;
    }
}

/// A bitmap that may store its complement for better compression.
///
/// For sparse bitmaps (few bits set), the set bits are stored directly.
/// For dense bitmaps (most bits set), the *unset* bits are stored instead,
/// and the meaning is inverted. This keeps the underlying `RawBitmap` small
/// at both extremes of the density spectrum.
///
/// AND and OR operations between bitmaps with mixed representations are
/// handled transparently using De Morgan's laws, dispatching to the
/// appropriate `RawBitmap` operation (AND, OR, or SUB).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "allocative", derive(allocative::Allocative))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Bitmap {
    inner: RawBitmap,
    inverted: bool,
}

impl Bitmap {
    /// Create an empty bitmap (no bits set).
    pub fn empty(universe_size: u32) -> Self {
        Self {
            inner: RawBitmap::empty(universe_size),
            inverted: false,
        }
    }

    /// Create a full bitmap (all bits set).
    pub fn full(universe_size: u32) -> Self {
        Self {
            inner: RawBitmap::empty(universe_size),
            inverted: true,
        }
    }

    /// Build from a sorted iterator of **set** values.
    ///
    /// The resulting bitmap stores these values directly (normal representation).
    pub fn from_sorted_iter(iter: impl Iterator<Item = u32>, universe_size: u32) -> Self {
        Self {
            inner: RawBitmap::from_sorted_iter(iter, universe_size),
            inverted: false,
        }
    }

    /// Build from a sorted iterator of **unset** values (the complement).
    ///
    /// The resulting bitmap logically contains every value in `0..universe_size`
    /// that is NOT yielded by `complement_iter`.
    pub fn from_sorted_iter_complemented(
        complement_iter: impl Iterator<Item = u32>,
        universe_size: u32,
    ) -> Self {
        Self {
            inner: RawBitmap::from_sorted_iter(complement_iter, universe_size),
            inverted: true,
        }
    }

    /// The universe size (exclusive upper bound on values).
    pub fn universe_size(&self) -> u32 {
        self.inner.universe_size()
    }

    /// Test whether `value` is in the bitmap.
    pub fn contains(&self, value: u32) -> bool {
        self.inner.contains(value) ^ self.inverted
    }

    /// Count the number of set bits.
    pub fn len(&self) -> u64 {
        if self.inverted {
            self.inner.universe_size() as u64 - self.inner.len()
        } else {
            self.inner.len()
        }
    }

    /// Returns `true` if no bits are set.
    pub fn is_empty(&self) -> bool {
        if self.inverted {
            self.inner.len() == self.inner.universe_size() as u64
        } else {
            self.inner.is_empty()
        }
    }

    /// Whether this bitmap uses inverted (complemented) representation.
    pub fn is_inverted(&self) -> bool {
        self.inverted
    }

    /// Access the underlying raw bitmap.
    pub fn raw(&self) -> &RawBitmap {
        &self.inner
    }

    /// The number of heap-allocated bytes used by this bitmap.
    pub fn heap_bytes(&self) -> usize {
        self.inner.heap_bytes()
    }
}

impl BitAnd for &Bitmap {
    type Output = Bitmap;

    /// Intersection using De Morgan's dispatch:
    ///
    /// - N ∩ N → A & B (normal)
    /// - N ∩ I → A - B (normal)
    /// - I ∩ N → B - A (normal)
    /// - I ∩ I → A | B (inverted)
    fn bitand(self, rhs: Self) -> Bitmap {
        assert_eq!(
            self.inner.universe_size(),
            rhs.inner.universe_size(),
            "universe_size mismatch: {} vs {}",
            self.inner.universe_size(),
            rhs.inner.universe_size()
        );

        let (inner, inverted) = match (self.inverted, rhs.inverted) {
            (false, false) => (&self.inner & &rhs.inner, false),
            (false, true) => (&self.inner - &rhs.inner, false),
            (true, false) => (&rhs.inner - &self.inner, false),
            (true, true) => (&self.inner | &rhs.inner, true),
        };

        Bitmap { inner, inverted }
    }
}

impl BitOr for &Bitmap {
    type Output = Bitmap;

    /// Union using De Morgan's dispatch:
    ///
    /// - N ∪ N → A | B (normal)
    /// - N ∪ I → B - A (inverted)
    /// - I ∪ N → A - B (inverted)
    /// - I ∪ I → A & B (inverted)
    fn bitor(self, rhs: Self) -> Bitmap {
        assert_eq!(
            self.inner.universe_size(),
            rhs.inner.universe_size(),
            "universe_size mismatch: {} vs {}",
            self.inner.universe_size(),
            rhs.inner.universe_size()
        );

        let (inner, inverted) = match (self.inverted, rhs.inverted) {
            (false, false) => (&self.inner | &rhs.inner, false),
            (false, true) => (&rhs.inner - &self.inner, true),
            (true, false) => (&self.inner - &rhs.inner, true),
            (true, true) => (&self.inner & &rhs.inner, true),
        };

        Bitmap { inner, inverted }
    }
}

impl BitAndAssign<&Bitmap> for Bitmap {
    fn bitand_assign(&mut self, rhs: &Bitmap) {
        *self = &*self & rhs;
    }
}

impl BitOrAssign<&Bitmap> for Bitmap {
    fn bitor_assign(&mut self, rhs: &Bitmap) {
        *self = &*self | rhs;
    }
}

/// Extract the 3-bit child index for `value` at the given tree `level`.
#[inline]
fn child_index(level: u32, value: u32) -> u8 {
    ((value >> (3 * level)) & 7) as u8
}

/// A single node byte from a serialized tree8.
///
/// Each bit indicates whether the corresponding child (0..8) is present.
#[derive(Clone, Copy)]
struct Node(u8);

impl Node {
    /// Test whether child `child` is present.
    #[inline]
    fn has_child(self, child: u8) -> bool {
        self.0 & (1u8 << child) != 0
    }

    /// The number of present children.
    #[inline]
    fn count_children(self) -> u32 {
        self.0.count_ones()
    }

    /// The lowest present child index.
    #[inline]
    fn min_child(self) -> u8 {
        self.0.trailing_zeros() as u8
    }

    /// The highest present child index.
    #[inline]
    fn max_child(self) -> u8 {
        7 - self.0.leading_zeros() as u8
    }

    /// The raw bits of this node.
    #[inline]
    fn bits(self) -> u8 {
        self.0
    }
}

/// The value contribution of child index `child` at the given tree `level`.
#[inline]
fn child_offset(level: u32, child: u8) -> u32 {
    (child as u32) << (3 * level)
}

/// Advance `pos` past the subtree rooted at `data[pos]` which is `level` levels
/// tall (1 = single leaf byte). Returns the position immediately after the subtree.
fn skip_subtree_at(data: &[u8], mut pos: usize, level: u32) -> usize {
    let node = data[pos];
    pos += 1;
    let level = level - 1;
    if level == 0 {
        return pos;
    }
    for child in 0..8u8 {
        if node & (1 << child) != 0 {
            pos = skip_subtree_at(data, pos, level);
        }
    }
    pos
}

/// Cursor for reading nodes sequentially from a serialized tree8.
struct NodeReader<'a> {
    nodes: &'a [u8],
    pos: usize,
}

impl NodeReader<'_> {
    /// Read the next node and advance the cursor.
    #[inline]
    fn next(&mut self) -> Node {
        let node = Node(self.nodes[self.pos]);
        self.pos += 1;
        node
    }

    /// Find the minimum value in the subtree rooted at `level`.
    fn min_value(&mut self, level: u32) -> u32 {
        let node = self.next();
        let level = level - 1;

        let min_child = node.min_child();

        if level == 0 {
            return min_child as u32;
        }

        let child_value = self.min_value(level);
        child_offset(level, min_child) + child_value
    }

    /// Find the maximum value in the subtree rooted at `level`.
    fn max_value(&mut self, level: u32) -> u32 {
        let node = self.next();
        let level = level - 1;

        let max_child = node.max_child();

        if level == 0 {
            return max_child as u32;
        }

        // Skip all children before the highest set child.
        for child in 0..max_child {
            if node.has_child(child) {
                self.skip_subtree(level);
            }
        }

        let child_value = self.max_value(level);
        child_offset(level, max_child) + child_value
    }

    /// Skip over a subtree rooted at `level`, returning the number of values it contains.
    ///
    /// Matches `skip_hits` from `src/fid.c:280-293`.
    fn skip_subtree(&mut self, level: u32) -> u64 {
        let node = self.next();
        let level = level - 1;

        if level == 0 {
            return node.count_children() as u64;
        }

        let mut count = 0u64;
        for child in 0..8u8 {
            if node.has_child(child) {
                count += self.skip_subtree(level);
            }
        }
        count
    }
}

/// Recursive contains check. Matches `is_hit_1` from `src/fid.c:260-278`.
fn contains_inner(nodes: &mut NodeReader, level: u32, value: u32) -> bool {
    let level = level - 1;
    let child = child_index(level, value);
    let node = nodes.next();

    if !node.has_child(child) {
        return false;
    }
    if level == 0 {
        return true;
    }

    // Skip preceding sibling subtrees.
    for sibling in 0..child {
        if node.has_child(sibling) {
            nodes.skip_subtree(level);
        }
    }
    contains_inner(nodes, level, value)
}

/// Count set bits in the subtree at `level` that fall within `[start, end)`.
///
/// `base` is the value offset of this subtree's root (0 for the tree root).
/// Uses u64 arithmetic to avoid overflow at the highest tree levels.
fn range_count(reader: &mut NodeReader, level: u32, base: u64, start: u64, end: u64) -> u64 {
    let node = reader.next();
    let child_level = level - 1;

    if child_level == 0 {
        // Leaf: count only the bits within [start, end).
        let lo = start.saturating_sub(base).min(8) as u8;
        let hi = end.saturating_sub(base).min(8) as u8;
        let mask = ((1u16 << hi) - (1u16 << lo)) as u8;
        return (node.bits() & mask).count_ones() as u64;
    }

    let stride: u64 = 1u64 << (3 * child_level);
    let mut count = 0u64;

    for child in 0..8u8 {
        if !node.has_child(child) {
            continue;
        }

        let child_base = base + (child as u64) * stride;
        let child_end = child_base + stride;

        if child_end <= start || child_base >= end {
            // Fully outside: skip without counting.
            reader.skip_subtree(child_level);
        } else if child_base >= start && child_end <= end {
            // Fully inside: count everything in the subtree.
            count += reader.skip_subtree(child_level);
        } else {
            // Partial overlap: recurse.
            count += range_count(reader, child_level, child_base, start, end);
        }
    }

    count
}

/// Remove values in `[start, end)` from the subtree at `level`, writing the
/// surviving nodes to `out`. Returns `true` if the subtree is non-empty after
/// removal.
///
/// Same structure as the set-operation walkers: children fully outside the
/// removal range are copied, children fully inside are skipped (dropped), and
/// children with partial overlap are recursed into.
fn remove_range_subtree(
    reader: &mut NodeReader,
    level: u32,
    base: u64,
    start: u64,
    end: u64,
    out: &mut Vec<u8>,
) -> bool {
    let node = reader.next();
    let child_level = level - 1;

    if child_level == 0 {
        // Leaf: mask out bits within [start, end).
        let lo = start.saturating_sub(base).min(8) as u8;
        let hi = end.saturating_sub(base).min(8) as u8;
        let mask = ((1u16 << hi) - (1u16 << lo)) as u8;
        let result = node.bits() & !mask;
        if result != 0 {
            out.push(result);
            return true;
        }
        return false;
    }

    let stride: u64 = 1u64 << (3 * child_level);
    let node_pos = out.len();
    out.push(0);

    let mut result_bits: u8 = 0;

    for child in 0..8u8 {
        if !node.has_child(child) {
            continue;
        }

        let child_base = base + (child as u64) * stride;
        let child_end = child_base + stride;

        if child_end <= start || child_base >= end {
            // Fully outside removal range: keep.
            copy_subtree(reader, child_level, out);
            result_bits |= 1 << child;
        } else if child_base >= start && child_end <= end {
            // Fully inside removal range: drop.
            reader.skip_subtree(child_level);
        } else {
            // Partial overlap: recurse.
            if remove_range_subtree(reader, child_level, child_base, start, end, out) {
                result_bits |= 1 << child;
            }
        }
    }

    if result_bits != 0 {
        out[node_pos] = result_bits;
        true
    } else {
        out.pop();
        false
    }
}

/// Copy a subtree rooted at `level` from the reader to `out` verbatim.
///
/// Advances the reader past the subtree by calling `skip_subtree`, then copies
/// the raw bytes that were traversed.
fn copy_subtree(reader: &mut NodeReader, level: u32, out: &mut Vec<u8>) {
    let start = reader.pos;
    reader.skip_subtree(level);
    out.extend_from_slice(&reader.nodes[start..reader.pos]);
}

// The four set-operation walkers below share identical structure and differ
// in exactly three parameters:
//
//   Operation  | Leaf op   | A-only child | B-only child
//   -----------+-----------+--------------+-------------
//   OR  (union)| a | b     | copy         | copy
//   AND (inter)| a & b     | skip         | skip
//   SUB (diff) | a & !b    | copy         | skip
//   XOR (symd) | a ^ b     | copy         | copy
//
// They could be unified into a single generic walker parameterized by these
// three values, but are kept separate for readability.

/// Recursively compute the union of two subtrees at the given `level`.
///
/// Walks both trees in lockstep. Children present in both are recursed into
/// (OR-ing leaf bytes). Children unique to one side are bulk-copied. The result
/// is always non-empty when at least one input subtree is non-empty.
fn union_subtree(a: &mut NodeReader, b: &mut NodeReader, level: u32, out: &mut Vec<u8>) {
    let node_a = a.next();
    let node_b = b.next();
    let child_level = level - 1;

    if child_level == 0 {
        out.push(node_a.bits() | node_b.bits());
        return;
    }

    out.push(node_a.bits() | node_b.bits());

    for child in 0..8u8 {
        let in_a = node_a.has_child(child);
        let in_b = node_b.has_child(child);

        match (in_a, in_b) {
            (true, true) => {
                union_subtree(a, b, child_level, out);
            }
            (true, false) => {
                copy_subtree(a, child_level, out);
            }
            (false, true) => {
                copy_subtree(b, child_level, out);
            }
            (false, false) => {}
        }
    }
}

/// Recursively intersect two subtrees at the given `level`.
///
/// Walks both trees in lockstep, only descending into children present in both.
/// Subtrees unique to one side are skipped without expansion. Returns `true`
/// if the intersection produced any output.
fn intersect_subtree(
    a: &mut NodeReader,
    b: &mut NodeReader,
    level: u32,
    out: &mut Vec<u8>,
) -> bool {
    let node_a = a.next();
    let node_b = b.next();
    let child_level = level - 1;

    if child_level == 0 {
        // Leaf: AND the two bytes directly.
        let result = node_a.bits() & node_b.bits();
        if result != 0 {
            out.push(result);
            return true;
        }
        return false;
    }

    // Inner node: reserve a slot for the result node byte.
    let node_pos = out.len();
    out.push(0);

    let mut result_bits: u8 = 0;

    for child in 0..8u8 {
        let in_a = node_a.has_child(child);
        let in_b = node_b.has_child(child);

        match (in_a, in_b) {
            (true, true) => {
                if intersect_subtree(a, b, child_level, out) {
                    result_bits |= 1 << child;
                }
            }
            (true, false) => {
                a.skip_subtree(child_level);
            }
            (false, true) => {
                b.skip_subtree(child_level);
            }
            (false, false) => {}
        }
    }

    if result_bits != 0 {
        out[node_pos] = result_bits;
        true
    } else {
        out.pop();
        false
    }
}

/// Recursively compute the set difference (a - b) of two subtrees at `level`.
///
/// Children only in A are copied verbatim. Children only in B are skipped.
/// Children in both are recursed into with leaf op `a & !b`. Returns `true`
/// if the result is non-empty (needs pruning like intersect).
fn difference_subtree(
    a: &mut NodeReader,
    b: &mut NodeReader,
    level: u32,
    out: &mut Vec<u8>,
) -> bool {
    let node_a = a.next();
    let node_b = b.next();
    let child_level = level - 1;

    if child_level == 0 {
        let result = node_a.bits() & !node_b.bits();
        if result != 0 {
            out.push(result);
            return true;
        }
        return false;
    }

    let node_pos = out.len();
    out.push(0);

    let mut result_bits: u8 = 0;

    for child in 0..8u8 {
        let in_a = node_a.has_child(child);
        let in_b = node_b.has_child(child);

        match (in_a, in_b) {
            (true, true) => {
                if difference_subtree(a, b, child_level, out) {
                    result_bits |= 1 << child;
                }
            }
            (true, false) => {
                copy_subtree(a, child_level, out);
                result_bits |= 1 << child;
            }
            (false, true) => {
                b.skip_subtree(child_level);
            }
            (false, false) => {}
        }
    }

    if result_bits != 0 {
        out[node_pos] = result_bits;
        true
    } else {
        out.pop();
        false
    }
}

/// Recursively compute the symmetric difference (a ^ b) of two subtrees at `level`.
///
/// Children unique to one side are copied verbatim. Children in both are
/// recursed into with leaf op `a ^ b`. Returns `true` if the result is
/// non-empty (needs pruning since identical leaves XOR to zero).
fn symmetric_difference_subtree(
    a: &mut NodeReader,
    b: &mut NodeReader,
    level: u32,
    out: &mut Vec<u8>,
) -> bool {
    let node_a = a.next();
    let node_b = b.next();
    let child_level = level - 1;

    if child_level == 0 {
        let result = node_a.bits() ^ node_b.bits();
        if result != 0 {
            out.push(result);
            return true;
        }
        return false;
    }

    let node_pos = out.len();
    out.push(0);

    let mut result_bits: u8 = 0;

    for child in 0..8u8 {
        let in_a = node_a.has_child(child);
        let in_b = node_b.has_child(child);

        match (in_a, in_b) {
            (true, true) => {
                if symmetric_difference_subtree(a, b, child_level, out) {
                    result_bits |= 1 << child;
                }
            }
            (true, false) => {
                copy_subtree(a, child_level, out);
                result_bits |= 1 << child;
            }
            (false, true) => {
                copy_subtree(b, child_level, out);
                result_bits |= 1 << child;
            }
            (false, false) => {}
        }
    }

    if result_bits != 0 {
        out[node_pos] = result_bits;
        true
    } else {
        out.pop();
        false
    }
}

// ---- Roaring bitmap interop (feature-gated) ----

#[cfg(feature = "roaring")]
mod roaring_interop {
    use super::*;
    use roaring::RoaringBitmap;

    impl RawBitmap {
        /// Build from a `RoaringBitmap` with an explicit universe size.
        ///
        /// Use this when the universe size is known and may be larger than `max + 1`.
        pub fn from_roaring(rb: &RoaringBitmap, universe_size: u32) -> Self {
            Self::from_sorted_iter(rb.iter(), universe_size)
        }
    }

    impl From<&RoaringBitmap> for RawBitmap {
        /// Build from a `RoaringBitmap`, using `max + 1` as the universe size.
        fn from(rb: &RoaringBitmap) -> Self {
            let universe_size = rb.max().map_or(0, |m| m + 1);
            Self::from_roaring(rb, universe_size)
        }
    }

    impl From<&RawBitmap> for RoaringBitmap {
        fn from(bm: &RawBitmap) -> Self {
            RoaringBitmap::from_sorted_iter(bm.iter()).unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ceil_log8() {
        assert_eq!(ceil_log8(0), 0);
        assert_eq!(ceil_log8(1), 1);
        assert_eq!(ceil_log8(8), 1);
        assert_eq!(ceil_log8(9), 2);
        assert_eq!(ceil_log8(64), 2);
        assert_eq!(ceil_log8(65), 3);
        assert_eq!(ceil_log8(512), 3);
        assert_eq!(ceil_log8(513), 4);
    }

    #[test]
    fn test_empty_bitmap() {
        let bm = RawBitmap::empty(0);
        assert_eq!(bm.universe_size(), 0);
        assert_eq!(bm.levels(), 0);
        assert!(bm.data().is_empty());

        let bm = RawBitmap::empty(1);
        assert_eq!(bm.universe_size(), 1);
        assert_eq!(bm.levels(), 1);
        assert!(bm.data().is_empty());

        let bm = RawBitmap::empty(64);
        assert_eq!(bm.universe_size(), 64);
        assert_eq!(bm.levels(), 2);
        assert!(bm.data().is_empty());

        let bm = RawBitmap::empty(512);
        assert_eq!(bm.universe_size(), 512);
        assert_eq!(bm.levels(), 3);
        assert!(bm.data().is_empty());
    }

    #[test]
    fn test_build_universe8_bit0() {
        // universe=8, 1 level. Value 0 → tree8 [0x01]
        let bm = RawBitmap::from_sorted_iter([0].into_iter(), 8);
        assert_eq!(bm.data(), &[0x01]);
    }

    #[test]
    fn test_build_universe8_bit7() {
        let bm = RawBitmap::from_sorted_iter([7].into_iter(), 8);
        assert_eq!(bm.data(), &[0x80]);
    }

    #[test]
    fn test_build_universe8_all() {
        let bm = RawBitmap::from_sorted_iter(0..8, 8);
        assert_eq!(bm.data(), &[0xFF]);
    }

    #[test]
    fn test_build_universe16_insert_0_8() {
        // universe=16, 2 levels.
        // Serialize: root=0x03, child0=0x01, child1=0x01
        let bm = RawBitmap::from_sorted_iter([0, 8].into_iter(), 16);
        assert_eq!(bm.data(), &[0x03, 0x01, 0x01]);
    }

    #[test]
    fn test_build_universe64_insert_63() {
        // universe=64, 2 levels.
        // Serialize: root=0x80, leaf=0x80
        let bm = RawBitmap::from_sorted_iter([63].into_iter(), 64);
        assert_eq!(bm.data(), &[0x80, 0x80]);
    }

    #[test]
    fn test_build_universe9_insert_8() {
        // universe=9, 2 levels. Value 8 → root=0x02, leaf=0x01
        let bm = RawBitmap::from_sorted_iter([8].into_iter(), 9);
        assert_eq!(bm.levels(), 2);
        assert_eq!(bm.data(), &[0x02, 0x01]);
    }

    #[test]
    fn test_contains_universe8() {
        let bm = make_bitmap(8, &[0, 3, 7]);
        for i in 0..8 {
            assert_eq!(bm.contains(i), i == 0 || i == 3 || i == 7, "value={i}");
        }
    }

    #[test]
    fn test_contains_universe16() {
        let bm = make_bitmap(16, &[0, 8]);
        for i in 0..16 {
            assert_eq!(bm.contains(i), i == 0 || i == 8, "value={i}");
        }
    }

    #[test]
    fn test_contains_universe64_sparse() {
        let bm = make_bitmap(64, &[0, 31, 63]);
        for i in 0..64 {
            assert_eq!(bm.contains(i), i == 0 || i == 31 || i == 63, "value={i}");
        }
    }

    #[test]
    fn test_contains_universe64_dense() {
        let values: Vec<u32> = (0..64).collect();
        let bm = make_bitmap(64, &values);
        for i in 0..64 {
            assert!(bm.contains(i), "value={i}");
        }
        assert!(!bm.contains(64));
    }

    #[test]
    fn test_contains_empty() {
        let bm = RawBitmap::empty(64);
        for i in 0..64 {
            assert!(!bm.contains(i));
        }
    }

    #[test]
    fn test_contains_universe9_insert_8() {
        let bm = make_bitmap(9, &[8]);
        for i in 0..9 {
            assert_eq!(bm.contains(i), i == 8, "value={i}");
        }
    }

    #[test]
    fn test_contains_universe512() {
        let values = [0, 1, 7, 8, 63, 64, 255, 256, 511];
        let bm = make_bitmap(512, &values);
        assert_eq!(bm.levels(), 3);
        for i in 0..512 {
            assert_eq!(bm.contains(i), values.contains(&i), "value={i}");
        }
    }

    #[test]
    fn test_iter_ascending_order() {
        let bm = make_bitmap(512, &[511, 0, 255, 1, 63, 64, 8, 7, 256]);
        let result: Vec<u32> = bm.iter().collect();
        assert_eq!(result, vec![0, 1, 7, 8, 63, 64, 255, 256, 511]);
    }

    #[test]
    fn test_iter_empty() {
        let bm = RawBitmap::empty(64);
        let result: Vec<u32> = bm.iter().collect();
        assert!(result.is_empty());
    }

    #[test]
    fn test_iter_single_element() {
        let bm = make_bitmap(64, &[42]);
        let result: Vec<u32> = bm.iter().collect();
        assert_eq!(result, vec![42]);
    }

    #[test]
    fn test_iter_full() {
        let bm = make_bitmap(8, &[0, 1, 2, 3, 4, 5, 6, 7]);
        let result: Vec<u32> = bm.iter().collect();
        assert_eq!(result, vec![0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_len() {
        let bm = RawBitmap::empty(64);
        assert_eq!(bm.len(), 0);

        let bm = make_bitmap(512, &[0, 1, 7, 8, 63, 64, 255, 256, 511]);
        assert_eq!(bm.len(), 9);
    }

    #[test]
    fn test_is_empty() {
        assert!(RawBitmap::empty(64).is_empty());
        assert!(!make_bitmap(8, &[0]).is_empty());
    }

    #[test]
    fn test_min_max() {
        let bm = RawBitmap::empty(64);
        assert_eq!(bm.min(), None);
        assert_eq!(bm.max(), None);

        let bm = make_bitmap(512, &[42]);
        assert_eq!(bm.min(), Some(42));
        assert_eq!(bm.max(), Some(42));

        let bm = make_bitmap(512, &[0, 255, 511]);
        assert_eq!(bm.min(), Some(0));
        assert_eq!(bm.max(), Some(511));
    }

    #[test]
    fn test_min_max_universe8() {
        let bm = make_bitmap(8, &[3, 5]);
        assert_eq!(bm.min(), Some(3));
        assert_eq!(bm.max(), Some(5));
    }

    fn make_bitmap(universe_size: u32, values: &[u32]) -> RawBitmap {
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        RawBitmap::from_sorted_iter(sorted.into_iter(), universe_size)
    }

    #[test]
    fn test_bitor_disjoint() {
        let a = make_bitmap(64, &[0, 1, 2]);
        let b = make_bitmap(64, &[60, 61, 62]);
        let c = &a | &b;
        let result: Vec<u32> = c.iter().collect();
        assert_eq!(result, vec![0, 1, 2, 60, 61, 62]);
    }

    #[test]
    fn test_bitor_overlapping() {
        let a = make_bitmap(64, &[0, 1, 2, 3]);
        let b = make_bitmap(64, &[2, 3, 4, 5]);
        let c = &a | &b;
        let result: Vec<u32> = c.iter().collect();
        assert_eq!(result, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_bitor_empty() {
        let a = make_bitmap(64, &[1, 2, 3]);
        let b = RawBitmap::empty(64);
        let c = &a | &b;
        assert_eq!(c, a);
    }

    #[test]
    fn test_bitor_single_level() {
        let a = make_bitmap(8, &[0, 2, 4]);
        let b = make_bitmap(8, &[1, 3, 5]);
        let c = &a | &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_bitor_single_level_overlapping() {
        let a = make_bitmap(8, &[0, 1, 2]);
        let b = make_bitmap(8, &[1, 2, 3]);
        let c = &a | &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_bitor_one_empty_both_directions() {
        let a = make_bitmap(512, &[0, 100, 511]);
        let b = RawBitmap::empty(512);
        assert_eq!(&a | &b, a);
        assert_eq!(&b | &a, a);
    }

    #[test]
    fn test_bitor_both_empty() {
        let a = RawBitmap::empty(64);
        let b = RawBitmap::empty(64);
        let c = &a | &b;
        assert!(c.is_empty());
    }

    #[test]
    fn test_bitor_with_self() {
        let a = make_bitmap(512, &[0, 7, 42, 100, 255, 511]);
        let c = &a | &a;
        assert_eq!(c, a);
    }

    #[test]
    fn test_bitor_3level_disjoint_octants() {
        // Values in completely different top-level children — exercises copy_subtree
        let a = make_bitmap(512, &[0, 1, 2]);
        let b = make_bitmap(512, &[500, 510, 511]);
        let c = &a | &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1, 2, 500, 510, 511]);
    }

    #[test]
    fn test_bitor_3level_partial_overlap() {
        let a = make_bitmap(512, &[0, 8, 64, 256]);
        let b = make_bitmap(512, &[0, 9, 65, 300]);
        let c = &a | &b;
        let mut expected = vec![0, 8, 9, 64, 65, 256, 300];
        expected.sort();
        assert_eq!(c.iter().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn test_bitor_4level() {
        let a = make_bitmap(4096, &[0, 511, 1000, 2000, 4095]);
        let b = make_bitmap(4096, &[0, 512, 1000, 3000, 4095]);
        let c = &a | &b;
        let mut expected = vec![0, 511, 512, 1000, 2000, 3000, 4095];
        expected.sort();
        expected.dedup();
        assert_eq!(c.iter().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn test_bitor_result_valid_contains() {
        let a = make_bitmap(512, &[0, 1, 7, 8, 63, 64, 255, 256, 511]);
        let b = make_bitmap(512, &[1, 8, 32, 64, 128, 255, 400, 511]);
        let c = &a | &b;
        for v in c.iter() {
            assert!(
                a.contains(v) || b.contains(v),
                "result {v} not in either operand"
            );
        }
        for v in 0..512 {
            if a.contains(v) || b.contains(v) {
                assert!(c.contains(v), "value {v} missing from union");
            }
        }
    }

    #[test]
    fn test_bitand_overlapping() {
        let a = make_bitmap(64, &[0, 1, 2, 3]);
        let b = make_bitmap(64, &[2, 3, 4, 5]);
        let c = &a & &b;
        let result: Vec<u32> = c.iter().collect();
        assert_eq!(result, vec![2, 3]);
    }

    #[test]
    fn test_bitand_disjoint() {
        let a = make_bitmap(64, &[0, 1]);
        let b = make_bitmap(64, &[2, 3]);
        let c = &a & &b;
        assert!(c.is_empty());
    }

    #[test]
    fn test_bitand_single_level() {
        // universe=8 → 1 level, leaf-only AND
        let a = make_bitmap(8, &[0, 1, 3, 5, 7]);
        let b = make_bitmap(8, &[1, 2, 3, 6, 7]);
        let c = &a & &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![1, 3, 7]);
    }

    #[test]
    fn test_bitand_single_level_disjoint() {
        let a = make_bitmap(8, &[0, 2, 4]);
        let b = make_bitmap(8, &[1, 3, 5]);
        let c = &a & &b;
        assert!(c.is_empty());
    }

    #[test]
    fn test_bitand_one_empty() {
        let a = make_bitmap(512, &[0, 100, 255, 511]);
        let b = RawBitmap::empty(512);
        assert!((&a & &b).is_empty());
        assert!((&b & &a).is_empty());
    }

    #[test]
    fn test_bitand_both_empty() {
        let a = RawBitmap::empty(64);
        let b = RawBitmap::empty(64);
        assert!((&a & &b).is_empty());
    }

    #[test]
    fn test_bitand_with_self() {
        let a = make_bitmap(512, &[0, 7, 42, 100, 255, 511]);
        let c = &a & &a;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 7, 42, 100, 255, 511]);
        assert_eq!(c, a);
    }

    #[test]
    fn test_bitand_3level_sparse() {
        // 3-level tree (universe=512), values in completely different octants
        let a = make_bitmap(512, &[0, 1, 2]); // all in first leaf group
        let b = make_bitmap(512, &[500, 510, 511]); // all in last leaf group
        let c = &a & &b;
        assert!(c.is_empty());
    }

    #[test]
    fn test_bitand_3level_partial_overlap() {
        // Inner nodes overlap but only some leaves do
        let a = make_bitmap(512, &[0, 8, 64, 256]);
        let b = make_bitmap(512, &[0, 9, 65, 300]);
        let c = &a & &b;
        // Only value 0 is in both
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn test_bitand_inner_overlap_leaf_disjoint() {
        // Values share the same inner-node path but different leaf bits
        // In universe=64 (2 levels), values 0..7 share leaf byte 0.
        // a has bit 0, b has bit 7 — same inner child, different leaf bits.
        let a = make_bitmap(64, &[0]);
        let b = make_bitmap(64, &[7]);
        let c = &a & &b;
        assert!(c.is_empty());
    }

    #[test]
    fn test_bitand_4level() {
        // 4-level tree: universe=4096
        let a = make_bitmap(4096, &[0, 511, 1000, 2000, 4095]);
        let b = make_bitmap(4096, &[0, 512, 1000, 3000, 4095]);
        let c = &a & &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1000, 4095]);
    }

    #[test]
    fn test_bitand_dense() {
        // Both sides fully dense in a single-level tree
        let a = make_bitmap(8, &[0, 1, 2, 3, 4, 5, 6, 7]);
        let b = make_bitmap(8, &[0, 1, 2, 3, 4, 5, 6, 7]);
        let c = &a & &b;
        assert_eq!(c.len(), 8);
    }

    #[test]
    fn test_bitand_subset() {
        // a is a subset of b — AND should equal a
        let a = make_bitmap(512, &[10, 20, 30]);
        let b = make_bitmap(512, &[10, 15, 20, 25, 30, 35]);
        let c = &a & &b;
        assert_eq!(c, a);
    }

    #[test]
    fn test_bitand_result_valid_contains() {
        // Verify every value in the result is in both operands,
        // and every value in both operands is in the result.
        let a = make_bitmap(512, &[0, 1, 7, 8, 63, 64, 255, 256, 511]);
        let b = make_bitmap(512, &[1, 8, 32, 64, 128, 255, 400, 511]);
        let c = &a & &b;
        for v in c.iter() {
            assert!(a.contains(v), "result {v} not in a");
            assert!(b.contains(v), "result {v} not in b");
        }
        for v in 0..512 {
            if a.contains(v) && b.contains(v) {
                assert!(c.contains(v), "common value {v} missing from result");
            }
        }
    }

    #[test]
    fn test_sub() {
        let a = make_bitmap(64, &[0, 1, 2, 3]);
        let b = make_bitmap(64, &[2, 3, 4, 5]);
        let c = &a - &b;
        let result: Vec<u32> = c.iter().collect();
        assert_eq!(result, vec![0, 1]);
    }

    #[test]
    fn test_sub_single_level() {
        let a = make_bitmap(8, &[0, 1, 2, 3, 4]);
        let b = make_bitmap(8, &[2, 3, 5]);
        let c = &a - &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1, 4]);
    }

    #[test]
    fn test_sub_one_empty() {
        let a = make_bitmap(512, &[0, 100, 511]);
        let b = RawBitmap::empty(512);
        assert_eq!(&a - &b, a);

        let c = &b - &a;
        assert!(c.is_empty());
    }

    #[test]
    fn test_sub_both_empty() {
        let a = RawBitmap::empty(64);
        let b = RawBitmap::empty(64);
        assert!((&a - &b).is_empty());
    }

    #[test]
    fn test_sub_with_self() {
        let a = make_bitmap(512, &[0, 42, 255, 511]);
        let c = &a - &a;
        assert!(c.is_empty());
    }

    #[test]
    fn test_sub_disjoint() {
        // Nothing to subtract — result equals a.
        let a = make_bitmap(512, &[0, 1, 2]);
        let b = make_bitmap(512, &[500, 510, 511]);
        let c = &a - &b;
        assert_eq!(c, a);
    }

    #[test]
    fn test_sub_3level_partial() {
        let a = make_bitmap(512, &[0, 8, 64, 256]);
        let b = make_bitmap(512, &[0, 9, 65, 300]);
        let c = &a - &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![8, 64, 256]);
    }

    #[test]
    fn test_sub_superset() {
        // b is a superset of a — result is empty.
        let a = make_bitmap(512, &[10, 20, 30]);
        let b = make_bitmap(512, &[10, 15, 20, 25, 30, 35]);
        let c = &a - &b;
        assert!(c.is_empty());
    }

    #[test]
    fn test_sub_4level() {
        let a = make_bitmap(4096, &[0, 511, 1000, 2000, 4095]);
        let b = make_bitmap(4096, &[0, 512, 1000, 3000, 4095]);
        let c = &a - &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![511, 2000]);
    }

    #[test]
    fn test_sub_result_valid() {
        let a = make_bitmap(512, &[0, 1, 7, 8, 63, 64, 255, 256, 511]);
        let b = make_bitmap(512, &[1, 8, 32, 64, 128, 255, 400, 511]);
        let c = &a - &b;
        for v in c.iter() {
            assert!(a.contains(v), "result {v} not in a");
            assert!(!b.contains(v), "result {v} should not be in b");
        }
        for v in 0..512 {
            if a.contains(v) && !b.contains(v) {
                assert!(c.contains(v), "value {v} missing from difference");
            }
        }
    }

    #[test]
    fn test_bitxor() {
        let a = make_bitmap(64, &[0, 1, 2, 3]);
        let b = make_bitmap(64, &[2, 3, 4, 5]);
        let c = &a ^ &b;
        let result: Vec<u32> = c.iter().collect();
        assert_eq!(result, vec![0, 1, 4, 5]);
    }

    #[test]
    fn test_bitxor_single_level() {
        let a = make_bitmap(8, &[0, 1, 2, 3]);
        let b = make_bitmap(8, &[2, 3, 4, 5]);
        let c = &a ^ &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1, 4, 5]);
    }

    #[test]
    fn test_bitxor_one_empty() {
        let a = make_bitmap(512, &[0, 100, 511]);
        let b = RawBitmap::empty(512);
        assert_eq!(&a ^ &b, a);
        assert_eq!(&b ^ &a, a);
    }

    #[test]
    fn test_bitxor_both_empty() {
        let a = RawBitmap::empty(64);
        let b = RawBitmap::empty(64);
        assert!((&a ^ &b).is_empty());
    }

    #[test]
    fn test_bitxor_with_self() {
        let a = make_bitmap(512, &[0, 42, 255, 511]);
        let c = &a ^ &a;
        assert!(c.is_empty());
    }

    #[test]
    fn test_bitxor_disjoint() {
        // Disjoint — XOR equals union.
        let a = make_bitmap(512, &[0, 1, 2]);
        let b = make_bitmap(512, &[500, 510, 511]);
        let c = &a ^ &b;
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0, 1, 2, 500, 510, 511]);
    }

    #[test]
    fn test_bitxor_3level_partial() {
        let a = make_bitmap(512, &[0, 8, 64, 256]);
        let b = make_bitmap(512, &[0, 9, 65, 300]);
        let c = &a ^ &b;
        let mut expected = vec![8, 9, 64, 65, 256, 300];
        expected.sort();
        assert_eq!(c.iter().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn test_bitxor_4level() {
        let a = make_bitmap(4096, &[0, 511, 1000, 2000, 4095]);
        let b = make_bitmap(4096, &[0, 512, 1000, 3000, 4095]);
        let c = &a ^ &b;
        let mut expected = vec![511, 512, 2000, 3000];
        expected.sort();
        assert_eq!(c.iter().collect::<Vec<_>>(), expected);
    }

    #[test]
    fn test_bitxor_result_valid() {
        let a = make_bitmap(512, &[0, 1, 7, 8, 63, 64, 255, 256, 511]);
        let b = make_bitmap(512, &[1, 8, 32, 64, 128, 255, 400, 511]);
        let c = &a ^ &b;
        for v in c.iter() {
            assert!(
                a.contains(v) ^ b.contains(v),
                "result {v} should be in exactly one operand"
            );
        }
        for v in 0..512 {
            if a.contains(v) ^ b.contains(v) {
                assert!(c.contains(v), "value {v} missing from xor");
            }
        }
    }

    #[test]
    fn test_bitor_commutativity() {
        let a = make_bitmap(64, &[0, 10, 20]);
        let b = make_bitmap(64, &[5, 15, 25]);
        assert_eq!(&a | &b, &b | &a);
    }

    #[test]
    fn test_bitand_commutativity() {
        let a = make_bitmap(64, &[0, 10, 20, 30]);
        let b = make_bitmap(64, &[10, 20, 40, 50]);
        assert_eq!(&a & &b, &b & &a);
    }

    #[test]
    fn test_bitxor_commutativity() {
        let a = make_bitmap(64, &[0, 10, 20]);
        let b = make_bitmap(64, &[10, 20, 30]);
        assert_eq!(&a ^ &b, &b ^ &a);
    }

    #[test]
    fn test_intersection_subset_of_union() {
        let a = make_bitmap(512, &[0, 100, 200, 300, 400, 511]);
        let b = make_bitmap(512, &[50, 100, 250, 300, 450, 511]);
        let intersection = &a & &b;
        let union = &a | &b;
        for val in intersection.iter() {
            assert!(union.contains(val));
        }
    }

    #[test]
    fn test_bitor_full() {
        let a = make_bitmap(8, &[0, 1, 2, 3]);
        let b = make_bitmap(8, &[4, 5, 6, 7]);
        let c = &a | &b;
        assert_eq!(c.len(), 8);
    }

    #[test]
    fn test_assign_variants() {
        let mut a = make_bitmap(64, &[0, 1, 2]);
        let b = make_bitmap(64, &[2, 3, 4]);
        a |= &b;
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);

        let mut a = make_bitmap(64, &[0, 1, 2, 3]);
        a &= &b;
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![2, 3]);

        let mut a = make_bitmap(64, &[0, 1, 2, 3]);
        a -= &b;
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![0, 1]);

        let mut a = make_bitmap(64, &[0, 1, 2, 3]);
        a ^= &b;
        assert_eq!(a.iter().collect::<Vec<_>>(), vec![0, 1, 4]);
    }

    #[test]
    #[should_panic(expected = "universe_size mismatch")]
    fn test_set_op_mismatched_universe() {
        let a = make_bitmap(64, &[0]);
        let b = make_bitmap(128, &[0]);
        let _ = &a | &b;
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let bm = make_bitmap(512, &[0, 1, 7, 8, 63, 64, 255, 256, 511]);
        let mut buf = Vec::new();
        bm.serialize_into(&mut buf).unwrap();
        let bm2 = RawBitmap::deserialize_from(&buf[..]).unwrap();
        assert_eq!(bm, bm2);
    }

    #[test]
    fn test_serialize_deserialize_empty() {
        let bm = RawBitmap::empty(64);
        let mut buf = Vec::new();
        bm.serialize_into(&mut buf).unwrap();
        let bm2 = RawBitmap::deserialize_from(&buf[..]).unwrap();
        assert_eq!(bm, bm2);
    }

    #[test]
    fn test_serialized_size_matches() {
        let bm = make_bitmap(512, &[0, 100, 200, 300, 511]);
        let mut buf = Vec::new();
        bm.serialize_into(&mut buf).unwrap();
        assert_eq!(buf.len(), bm.serialized_size());
    }

    #[test]
    fn test_deserialize_truncated() {
        // Only 4 bytes (missing data_len and data).
        let buf = [1u8, 0, 0, 0];
        let result = RawBitmap::deserialize_from(&buf[..]);
        assert!(result.is_err());
    }

    #[test]
    fn test_heap_bytes() {
        let bm = RawBitmap::empty(64);
        assert_eq!(bm.heap_bytes(), 0);

        let bm = make_bitmap(8, &[0]);
        assert_eq!(bm.heap_bytes(), bm.data().len());
    }

    #[test]
    fn test_insert_into_empty() {
        let mut bm = RawBitmap::empty(64);
        assert!(bm.is_empty());
        bm.insert(42);
        assert!(!bm.is_empty());
        assert!(bm.contains(42));
        assert_eq!(bm.len(), 1);
    }

    #[test]
    fn test_insert_duplicate_noop() {
        let mut bm = make_bitmap(64, &[10, 20]);
        let before = bm.clone();
        bm.insert(10);
        assert_eq!(bm, before);
    }

    #[test]
    fn test_remove_from_populated() {
        let mut bm = make_bitmap(64, &[10, 20, 30]);
        bm.remove(20);
        assert!(!bm.contains(20));
        assert!(bm.contains(10));
        assert!(bm.contains(30));
        assert_eq!(bm.len(), 2);
    }

    #[test]
    fn test_remove_absent_noop() {
        let mut bm = make_bitmap(64, &[10, 20]);
        let before = bm.clone();
        bm.remove(30);
        assert_eq!(bm, before);
    }

    #[test]
    fn test_clear() {
        let mut bm = make_bitmap(64, &[10, 20, 30]);
        bm.clear();
        assert!(bm.is_empty());
        assert_eq!(bm.len(), 0);
        for i in 0..64 {
            assert!(!bm.contains(i));
        }
    }

    #[test]
    fn test_mutation_sequence() {
        let mut bm = RawBitmap::empty(512);
        bm.insert(0);
        bm.insert(100);
        bm.insert(511);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0, 100, 511]);

        bm.remove(100);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0, 511]);

        bm.insert(200);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0, 200, 511]);

        bm.clear();
        assert!(bm.is_empty());

        bm.insert(42);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![42]);
    }

    #[test]
    fn test_remove_last_element() {
        let mut bm = make_bitmap(8, &[3]);
        bm.remove(3);
        assert!(bm.is_empty());
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn test_insert_out_of_bounds_mutation() {
        let mut bm = RawBitmap::empty(8);
        bm.insert(8);
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn test_remove_out_of_bounds() {
        let mut bm = RawBitmap::empty(8);
        bm.remove(8);
    }

    #[test]
    fn test_from_sorted_iter_correctness() {
        // Verify from_sorted_iter produces bitmaps with correct contains/iter.
        let cases: Vec<(u32, Vec<u32>)> = vec![
            (8, vec![0]),
            (8, vec![7]),
            (8, vec![0, 1, 2, 3, 4, 5, 6, 7]),
            (16, vec![0, 8]),
            (64, vec![63]),
            (64, vec![0, 31, 63]),
            (512, vec![0, 1, 7, 8, 63, 64, 255, 256, 511]),
            (9, vec![8]),
            (1_000_000, vec![950_000]),
            (1_000_000, vec![0, 500_000, 999_999]),
        ];
        for (universe_size, values) in &cases {
            let bm = RawBitmap::from_sorted_iter(values.iter().copied(), *universe_size);
            let result: Vec<u32> = bm.iter().collect();
            assert_eq!(
                result, *values,
                "universe={universe_size}, values={values:?}"
            );
            for &v in values {
                assert!(bm.contains(v), "universe={universe_size}, missing {v}");
            }
            assert_eq!(bm.len(), values.len() as u64);
        }
    }

    #[test]
    fn test_from_sorted_iter_empty() {
        let bm = RawBitmap::from_sorted_iter(std::iter::empty(), 64);
        assert!(bm.is_empty());
        assert_eq!(bm.universe_size(), 64);

        let bm = RawBitmap::from_sorted_iter(std::iter::empty(), 0);
        assert!(bm.is_empty());
        assert_eq!(bm.levels(), 0);
    }

    #[test]
    fn test_from_sorted_iter_single_level() {
        let bm = RawBitmap::from_sorted_iter([3, 5].into_iter(), 8);
        assert_eq!(bm.data(), &[0x28]); // bits 3 and 5
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![3, 5]);
    }

    #[test]
    fn test_from_sorted_iter_duplicates_tolerated() {
        let bm = RawBitmap::from_sorted_iter([10, 10, 20, 20, 20].into_iter(), 64);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![10, 20]);
    }

    #[test]
    fn test_from_sorted_iter_dense() {
        let values: Vec<u32> = (0..64).collect();
        let bm = RawBitmap::from_sorted_iter(values.iter().copied(), 64);
        assert_eq!(bm.len(), 64);
        for v in 0..64 {
            assert!(bm.contains(v));
        }
    }

    #[test]
    fn test_from_sorted_iter_large_universe() {
        let bm = RawBitmap::from_sorted_iter([0, 500_000, 999_999].into_iter(), 1_000_000);
        assert_eq!(bm.len(), 3);
        assert!(bm.contains(0));
        assert!(bm.contains(500_000));
        assert!(bm.contains(999_999));
        assert!(!bm.contains(1));
    }

    #[test]
    fn test_from_range_full() {
        let bm = RawBitmap::from_range(0..64, 64);
        assert_eq!(bm.len(), 64);
        for v in 0..64 {
            assert!(bm.contains(v));
        }
    }

    #[test]
    fn test_from_range_partial() {
        let bm = RawBitmap::from_range(10..20, 64);
        assert_eq!(bm.len(), 10);
        for v in 0..64 {
            assert_eq!(bm.contains(v), (10..20).contains(&v));
        }
    }

    #[test]
    fn test_from_range_inclusive() {
        let bm = RawBitmap::from_range(5..=10, 64);
        assert_eq!(bm.len(), 6);
        assert!(bm.contains(5));
        assert!(bm.contains(10));
        assert!(!bm.contains(4));
        assert!(!bm.contains(11));
    }

    #[test]
    fn test_from_range_unbounded() {
        let bm = RawBitmap::from_range(.., 64);
        assert_eq!(bm.len(), 64);

        let bm = RawBitmap::from_range(..32, 64);
        assert_eq!(bm.len(), 32);
        assert!(bm.contains(0));
        assert!(!bm.contains(32));

        let bm = RawBitmap::from_range(32.., 64);
        assert_eq!(bm.len(), 32);
        assert!(!bm.contains(31));
        assert!(bm.contains(32));
        assert!(bm.contains(63));
    }

    #[test]
    fn test_from_range_empty() {
        let bm = RawBitmap::from_range(10..10, 64);
        assert!(bm.is_empty());

        let bm = RawBitmap::from_range(20..10, 64);
        assert!(bm.is_empty());
    }

    #[test]
    fn test_from_range_clamped_to_universe() {
        let bm = RawBitmap::from_range(0..1000, 64);
        assert_eq!(bm.len(), 64);

        let bm = RawBitmap::from_range(60..1000, 64);
        assert_eq!(bm.len(), 4);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![60, 61, 62, 63]);
    }

    #[test]
    fn test_from_range_matches_from_sorted_iter() {
        for &universe in &[8, 64, 512, 4096] {
            let from_range = RawBitmap::from_range(0..universe, universe);
            let from_iter = RawBitmap::from_sorted_iter(0..universe, universe);
            assert_eq!(from_range, from_iter, "universe={universe}");

            let from_range = RawBitmap::from_range(universe / 4..universe * 3 / 4, universe);
            let start = universe / 4;
            let end = universe * 3 / 4;
            let from_iter = RawBitmap::from_sorted_iter(start..end, universe);
            assert_eq!(
                from_range, from_iter,
                "universe={universe} range={start}..{end}"
            );
        }
    }

    #[test]
    fn test_range_cardinality_full() {
        let bm = make_bitmap(64, &(0..64).collect::<Vec<_>>());
        assert_eq!(bm.range_cardinality(..), 64);
        assert_eq!(bm.range_cardinality(0..64), 64);
        assert_eq!(bm.range_cardinality(0..=63), 64);
    }

    #[test]
    fn test_range_cardinality_equals_len() {
        let bm = make_bitmap(512, &[0, 7, 8, 63, 64, 255, 256, 511]);
        assert_eq!(bm.range_cardinality(..), bm.len());
        assert_eq!(bm.range_cardinality(0..512), bm.len());
    }

    #[test]
    fn test_range_cardinality_partial() {
        let bm = make_bitmap(64, &[0, 1, 2, 10, 20, 30, 40, 50, 60, 63]);
        assert_eq!(bm.range_cardinality(0..3), 3);
        assert_eq!(bm.range_cardinality(10..11), 1);
        assert_eq!(bm.range_cardinality(3..10), 0);
        assert_eq!(bm.range_cardinality(10..=20), 2);
        assert_eq!(bm.range_cardinality(..10), 3);
        assert_eq!(bm.range_cardinality(60..), 2);
    }

    #[test]
    fn test_range_cardinality_empty_bitmap() {
        let bm = RawBitmap::empty(64);
        assert_eq!(bm.range_cardinality(..), 0);
        assert_eq!(bm.range_cardinality(0..64), 0);
    }

    #[test]
    fn test_range_cardinality_empty_range() {
        let bm = make_bitmap(64, &[0, 1, 2, 3]);
        assert_eq!(bm.range_cardinality(10..10), 0);
        assert_eq!(bm.range_cardinality(10..5), 0);
    }

    #[test]
    fn test_range_cardinality_multi_level() {
        let bm = make_bitmap(4096, &[0, 100, 500, 1000, 2000, 3000, 4095]);
        assert_eq!(bm.range_cardinality(0..101), 2);
        assert_eq!(bm.range_cardinality(100..1001), 3);
        assert_eq!(bm.range_cardinality(1000..4096), 4);
        assert_eq!(bm.range_cardinality(3000..=4095), 2);
    }

    #[test]
    fn test_range_cardinality_single_level() {
        let bm = make_bitmap(8, &[1, 3, 5, 7]);
        assert_eq!(bm.range_cardinality(0..4), 2);
        assert_eq!(bm.range_cardinality(4..8), 2);
        assert_eq!(bm.range_cardinality(0..8), 4);
        assert_eq!(bm.range_cardinality(1..=5), 3);
    }

    #[test]
    fn test_remove_range_all() {
        let mut bm = make_bitmap(64, &(0..64).collect::<Vec<_>>());
        bm.remove_range(..);
        assert!(bm.is_empty());
    }

    #[test]
    fn test_remove_range_prefix() {
        let mut bm = make_bitmap(64, &(0..64).collect::<Vec<_>>());
        bm.remove_range(..32);
        assert_eq!(bm.len(), 32);
        assert!(!bm.contains(31));
        assert!(bm.contains(32));
        assert!(bm.contains(63));
    }

    #[test]
    fn test_remove_range_suffix() {
        let mut bm = make_bitmap(64, &(0..64).collect::<Vec<_>>());
        bm.remove_range(32..);
        assert_eq!(bm.len(), 32);
        assert!(bm.contains(0));
        assert!(bm.contains(31));
        assert!(!bm.contains(32));
    }

    #[test]
    fn test_remove_range_middle() {
        let mut bm = make_bitmap(64, &(0..64).collect::<Vec<_>>());
        bm.remove_range(10..20);
        assert_eq!(bm.len(), 54);
        assert!(bm.contains(9));
        assert!(!bm.contains(10));
        assert!(!bm.contains(19));
        assert!(bm.contains(20));
    }

    #[test]
    fn test_remove_range_empty_range() {
        let vals: Vec<u32> = (0..64).collect();
        let mut bm = make_bitmap(64, &vals);
        let orig = bm.clone();
        bm.remove_range(10..10);
        assert_eq!(bm, orig);
    }

    #[test]
    fn test_remove_range_no_overlap() {
        let mut bm = make_bitmap(64, &[0, 1, 2, 60, 61, 62]);
        let orig = bm.clone();
        bm.remove_range(30..40);
        assert_eq!(bm, orig);
    }

    #[test]
    fn test_remove_range_single_level() {
        let mut bm = make_bitmap(8, &[0, 1, 2, 3, 4, 5, 6, 7]);
        bm.remove_range(2..6);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0, 1, 6, 7]);
    }

    #[test]
    fn test_remove_range_multi_level() {
        let mut bm = make_bitmap(4096, &[0, 100, 500, 1000, 2000, 3000, 4095]);
        bm.remove_range(100..3000);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![0, 3000, 4095]);
    }

    #[test]
    fn test_remove_range_inclusive() {
        let mut bm = make_bitmap(64, &[5, 10, 15, 20, 25]);
        bm.remove_range(10..=20);
        assert_eq!(bm.iter().collect::<Vec<_>>(), vec![5, 25]);
    }

    #[test]
    fn test_remove_range_empty_bitmap() {
        let mut bm = RawBitmap::empty(64);
        bm.remove_range(0..64);
        assert!(bm.is_empty());
    }

    #[test]
    fn test_estimate_data_size_matches_actual() {
        // Verify estimate matches actual data().len() for various cases.
        let cases: Vec<(u32, Vec<u32>)> = vec![
            (8, vec![0]),
            (8, vec![7]),
            (8, vec![0, 1, 2, 3, 4, 5, 6, 7]),
            (16, vec![0, 8]),
            (64, vec![63]),
            (64, vec![0, 31, 63]),
            (512, vec![0, 1, 7, 8, 63, 64, 255, 256, 511]),
            (9, vec![8]),
            (1_000_000, vec![950_000]),
            (1_000_000, vec![0, 500_000, 999_999]),
        ];
        for (universe_size, values) in &cases {
            let bm = make_bitmap(*universe_size, values);
            let estimated = estimate_data_size(*universe_size, values.iter().copied());
            assert_eq!(
                estimated,
                bm.data().len(),
                "universe={universe_size}, values={values:?}"
            );
        }
    }

    #[test]
    fn test_estimate_data_size_empty() {
        assert_eq!(estimate_data_size(64, std::iter::empty()), 0);
        assert_eq!(estimate_data_size(0, std::iter::empty()), 0);
    }

    #[test]
    fn test_estimate_data_size_single_element_is_levels() {
        // A single element always costs exactly `levels` bytes.
        for &universe in &[8, 64, 512, 4096, 1_000_000] {
            let levels = ceil_log8(universe) as usize;
            let size = estimate_data_size(universe, std::iter::once(0));
            assert_eq!(size, levels, "universe={universe}");
        }
    }

    #[test]
    fn test_estimate_data_size_dense() {
        // Dense bitmap: all values set in universe=64.
        let values: Vec<u32> = (0..64).collect();
        let bm = make_bitmap(64, &values);
        let estimated = estimate_data_size(64, values.iter().copied());
        assert_eq!(estimated, bm.data().len());
    }

    // ---- Bitmap (high-level, De Morgan) tests ----

    #[test]
    fn test_bitmap_contains_normal() {
        let bm = Bitmap::from_sorted_iter([10, 20, 30].into_iter(), 64);
        assert!(bm.contains(10));
        assert!(bm.contains(20));
        assert!(bm.contains(30));
        assert!(!bm.contains(0));
        assert!(!bm.contains(15));
        assert!(!bm.is_inverted());
    }

    #[test]
    fn test_bitmap_contains_inverted() {
        let bm = Bitmap::from_sorted_iter_complemented([10, 20, 30].into_iter(), 64);
        assert!(!bm.contains(10));
        assert!(!bm.contains(20));
        assert!(!bm.contains(30));
        assert!(bm.contains(0));
        assert!(bm.contains(15));
        assert!(bm.contains(63));
        assert!(bm.is_inverted());
    }

    #[test]
    fn test_bitmap_len() {
        let bm = Bitmap::from_sorted_iter([10, 20, 30].into_iter(), 64);
        assert_eq!(bm.len(), 3);

        let bm = Bitmap::from_sorted_iter_complemented([10, 20, 30].into_iter(), 64);
        assert_eq!(bm.len(), 61);
    }

    #[test]
    fn test_bitmap_empty_full() {
        let bm = Bitmap::empty(64);
        assert!(bm.is_empty());
        assert_eq!(bm.len(), 0);
        assert!(!bm.contains(0));

        let bm = Bitmap::full(64);
        assert!(!bm.is_empty());
        assert_eq!(bm.len(), 64);
        for i in 0..64 {
            assert!(bm.contains(i));
        }
    }

    #[test]
    fn test_bitmap_and_nn() {
        let a = Bitmap::from_sorted_iter([0, 1, 2, 3].into_iter(), 64);
        let b = Bitmap::from_sorted_iter([2, 3, 4, 5].into_iter(), 64);
        let c = &a & &b;
        assert!(!c.is_inverted());
        assert_eq!(c.len(), 2);
        assert!(c.contains(2));
        assert!(c.contains(3));
        assert!(!c.contains(0));
        assert!(!c.contains(4));
    }

    #[test]
    fn test_bitmap_and_ni() {
        // A = {0,1,2,3}, B stored as complement of {2,3} → B = universe \ {2,3}
        // A ∩ B = {0,1,2,3} ∩ (universe \ {2,3}) = {0,1}
        let a = Bitmap::from_sorted_iter([0, 1, 2, 3].into_iter(), 64);
        let b = Bitmap::from_sorted_iter_complemented([2, 3].into_iter(), 64);
        let c = &a & &b;
        assert!(!c.is_inverted());
        assert_eq!(c.len(), 2);
        assert!(c.contains(0));
        assert!(c.contains(1));
        assert!(!c.contains(2));
        assert!(!c.contains(3));
    }

    #[test]
    fn test_bitmap_and_in() {
        let a = Bitmap::from_sorted_iter_complemented([2, 3].into_iter(), 64);
        let b = Bitmap::from_sorted_iter([0, 1, 2, 3].into_iter(), 64);
        let c = &a & &b;
        assert!(!c.is_inverted());
        assert_eq!(c.len(), 2);
        assert!(c.contains(0));
        assert!(c.contains(1));
        assert!(!c.contains(2));
    }

    #[test]
    fn test_bitmap_and_ii() {
        // A = universe \ {0,1}, B = universe \ {2,3}
        // A ∩ B = universe \ ({0,1} ∪ {2,3}) = universe \ {0,1,2,3}
        let a = Bitmap::from_sorted_iter_complemented([0, 1].into_iter(), 64);
        let b = Bitmap::from_sorted_iter_complemented([2, 3].into_iter(), 64);
        let c = &a & &b;
        assert!(c.is_inverted());
        assert!(!c.contains(0));
        assert!(!c.contains(1));
        assert!(!c.contains(2));
        assert!(!c.contains(3));
        assert!(c.contains(4));
        assert!(c.contains(63));
    }

    #[test]
    fn test_bitmap_or_nn() {
        let a = Bitmap::from_sorted_iter([0, 1].into_iter(), 64);
        let b = Bitmap::from_sorted_iter([2, 3].into_iter(), 64);
        let c = &a | &b;
        assert!(!c.is_inverted());
        assert_eq!(c.len(), 4);
        assert!(c.contains(0));
        assert!(c.contains(3));
        assert!(!c.contains(4));
    }

    #[test]
    fn test_bitmap_or_ni() {
        // A = {10, 20}, B = universe \ {5, 10, 15}
        // A ∪ B = universe \ ({5, 10, 15} \ {10, 20}) = universe \ {5, 15}
        let a = Bitmap::from_sorted_iter([10, 20].into_iter(), 64);
        let b = Bitmap::from_sorted_iter_complemented([5, 10, 15].into_iter(), 64);
        let c = &a | &b;
        assert!(c.is_inverted());
        assert!(!c.contains(5));
        assert!(c.contains(10));
        assert!(!c.contains(15));
        assert!(c.contains(20));
        assert!(c.contains(0));
        assert!(c.contains(63));
    }

    #[test]
    fn test_bitmap_or_in() {
        let a = Bitmap::from_sorted_iter_complemented([5, 10, 15].into_iter(), 64);
        let b = Bitmap::from_sorted_iter([10, 20].into_iter(), 64);
        let c = &a | &b;
        assert!(c.is_inverted());
        assert!(!c.contains(5));
        assert!(c.contains(10));
        assert!(!c.contains(15));
        assert!(c.contains(20));
    }

    #[test]
    fn test_bitmap_or_ii() {
        // A = universe \ {0,1,2}, B = universe \ {2,3,4}
        // A ∪ B = universe \ ({0,1,2} ∩ {2,3,4}) = universe \ {2}
        let a = Bitmap::from_sorted_iter_complemented([0, 1, 2].into_iter(), 64);
        let b = Bitmap::from_sorted_iter_complemented([2, 3, 4].into_iter(), 64);
        let c = &a | &b;
        assert!(c.is_inverted());
        assert_eq!(c.len(), 63);
        assert!(c.contains(0));
        assert!(c.contains(1));
        assert!(!c.contains(2));
        assert!(c.contains(3));
        assert!(c.contains(4));
    }

    #[test]
    fn test_bitmap_and_with_empty() {
        let a = Bitmap::from_sorted_iter([10, 20].into_iter(), 64);
        let b = Bitmap::empty(64);
        let c = &a & &b;
        assert!(c.is_empty());

        let c = &a & &Bitmap::full(64);
        assert_eq!(c.len(), 2);
        assert!(c.contains(10));
        assert!(c.contains(20));
    }

    #[test]
    fn test_bitmap_or_with_full() {
        let a = Bitmap::from_sorted_iter([10, 20].into_iter(), 64);
        let b = Bitmap::full(64);
        let c = &a | &b;
        assert_eq!(c.len(), 64);
    }

    #[test]
    fn test_bitmap_assign_variants() {
        let b = Bitmap::from_sorted_iter([2, 3, 4].into_iter(), 64);

        let mut a = Bitmap::from_sorted_iter([0, 1, 2].into_iter(), 64);
        a &= &b;
        assert!(a.contains(2));
        assert!(!a.contains(0));

        let mut a = Bitmap::from_sorted_iter([0, 1, 2].into_iter(), 64);
        a |= &b;
        for v in [0, 1, 2, 3, 4] {
            assert!(a.contains(v), "missing {v}");
        }
    }

    #[test]
    fn test_bitmap_and_exhaustive() {
        // Test all 4 representation combos against brute-force reference.
        let universe = 64u32;
        let a_vals: Vec<u32> = vec![0, 1, 7, 8, 32, 63];
        let b_vals: Vec<u32> = vec![1, 8, 32, 40, 63];
        let a_complement: Vec<u32> = (0..universe).filter(|v| !a_vals.contains(v)).collect();
        let b_complement: Vec<u32> = (0..universe).filter(|v| !b_vals.contains(v)).collect();

        let representations = [
            (
                Bitmap::from_sorted_iter(a_vals.iter().copied(), universe),
                Bitmap::from_sorted_iter(b_vals.iter().copied(), universe),
                "N&N",
            ),
            (
                Bitmap::from_sorted_iter(a_vals.iter().copied(), universe),
                Bitmap::from_sorted_iter_complemented(b_complement.iter().copied(), universe),
                "N&I",
            ),
            (
                Bitmap::from_sorted_iter_complemented(a_complement.iter().copied(), universe),
                Bitmap::from_sorted_iter(b_vals.iter().copied(), universe),
                "I&N",
            ),
            (
                Bitmap::from_sorted_iter_complemented(a_complement.iter().copied(), universe),
                Bitmap::from_sorted_iter_complemented(b_complement.iter().copied(), universe),
                "I&I",
            ),
        ];

        for (a, b, label) in &representations {
            let c = a & b;
            for v in 0..universe {
                let expected = a_vals.contains(&v) && b_vals.contains(&v);
                assert_eq!(c.contains(v), expected, "{label}: value {v}");
            }
        }
    }

    #[test]
    fn test_bitmap_or_exhaustive() {
        let universe = 64u32;
        let a_vals: Vec<u32> = vec![0, 1, 7, 8, 32, 63];
        let b_vals: Vec<u32> = vec![1, 8, 32, 40, 63];
        let a_complement: Vec<u32> = (0..universe).filter(|v| !a_vals.contains(v)).collect();
        let b_complement: Vec<u32> = (0..universe).filter(|v| !b_vals.contains(v)).collect();

        let representations = [
            (
                Bitmap::from_sorted_iter(a_vals.iter().copied(), universe),
                Bitmap::from_sorted_iter(b_vals.iter().copied(), universe),
                "N|N",
            ),
            (
                Bitmap::from_sorted_iter(a_vals.iter().copied(), universe),
                Bitmap::from_sorted_iter_complemented(b_complement.iter().copied(), universe),
                "N|I",
            ),
            (
                Bitmap::from_sorted_iter_complemented(a_complement.iter().copied(), universe),
                Bitmap::from_sorted_iter(b_vals.iter().copied(), universe),
                "I|N",
            ),
            (
                Bitmap::from_sorted_iter_complemented(a_complement.iter().copied(), universe),
                Bitmap::from_sorted_iter_complemented(b_complement.iter().copied(), universe),
                "I|I",
            ),
        ];

        for (a, b, label) in &representations {
            let c = a | b;
            for v in 0..universe {
                let expected = a_vals.contains(&v) || b_vals.contains(&v);
                assert_eq!(c.contains(v), expected, "{label}: value {v}");
            }
        }
    }

    // ---- Property tests using roaring as oracle ----

    #[cfg(feature = "roaring")]
    mod proptests {
        use super::*;
        use proptest::prelude::*;
        use roaring::RoaringBitmap;

        /// Maximum universe size for property tests. Kept small enough that
        /// exhaustive membership checks (0..universe) are fast.
        const MAX_UNIVERSE: u32 = 4096;

        /// Strategy: generate a (universe_size, sorted-deduped values) pair.
        fn arb_bitmap() -> impl Strategy<Value = (u32, Vec<u32>)> {
            (1u32..=MAX_UNIVERSE).prop_flat_map(|universe| {
                proptest::collection::vec(0..universe, 0..=(universe.min(256) as usize)).prop_map(
                    move |mut vals| {
                        vals.sort_unstable();
                        vals.dedup();
                        (universe, vals)
                    },
                )
            })
        }

        /// Build both a RawBitmap and a RoaringBitmap from the same values.
        fn make_pair(universe: u32, vals: &[u32]) -> (RawBitmap, RoaringBitmap) {
            let raw = RawBitmap::from_sorted_iter(vals.iter().copied(), universe);
            let roaring = RoaringBitmap::from_sorted_iter(vals.iter().copied()).unwrap();
            (raw, roaring)
        }

        // ===== Construction & queries =====

        proptest! {
            #[test]
            fn contains_matches_roaring((universe, vals) in arb_bitmap()) {
                let (raw, roaring) = make_pair(universe, &vals);
                for v in 0..universe {
                    prop_assert_eq!(
                        raw.contains(v),
                        roaring.contains(v),
                        "contains({}) mismatch, universe={}", v, universe
                    );
                }
            }

            #[test]
            fn iter_matches_roaring((universe, vals) in arb_bitmap()) {
                let (raw, roaring) = make_pair(universe, &vals);
                let raw_vals: Vec<u32> = raw.iter().collect();
                let roaring_vals: Vec<u32> = roaring.iter().collect();
                prop_assert_eq!(raw_vals, roaring_vals);
            }

            #[test]
            fn len_matches_roaring((universe, vals) in arb_bitmap()) {
                let (raw, roaring) = make_pair(universe, &vals);
                prop_assert_eq!(raw.len(), roaring.len());
            }

            #[test]
            fn min_max_match_roaring((universe, vals) in arb_bitmap()) {
                let (raw, roaring) = make_pair(universe, &vals);
                prop_assert_eq!(raw.min(), roaring.min());
                prop_assert_eq!(raw.max(), roaring.max());
            }

            #[test]
            fn estimate_data_size_is_exact((universe, vals) in arb_bitmap()) {
                let est = estimate_data_size(universe, vals.iter().copied());
                let raw = RawBitmap::from_sorted_iter(vals.iter().copied(), universe);
                prop_assert_eq!(est, raw.data().len());
            }
        }

        // ===== Insert / remove =====

        proptest! {
            #[test]
            fn insert_matches_roaring(
                (universe, vals) in arb_bitmap(),
                extra in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..32),
            ) {
                let (mut raw, mut roaring) = make_pair(universe, &vals);
                for v in extra {
                    let v = v % universe;
                    raw.insert(v);
                    roaring.insert(v);
                }

                let raw_vals: Vec<u32> = raw.iter().collect();
                let roaring_vals: Vec<u32> = roaring.iter().collect();
                prop_assert_eq!(raw_vals, roaring_vals);
            }

            #[test]
            fn remove_matches_roaring(
                (universe, vals) in arb_bitmap(),
                to_remove in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..32),
            ) {
                let (mut raw, mut roaring) = make_pair(universe, &vals);
                for v in to_remove {
                    let v = v % universe;
                    raw.remove(v);
                    roaring.remove(v);
                }

                let raw_vals: Vec<u32> = raw.iter().collect();
                let roaring_vals: Vec<u32> = roaring.iter().collect();
                prop_assert_eq!(raw_vals, roaring_vals);
            }
        }

        // ===== Set operations =====

        proptest! {
            #[test]
            fn union_matches_roaring(
                (universe, a_vals) in arb_bitmap(),
                b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=256usize),
            ) {
                let b_vals = {
                    let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };

                let (raw_a, roaring_a) = make_pair(universe, &a_vals);
                let (raw_b, roaring_b) = make_pair(universe, &b_vals);

                let raw_result = &raw_a | &raw_b;
                let roaring_result = &roaring_a | &roaring_b;

                let raw_out: Vec<u32> = raw_result.iter().collect();
                let roaring_out: Vec<u32> = roaring_result.iter().collect();
                prop_assert_eq!(raw_out, roaring_out);
            }

            #[test]
            fn intersection_matches_roaring(
                (universe, a_vals) in arb_bitmap(),
                b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=256usize),
            ) {
                let b_vals = {
                    let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };

                let (raw_a, roaring_a) = make_pair(universe, &a_vals);
                let (raw_b, roaring_b) = make_pair(universe, &b_vals);

                let raw_result = &raw_a & &raw_b;
                let roaring_result = &roaring_a & &roaring_b;

                let raw_out: Vec<u32> = raw_result.iter().collect();
                let roaring_out: Vec<u32> = roaring_result.iter().collect();
                prop_assert_eq!(raw_out, roaring_out);
            }

            #[test]
            fn difference_matches_roaring(
                (universe, a_vals) in arb_bitmap(),
                b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=256usize),
            ) {
                let b_vals = {
                    let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };

                let (raw_a, roaring_a) = make_pair(universe, &a_vals);
                let (raw_b, roaring_b) = make_pair(universe, &b_vals);

                let raw_result = &raw_a - &raw_b;
                let roaring_result = &roaring_a - &roaring_b;

                let raw_out: Vec<u32> = raw_result.iter().collect();
                let roaring_out: Vec<u32> = roaring_result.iter().collect();
                prop_assert_eq!(raw_out, roaring_out);
            }

            #[test]
            fn symmetric_difference_matches_roaring(
                (universe, a_vals) in arb_bitmap(),
                b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=256usize),
            ) {
                let b_vals = {
                    let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };

                let (raw_a, roaring_a) = make_pair(universe, &a_vals);
                let (raw_b, roaring_b) = make_pair(universe, &b_vals);

                let raw_result = &raw_a ^ &raw_b;
                let roaring_result = &roaring_a ^ &roaring_b;

                let raw_out: Vec<u32> = raw_result.iter().collect();
                let roaring_out: Vec<u32> = roaring_result.iter().collect();
                prop_assert_eq!(raw_out, roaring_out);
            }
        }

        // ===== Algebraic set properties =====

        proptest! {
            #[test]
            fn union_is_commutative((universe, a_vals) in arb_bitmap(), b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=128usize)) {
                let b_vals = {
                    let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };
                let (a, _) = make_pair(universe, &a_vals);
                let (b, _) = make_pair(universe, &b_vals);
                prop_assert_eq!(&a | &b, &b | &a);
            }

            #[test]
            fn intersection_is_commutative((universe, a_vals) in arb_bitmap(), b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=128usize)) {
                let b_vals = {
                    let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };
                let (a, _) = make_pair(universe, &a_vals);
                let (b, _) = make_pair(universe, &b_vals);
                prop_assert_eq!(&a & &b, &b & &a);
            }

            #[test]
            fn xor_is_commutative((universe, a_vals) in arb_bitmap(), b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=128usize)) {
                let b_vals = {
                    let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };
                let (a, _) = make_pair(universe, &a_vals);
                let (b, _) = make_pair(universe, &b_vals);
                prop_assert_eq!(&a ^ &b, &b ^ &a);
            }

            #[test]
            fn union_with_empty_is_identity((universe, vals) in arb_bitmap()) {
                let (a, _) = make_pair(universe, &vals);
                let empty = RawBitmap::empty(universe);
                prop_assert_eq!(&a | &empty, a.clone());
                prop_assert_eq!(&empty | &a, a);
            }

            #[test]
            fn intersection_with_empty_is_empty((universe, vals) in arb_bitmap()) {
                let (a, _) = make_pair(universe, &vals);
                let empty = RawBitmap::empty(universe);
                prop_assert!((&a & &empty).is_empty());
                prop_assert!((&empty & &a).is_empty());
            }

            #[test]
            fn difference_with_self_is_empty((universe, vals) in arb_bitmap()) {
                let (a, _) = make_pair(universe, &vals);
                prop_assert!((&a - &a).is_empty());
            }

            #[test]
            fn xor_with_self_is_empty((universe, vals) in arb_bitmap()) {
                let (a, _) = make_pair(universe, &vals);
                prop_assert!((&a ^ &a).is_empty());
            }

            #[test]
            fn intersection_is_subset_of_union(
                (universe, a_vals) in arb_bitmap(),
                b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=128usize),
            ) {
                let b_vals = {
                    let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };
                let (a, _) = make_pair(universe, &a_vals);
                let (b, _) = make_pair(universe, &b_vals);

                let inter = &a & &b;
                let union = &a | &b;

                // Every element in the intersection must be in the union.
                for v in inter.iter() {
                    prop_assert!(union.contains(v), "intersection element {} not in union", v);
                }
                prop_assert!(inter.len() <= union.len());
            }
        }

        // ===== Bitmap (De Morgan wrapper) property tests =====

        /// Strategy: generate a (universe, vals, inverted) triple.
        fn arb_bitmap_wrapper() -> impl Strategy<Value = (u32, Vec<u32>, bool)> {
            arb_bitmap().prop_flat_map(|(universe, vals)| {
                proptest::bool::ANY.prop_map(move |inv| (universe, vals.clone(), inv))
            })
        }

        fn make_bitmap_wrapper(universe: u32, vals: &[u32], inverted: bool) -> Bitmap {
            if inverted {
                // vals are the complement (unset values)
                Bitmap::from_sorted_iter_complemented(vals.iter().copied(), universe)
            } else {
                Bitmap::from_sorted_iter(vals.iter().copied(), universe)
            }
        }

        proptest! {
            #[test]
            fn bitmap_contains_matches_roaring(
                (universe, vals, inverted) in arb_bitmap_wrapper(),
            ) {
                let bm = make_bitmap_wrapper(universe, &vals, inverted);

                // Build the expected set using roaring.
                let stored = RoaringBitmap::from_sorted_iter(vals.iter().copied()).unwrap();

                for v in 0..universe {
                    let in_stored = stored.contains(v);
                    let expected = if inverted { !in_stored } else { in_stored };
                    prop_assert_eq!(
                        bm.contains(v), expected,
                        "Bitmap.contains({}) wrong, inverted={}", v, inverted
                    );
                }
            }

            #[test]
            fn bitmap_len_matches_roaring(
                (universe, vals, inverted) in arb_bitmap_wrapper(),
            ) {
                let bm = make_bitmap_wrapper(universe, &vals, inverted);
                let stored = RoaringBitmap::from_sorted_iter(vals.iter().copied()).unwrap();
                let expected = if inverted {
                    universe as u64 - stored.len()
                } else {
                    stored.len()
                };
                prop_assert_eq!(bm.len(), expected);
            }

            #[test]
            fn bitmap_and_matches_oracle(
                (universe, a_vals) in arb_bitmap(),
                b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=128usize),
                a_inv in proptest::bool::ANY,
                b_inv in proptest::bool::ANY,
            ) {
                let b_vals = {
                    let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };

                let bm_a = make_bitmap_wrapper(universe, &a_vals, a_inv);
                let bm_b = make_bitmap_wrapper(universe, &b_vals, b_inv);
                let result = &bm_a & &bm_b;

                // Oracle: compute expected membership directly.
                let stored_a = RoaringBitmap::from_sorted_iter(a_vals.iter().copied()).unwrap();
                let stored_b = RoaringBitmap::from_sorted_iter(b_vals.iter().copied()).unwrap();

                for v in 0..universe {
                    let a_has = if a_inv { !stored_a.contains(v) } else { stored_a.contains(v) };
                    let b_has = if b_inv { !stored_b.contains(v) } else { stored_b.contains(v) };
                    let expected = a_has && b_has;
                    prop_assert_eq!(
                        result.contains(v), expected,
                        "AND({}): a_inv={} b_inv={}", v, a_inv, b_inv
                    );
                }
            }

            #[test]
            fn bitmap_or_matches_oracle(
                (universe, a_vals) in arb_bitmap(),
                b_frac in proptest::collection::vec(0u32..MAX_UNIVERSE, 0..=128usize),
                a_inv in proptest::bool::ANY,
                b_inv in proptest::bool::ANY,
            ) {
                let b_vals = {
                    let mut v: Vec<u32> = b_frac.into_iter().map(|x| x % universe).collect();
                    v.sort_unstable();
                    v.dedup();
                    v
                };

                let bm_a = make_bitmap_wrapper(universe, &a_vals, a_inv);
                let bm_b = make_bitmap_wrapper(universe, &b_vals, b_inv);
                let result = &bm_a | &bm_b;

                let stored_a = RoaringBitmap::from_sorted_iter(a_vals.iter().copied()).unwrap();
                let stored_b = RoaringBitmap::from_sorted_iter(b_vals.iter().copied()).unwrap();

                for v in 0..universe {
                    let a_has = if a_inv { !stored_a.contains(v) } else { stored_a.contains(v) };
                    let b_has = if b_inv { !stored_b.contains(v) } else { stored_b.contains(v) };
                    let expected = a_has || b_has;
                    prop_assert_eq!(
                        result.contains(v), expected,
                        "OR({}): a_inv={} b_inv={}", v, a_inv, b_inv
                    );
                }
            }
        }

        // ===== Serialization roundtrip =====

        proptest! {
            #[test]
            fn serialize_roundtrip((universe, vals) in arb_bitmap()) {
                let raw = RawBitmap::from_sorted_iter(vals.iter().copied(), universe);
                let mut buf = Vec::new();
                raw.serialize_into(&mut buf).unwrap();
                let raw2 = RawBitmap::deserialize_from(&buf[..]).unwrap();
                prop_assert_eq!(raw, raw2);
            }

            #[test]
            fn roaring_roundtrip((universe, vals) in arb_bitmap()) {
                let raw = RawBitmap::from_sorted_iter(vals.iter().copied(), universe);
                let rb = RoaringBitmap::from(&raw);
                let raw2 = RawBitmap::from_roaring(&rb, universe);
                prop_assert_eq!(raw, raw2);
            }

            #[test]
            fn range_cardinality_matches_roaring(
                (universe, vals) in arb_bitmap(),
                range_start in 0u32..MAX_UNIVERSE,
                range_end in 0u32..MAX_UNIVERSE,
            ) {
                let lo = range_start.min(range_end) % universe;
                let hi = (range_start.max(range_end) % universe).saturating_add(1);
                let range = lo..hi;

                let (raw, roaring) = make_pair(universe, &vals);
                prop_assert_eq!(
                    raw.range_cardinality(range.clone()),
                    roaring.range_cardinality(range)
                );
            }

            #[test]
            fn range_cardinality_full_equals_len((universe, vals) in arb_bitmap()) {
                let raw = RawBitmap::from_sorted_iter(vals.iter().copied(), universe);
                prop_assert_eq!(raw.range_cardinality(..), raw.len());
            }

            #[test]
            fn remove_range_matches_roaring(
                (universe, vals) in arb_bitmap(),
                range_start in 0u32..MAX_UNIVERSE,
                range_end in 0u32..MAX_UNIVERSE,
            ) {
                let lo = range_start.min(range_end) % universe;
                let hi = (range_start.max(range_end) % universe).saturating_add(1);

                let (mut raw, mut roaring) = make_pair(universe, &vals);
                raw.remove_range(lo..hi);
                roaring.remove_range(lo..hi);

                let raw_vals: Vec<u32> = raw.iter().collect();
                let roaring_vals: Vec<u32> = roaring.iter().collect();
                prop_assert_eq!(raw_vals, roaring_vals);
            }
        }
    }

    #[cfg(feature = "roaring")]
    mod roaring_tests {
        use super::*;
        use roaring::RoaringBitmap;

        #[test]
        fn test_rawbitmap_from_roaring() {
            let mut rb = RoaringBitmap::new();
            rb.insert(0);
            rb.insert(42);
            rb.insert(511);

            let bm = RawBitmap::from_roaring(&rb, 512);
            assert_eq!(bm.universe_size(), 512);
            assert!(bm.contains(0));
            assert!(bm.contains(42));
            assert!(bm.contains(511));
            assert!(!bm.contains(1));
            assert_eq!(bm.len(), 3);
        }

        #[test]
        fn test_rawbitmap_from_roaring_ref() {
            let mut rb = RoaringBitmap::new();
            rb.insert(10);
            rb.insert(100);

            let bm = RawBitmap::from(&rb);
            // universe_size = max + 1 = 101
            assert_eq!(bm.universe_size(), 101);
            assert!(bm.contains(10));
            assert!(bm.contains(100));
            assert_eq!(bm.len(), 2);
        }

        #[test]
        fn test_rawbitmap_from_roaring_empty() {
            let rb = RoaringBitmap::new();
            let bm = RawBitmap::from(&rb);
            assert!(bm.is_empty());
            assert_eq!(bm.universe_size(), 0);

            let bm = RawBitmap::from_roaring(&rb, 64);
            assert!(bm.is_empty());
            assert_eq!(bm.universe_size(), 64);
        }

        #[test]
        fn test_roaring_from_rawbitmap() {
            let bm = make_bitmap(512, &[0, 42, 255, 511]);
            let rb = RoaringBitmap::from(&bm);
            assert_eq!(rb.len(), 4);
            assert!(rb.contains(0));
            assert!(rb.contains(42));
            assert!(rb.contains(255));
            assert!(rb.contains(511));
        }

        #[test]
        fn test_roaring_from_rawbitmap_empty() {
            let bm = RawBitmap::empty(64);
            let rb = RoaringBitmap::from(&bm);
            assert!(rb.is_empty());
        }

        #[test]
        fn test_roaring_roundtrip() {
            let values = [0, 1, 7, 8, 63, 64, 255, 256, 511];
            let mut rb = RoaringBitmap::new();
            for &v in &values {
                rb.insert(v);
            }

            let bm = RawBitmap::from_roaring(&rb, 512);
            let rb2 = RoaringBitmap::from(&bm);
            assert_eq!(rb, rb2);
        }
    }
}
