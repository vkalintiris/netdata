use crate::object_file::ObjectFile;
use error::{JournalError, Result};
use std::num::NonZeroU64;
use window_manager::MemoryMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

/// A reference to a single array of offsets in the journal file
pub struct Node {
    offset: NonZeroU64,
    next_offset: Option<NonZeroU64>,
    capacity: usize,
    // Number of items remaining in this array and subsequent arrays
    remaining_items: usize,
}

impl Node {
    /// Create a new offset array reference
    fn new<M: MemoryMap>(
        object_file: &ObjectFile<M>,
        offset: NonZeroU64,
        remaining_items: usize,
    ) -> Result<Self> {
        let array = object_file.offset_array_object(offset.get())?;

        Ok(Self {
            offset,
            next_offset: NonZeroU64::new(array.header.next_offset_array),
            capacity: array.capacity(),
            remaining_items,
        })
    }

    /// Get the offset of this array in the file
    pub fn offset(&self) -> u64 {
        self.offset.get()
    }

    /// Get the maximum number of items this array can hold
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Get the number of items available in this array
    pub fn len(&self) -> usize {
        self.capacity.min(self.remaining_items)
    }

    /// Check if this array is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Check if this array has a next array in the chain
    pub fn has_next(&self) -> bool {
        self.next_offset.is_some() && self.remaining_items > self.len()
    }

    /// Get the next array in the chain, if any
    pub fn next<M: MemoryMap>(&self, object_file: &ObjectFile<M>) -> Result<Option<Self>> {
        if !self.has_next() {
            return Ok(None);
        }

        let next_offset = self.next_offset.unwrap();
        let remaining_items = self.remaining_items - self.len();
        let node = Self::new(object_file, next_offset, remaining_items);

        Some(node).transpose()
    }

    /// Get an item at the specified index
    pub fn get<M: MemoryMap>(&self, object_file: &ObjectFile<M>, index: usize) -> Result<u64> {
        if index >= self.len() {
            return Err(JournalError::InvalidOffsetArrayIndex);
        }

        let array = object_file.offset_array_object(self.offset.get())?;
        array.get(index, self.remaining_items)
    }

    /// Returns the first index where the predicate returns false, or array length if
    /// the predicate is true for all elements
    pub fn partition_point<M, F>(
        &self,
        object_file: &ObjectFile<M>,
        left: usize,
        right: usize,
        predicate: F,
    ) -> Result<usize>
    where
        M: MemoryMap,
        F: Fn(u64) -> Result<bool>,
    {
        let mut left = left;
        let mut right = right;

        debug_assert!(left <= right);
        debug_assert!(right <= self.len());

        while left != right {
            let mid = left.midpoint(right);
            let offset = self.get(object_file, mid)?;

            if predicate(offset)? {
                left = mid + 1;
            } else {
                right = mid;
            }
        }

        Ok(left)
    }

    /// Find the forward or backward (depending on direction) position that matches the predicate.
    pub fn directed_partition_point<M, F>(
        &self,
        object_file: &ObjectFile<M>,
        left: usize,
        right: usize,
        predicate: F,
        direction: Direction,
    ) -> Result<Option<usize>>
    where
        M: MemoryMap,
        F: Fn(u64) -> Result<bool>,
    {
        let index = self.partition_point(object_file, left, right, predicate)?;

        Ok(match direction {
            Direction::Forward => {
                if index < self.len() {
                    Some(index)
                } else {
                    None
                }
            }
            Direction::Backward => {
                if index > 0 {
                    Some(index - 1)
                } else {
                    None
                }
            }
        })
    }
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let next_offset = self.next_offset.map(|x| x.get()).unwrap_or(0);

        f.debug_struct("Node")
            .field("offset", &format!("0x{:x}", self.offset))
            .field("next_offset", &format!("0x{:x}", next_offset))
            .field("capacity", &self.capacity)
            .field("len", &self.len())
            .field("remaining_items", &self.remaining_items)
            .finish()
    }
}

/// A linked list of offset arrays
#[derive(Copy, Clone)]
pub struct List {
    head_offset: NonZeroU64,
    total_items: usize,
}

impl std::fmt::Debug for List {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("List")
            .field("head_offset", &format!("0x{:x}", self.head_offset))
            .field("total_items", &self.total_items)
            .finish()
    }
}

impl List {
    /// Create a new list from head offset and total items
    pub fn new(head_offset: u64, total_items: usize) -> Result<Self> {
        let head_offset =
            NonZeroU64::new(head_offset).ok_or(JournalError::InvalidOffsetArrayOffset)?;

        Ok(Self {
            head_offset,
            total_items,
        })
    }

    /// Get the head array of this chain
    pub fn head<M: MemoryMap>(&self, object_file: &ObjectFile<M>) -> Result<Node> {
        Node::new(object_file, self.head_offset, self.total_items)
    }

    /// Get the tail array of this list by traversing from head to tail
    pub fn tail<M: MemoryMap>(&self, object_file: &ObjectFile<M>) -> Result<Node> {
        let mut current = self.head(object_file)?;

        while let Some(next) = current.next(object_file)? {
            current = next;
        }

        Ok(current)
    }

    /// Get a cursor at the first position in the chain
    pub fn cursor_head<M: MemoryMap>(self, object_file: &ObjectFile<M>) -> Result<Cursor> {
        Cursor::at_head(object_file, self)
    }

    /// Get a cursor at the last position in the chain
    pub fn cursor_tail<M: MemoryMap>(self, object_file: &ObjectFile<M>) -> Result<Cursor> {
        Cursor::at_tail(object_file, self)
    }

    /// Finds the first/last array item position where the predicate function becomes false
    /// in a chain of offset arrays.
    ///
    /// # Parameters
    /// * `predicate` - Function that takes an array item value and returns true if the search should continue.
    /// * `direction` - Direction of the search (Forward or Backward)
    pub fn directed_partition_point<M, F>(
        self,
        object_file: &ObjectFile<M>,
        predicate: F,
        direction: Direction,
    ) -> Result<Option<Cursor>>
    where
        M: MemoryMap,
        F: Fn(u64) -> Result<bool>,
    {
        let mut last_cursor: Option<Cursor> = None;

        let mut node = self.head(object_file)?;

        loop {
            let left = 0;
            let right = node.len();

            if let Some(index) =
                node.directed_partition_point(object_file, left, right, &predicate, direction)?
            {
                let cursor = Cursor::at_position(
                    object_file,
                    self,
                    node.offset,
                    index,
                    node.remaining_items,
                )?;

                match direction {
                    Direction::Forward => {
                        return Ok(Some(cursor));
                    }
                    Direction::Backward => {
                        // In backward direction, save this match and continue
                        // to ensure we'll find the last match
                        last_cursor = Some(cursor);

                        // If this match is at the end of the array and there's a next array,
                        // we should check the next array as well
                        if index == node.len() - 1 && node.has_next() {
                            // continue;
                        } else {
                            return Ok(last_cursor);
                        }
                    }
                }
            } else if direction == Direction::Backward {
                // No match in this array for backward direction
                return Ok(last_cursor);
            }

            if let Some(nd) = node.next(object_file)? {
                node = nd;
            } else {
                break;
            }
        }

        // For backward direction, return the last match we found (if any)
        if direction == Direction::Backward {
            return Ok(last_cursor);
        }

        // No match found in any array
        Ok(None)
    }
}

/// A cursor pointing to a specific position within an offset array chain
#[derive(Clone, Copy)]
pub struct Cursor {
    offset_array_list: List,
    array_offset: NonZeroU64,
    array_index: usize,
    remaining_items: usize,
}

impl Cursor {
    /// Create a cursor at the head of the chain
    pub fn at_head<M: MemoryMap>(
        object_file: &ObjectFile<M>,
        offset_array_list: List,
    ) -> Result<Self> {
        let head = offset_array_list.head(object_file)?;
        if head.is_empty() {
            return Err(JournalError::EmptyOffsetArrayList);
        }

        Ok(Self {
            offset_array_list,
            array_offset: head.offset,
            array_index: 0,
            remaining_items: head.remaining_items,
        })
    }

    /// Create a cursor at the tail of the chain
    pub fn at_tail<M: MemoryMap>(
        object_file: &ObjectFile<M>,
        offset_array_list: List,
    ) -> Result<Self> {
        let mut current_array = offset_array_list.head(object_file)?;

        while let Some(next_array) = current_array.next(object_file)? {
            current_array = next_array;
        }

        let len = current_array.len();

        if len == 0 {
            return Err(JournalError::EmptyOffsetArrayNode);
        }

        Ok(Self {
            offset_array_list,
            array_offset: current_array.offset,
            array_index: len - 1,
            remaining_items: current_array.len(),
        })
    }

    /// Create a cursor at a specific position
    pub fn at_position<M: MemoryMap>(
        object_file: &ObjectFile<M>,
        offset_array_list: List,
        array_offset: NonZeroU64,
        array_index: usize,
        remaining_items: usize,
    ) -> Result<Self> {
        debug_assert!(offset_array_list.total_items >= remaining_items);

        // Verify the array exists
        let array = Node::new(object_file, array_offset, remaining_items)?;

        // Verify the index is valid
        if array_index >= array.len() {
            return Err(JournalError::InvalidOffsetArrayIndex);
        }

        Ok(Self {
            offset_array_list,
            array_offset,
            array_index,
            remaining_items,
        })
    }

    /// Get the current array this cursor points to
    pub fn node<M: MemoryMap>(&self, object_file: &ObjectFile<M>) -> Result<Node> {
        Node::new(object_file, self.array_offset, self.remaining_items)
    }

    pub fn value<M: MemoryMap>(&self, object_file: &ObjectFile<M>) -> Result<u64> {
        self.node(object_file)?.get(object_file, self.array_index)
    }

    /// Move to the next position
    pub fn next<M: MemoryMap>(&self, object_file: &ObjectFile<M>) -> Result<Option<Self>> {
        let array_node = self.node(object_file)?;

        if self.array_index + 1 < array_node.len() {
            // Next item is in the same array
            return Ok(Some(Self {
                offset_array_list: self.offset_array_list,
                array_offset: self.array_offset,
                array_index: self.array_index + 1,
                remaining_items: self.remaining_items,
            }));
        }

        if !array_node.has_next() {
            return Ok(None);
        }

        let next_array = array_node.next(object_file)?.unwrap();

        Ok(Some(Self {
            offset_array_list: self.offset_array_list,
            array_offset: next_array.offset,
            array_index: 0,
            remaining_items: self.remaining_items.saturating_sub(array_node.len()),
        }))
    }

    /// Move to the previous position
    pub fn previous<M: MemoryMap>(&self, object_file: &ObjectFile<M>) -> Result<Option<Self>> {
        if self.array_index > 0 {
            // Previous item is in the same array
            return Ok(Some(Self {
                offset_array_list: self.offset_array_list,
                array_offset: self.array_offset,
                array_index: self.array_index - 1,
                remaining_items: self.remaining_items,
            }));
        }

        if self.array_offset == self.offset_array_list.head_offset {
            return Ok(None);
        }

        let mut node = self.offset_array_list.head(object_file)?;
        while node.has_next() {
            if node.next_offset == Some(self.array_offset) {
                return Ok(Some(Self {
                    offset_array_list: self.offset_array_list,
                    array_offset: node.offset,
                    array_index: node.len() - 1,
                    remaining_items: node.remaining_items,
                }));
            }

            node = node.next(object_file)?.unwrap();
        }

        Err(JournalError::InvalidOffsetArrayOffset)
    }
}

impl std::fmt::Debug for Cursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cursor")
            .field("array_offset", &format!("0x{:x}", self.array_offset))
            .field("array_index", &self.array_index)
            .field("remaining_items", &self.remaining_items)
            .finish()
    }
}

#[derive(Debug, Copy, Clone)]
pub struct InlinedCursor {
    // cursor: Cursor,
    // inlined_offset: u64,
    // index: usize,
}

impl InlinedCursor {
    pub fn at_head(_inlined_offset: u64, _head_offset: u64, _total_items: usize) -> Result<Self> {
        todo!();
    }

    pub fn at_tail<M: MemoryMap>(&self, _object_file: &ObjectFile<M>) -> Result<Self> {
        todo!();
    }

    pub fn next<M: MemoryMap>(&self, _object_file: &ObjectFile<M>) -> Result<Option<Self>> {
        todo!();
    }

    pub fn previous<M: MemoryMap>(&self, _object_file: &ObjectFile<M>) -> Result<Option<Self>> {
        todo!();
    }

    pub fn value<M: MemoryMap>(&self, _object_file: &ObjectFile<M>) -> Result<u64> {
        todo!();
    }
}
