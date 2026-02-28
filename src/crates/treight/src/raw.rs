use std::io;
use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Sub, SubAssign};

use crate::ceil_log8;
use crate::node::{child_index, child_offset, skip_subtree_at, NodeReader};
use crate::ops::{
    contains_inner, difference_subtree, intersect_subtree, range_count, remove_range_subtree,
    symmetric_difference_subtree, union_subtree,
};

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

    /// The raw treight data bytes.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Create a cursor for reading nodes sequentially.
    pub(crate) fn nodes(&self) -> NodeReader<'_> {
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
