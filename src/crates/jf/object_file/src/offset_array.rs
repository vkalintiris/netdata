use crate::object_file::ObjectFile;
use error::{JournalError, Result};
use window_manager::MemoryMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

/// A reference to a single array of offsets in the journal file
pub struct Node<'a, M: MemoryMap> {
    object_file: &'a ObjectFile<M>,
    offset: u64,
    next_offset: u64,
    capacity: usize,
    // Number of items remaining in this array and subsequent arrays
    remaining_items: usize,
}

impl<'a, M: MemoryMap> Node<'a, M> {
    /// Create a new offset array reference
    fn new(object_file: &'a ObjectFile<M>, offset: u64, remaining_items: usize) -> Result<Self> {
        if offset == 0 {
            return Err(JournalError::InvalidOffsetArrayOffset);
        }

        let array = object_file.offset_array_object(offset)?;

        Ok(Self {
            object_file,
            offset,
            next_offset: array.header.next_offset_array,
            capacity: array.capacity(),
            remaining_items,
        })
    }

    /// Get the offset of this array in the file
    pub fn offset(&self) -> u64 {
        self.offset
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
        self.next_offset != 0 && self.remaining_items > self.len()
    }

    /// Get the next array in the chain, if any
    pub fn next(&self) -> Result<Option<Self>> {
        if !self.has_next() {
            return Ok(None);
        }

        Ok(Some(Self::new(
            self.object_file,
            self.next_offset,
            self.remaining_items - self.len(),
        )?))
    }

    /// Get an item at the specified index
    pub fn get(&self, index: usize) -> Result<u64> {
        if index >= self.len() {
            return Err(JournalError::InvalidOffsetArrayIndex);
        }

        let array = self.object_file.offset_array_object(self.offset)?;
        array.get(index, self.remaining_items)
    }

    /// Returns the first index where the predicate returns false, or array length if
    /// the predicate is true for all elements
    pub fn partition_point<F>(&self, left: usize, right: usize, predicate: F) -> Result<usize>
    where
        F: Fn(u64) -> Result<bool>,
    {
        let mut left = left;
        let mut right = right;

        debug_assert!(left <= right);
        debug_assert!(right <= self.len());

        while left != right {
            let mid = left.midpoint(right);
            let offset = self.get(mid)?;

            if predicate(offset)? {
                left = mid + 1;
            } else {
                right = mid;
            }
        }

        Ok(left)
    }

    /// Find the forward or backward (depending on direction) position that matches the predicate.
    pub fn directed_partition_point<F>(
        &self,
        left: usize,
        right: usize,
        predicate: F,
        direction: Direction,
    ) -> Result<Option<usize>>
    where
        F: Fn(u64) -> Result<bool>,
    {
        let index = self.partition_point(left, right, predicate)?;

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

impl<M: MemoryMap> std::fmt::Debug for Node<'_, M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("offset", &format!("0x{:x}", self.offset))
            .field("next_offset", &format!("0x{:x}", self.next_offset))
            .field("capacity", &self.capacity)
            .field("len", &self.len())
            .field("remaining_items", &self.remaining_items)
            .finish()
    }
}

/// A linked list of offset arrays
pub struct List<'a, M: MemoryMap> {
    object_file: &'a ObjectFile<M>,
    head_offset: u64,
    total_items: usize,
}

impl<M: MemoryMap> Clone for List<'_, M> {
    fn clone(&self) -> Self {
        List {
            object_file: self.object_file,
            head_offset: self.head_offset,
            total_items: self.total_items,
        }
    }
}

impl<M: MemoryMap> std::fmt::Debug for List<'_, M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("List")
            .field("head_offset", &format!("0x{:x}", self.head_offset))
            .field("total_items", &self.total_items)
            .finish()
    }
}

impl<'a, M: MemoryMap> List<'a, M> {
    /// Create a new list from head offset and total items
    pub fn new(
        object_file: &'a ObjectFile<M>,
        head_offset: u64,
        total_items: usize,
    ) -> Result<Self> {
        if head_offset == 0 {
            return Err(JournalError::InvalidOffsetArrayOffset);
        }

        Ok(Self {
            object_file,
            head_offset,
            total_items,
        })
    }

    /// Get the head array of this chain
    pub fn head(&self) -> Result<Node<'a, M>> {
        Node::new(self.object_file, self.head_offset, self.total_items)
    }

    /// Get the tail array of this list by traversing from head to tail
    pub fn tail(&self) -> Result<Node<'a, M>> {
        let mut current = self.head()?;

        while let Some(next) = current.next()? {
            current = next;
        }

        Ok(current)
    }

    /// Get an iterator over all array nodes in the list
    pub fn nodes(&'a self) -> NodeIterator<'a, M> {
        NodeIterator {
            chain: self,
            next_offset: Some(self.head_offset),
            remaining_items: self.total_items,
        }
    }

    /// Get a cursor at the first position in the chain
    pub fn cursor_head(self) -> Result<Cursor<'a, M>> {
        Cursor::at_head(self)
    }

    /// Get a cursor at the last position in the chain
    pub fn cursor_tail(self) -> Result<Cursor<'a, M>> {
        Cursor::at_tail(self)
    }

    /// Finds the first/last array item position where the predicate function becomes false
    /// in a chain of offset arrays.
    ///
    /// # Parameters
    /// * `predicate` - Function that takes an array item value and returns true if the search should continue.
    /// * `direction` - Direction of the search (Forward or Backward)
    pub fn directed_partition_point<F>(
        self,
        predicate: F,
        direction: Direction,
    ) -> Result<Option<Cursor<'a, M>>>
    where
        F: Fn(u64) -> Result<bool>,
    {
        let mut last_cursor: Option<Cursor<M>> = None;

        for node in self.nodes().flatten() {
            let left = 0;
            let right = node.len();

            if let Some(index) =
                node.directed_partition_point(left, right, &predicate, direction)?
            {
                let cursor =
                    Cursor::at_position(self.clone(), node.offset, index, node.remaining_items)?;

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
                            continue;
                        } else {
                            return Ok(last_cursor);
                        }
                    }
                }
            } else if direction == Direction::Backward {
                // No match in this array for backward direction
                return Ok(last_cursor);
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

/// Iterator over all arrays in a chain
pub struct NodeIterator<'a, M: MemoryMap> {
    chain: &'a List<'a, M>,
    next_offset: Option<u64>,
    remaining_items: usize,
}

impl<'a, M: MemoryMap> Iterator for NodeIterator<'a, M> {
    type Item = Result<Node<'a, M>>;

    fn next(&mut self) -> Option<Self::Item> {
        let offset = self.next_offset?;

        match Node::new(self.chain.object_file, offset, self.remaining_items) {
            Ok(array) => {
                self.next_offset = if array.has_next() {
                    Some(array.next_offset)
                } else {
                    None
                };
                self.remaining_items -= array.len();
                Some(Ok(array))
            }
            Err(e) => {
                self.next_offset = None;
                Some(Err(e))
            }
        }
    }
}

/// A cursor pointing to a specific position within an offset array chain
pub struct Cursor<'a, M: MemoryMap> {
    offset_array_list: List<'a, M>,
    array_offset: u64,
    array_index: usize,
    remaining_items: usize,
}

impl<M: MemoryMap> Clone for Cursor<'_, M> {
    fn clone(&self) -> Self {
        Cursor {
            offset_array_list: self.offset_array_list.clone(),
            array_offset: self.array_offset,
            array_index: self.array_index,
            remaining_items: self.remaining_items,
        }
    }
}

impl<'a, M: MemoryMap> Cursor<'a, M> {
    pub fn get_metadata(&self) -> CursorMetadata {
        CursorMetadata {
            head_offset: self.offset_array_list.head_offset,
            total_items: self.offset_array_list.total_items,
            array_offset: self.array_offset,
            array_index: self.array_index,
            remaining_items: self.remaining_items,
        }
    }

    pub fn from_metadata(
        object_file: &'a ObjectFile<M>,
        cursor_metadata: &CursorMetadata,
    ) -> Result<Self> {
        let offset_array_list = List::new(
            object_file,
            cursor_metadata.head_offset,
            cursor_metadata.total_items,
        )?;

        Ok(Self {
            offset_array_list,
            array_offset: cursor_metadata.array_offset,
            array_index: cursor_metadata.array_index,
            remaining_items: cursor_metadata.remaining_items,
        })
    }

    /// Create a cursor at the head of the chain
    pub fn at_head(offset_array_list: List<'a, M>) -> Result<Self> {
        let head = offset_array_list.head()?;
        if head.is_empty() {
            return Err(JournalError::EmptyOffsetArrayList);
        }

        Ok(Self {
            offset_array_list: offset_array_list.clone(),
            array_offset: head.offset,
            array_index: 0,
            remaining_items: head.remaining_items,
        })
    }

    /// Create a cursor at the tail of the chain
    pub fn at_tail(offset_array_list: List<'a, M>) -> Result<Self> {
        let mut current_array = offset_array_list.head()?;

        while let Some(next_array) = current_array.next()? {
            current_array = next_array;
        }

        let len = current_array.len();

        if len == 0 {
            return Err(JournalError::EmptyOffsetArrayNode);
        }

        Ok(Self {
            offset_array_list: offset_array_list.clone(),
            array_offset: current_array.offset,
            array_index: len - 1,
            remaining_items: current_array.len(),
        })
    }

    /// Create a cursor at a specific position
    pub fn at_position(
        offset_array_list: List<'a, M>,
        array_offset: u64,
        array_index: usize,
        remaining_items: usize,
    ) -> Result<Self> {
        debug_assert!(offset_array_list.total_items >= remaining_items);

        // Verify the array exists
        let array = Node::new(offset_array_list.object_file, array_offset, remaining_items)?;

        // Verify the index is valid
        if array_index >= array.len() {
            return Err(JournalError::InvalidOffsetArrayIndex);
        }

        Ok(Self {
            offset_array_list: offset_array_list.clone(),
            array_offset,
            array_index,
            remaining_items,
        })
    }

    /// Get the current array this cursor points to
    pub fn node(&self) -> Result<Node<'a, M>> {
        Node::new(
            self.offset_array_list.object_file,
            self.array_offset,
            self.remaining_items,
        )
    }

    pub fn value(&self) -> Result<u64> {
        self.node()?.get(self.array_index)
    }

    /// Move to the next position
    pub fn next(&self) -> Result<Option<Self>> {
        let array_node = self.node()?;

        if self.array_index + 1 < array_node.len() {
            // Next item is in the same array
            return Ok(Some(Self {
                offset_array_list: self.offset_array_list.clone(),
                array_offset: self.array_offset,
                array_index: self.array_index + 1,
                remaining_items: self.remaining_items,
            }));
        }

        if !array_node.has_next() {
            return Ok(None);
        }

        let next_array = array_node.next()?.unwrap();

        Ok(Some(Self {
            offset_array_list: self.offset_array_list.clone(),
            array_offset: next_array.offset,
            array_index: 0,
            remaining_items: self.remaining_items.saturating_sub(array_node.len()),
        }))
    }

    /// Move to the previous position
    pub fn previous(&self) -> Result<Option<Self>> {
        if self.array_index > 0 {
            // Previous item is in the same array
            return Ok(Some(Self {
                offset_array_list: self.offset_array_list.clone(),
                array_offset: self.array_offset,
                array_index: self.array_index - 1,
                remaining_items: self.remaining_items,
            }));
        }

        if self.array_offset == self.offset_array_list.head_offset {
            return Ok(None);
        }

        for array_node in self.offset_array_list.nodes().flatten() {
            if array_node.next_offset == self.array_offset {
                return Ok(Some(Self {
                    offset_array_list: self.offset_array_list.clone(),
                    array_offset: array_node.offset,
                    array_index: array_node.len() - 1,
                    remaining_items: array_node.remaining_items,
                }));
            }
        }

        Err(JournalError::InvalidOffsetArrayOffset)
    }
}

impl<M: MemoryMap> std::fmt::Debug for Cursor<'_, M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cursor")
            .field("array_offset", &format!("0x{:x}", self.array_offset))
            .field("array_index", &self.array_index)
            .field("remaining_items", &self.remaining_items)
            .finish()
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub struct CursorMetadata {
    // from `List` type
    head_offset: u64,
    total_items: usize,

    // from `Cursor` type
    array_offset: u64,
    array_index: usize,
    remaining_items: usize,
}

impl CursorMetadata {
    pub fn new(head_offset: u64, total_items: usize) -> Self {
        Self {
            head_offset,
            total_items,
            array_offset: head_offset,
            array_index: 0,
            remaining_items: total_items,
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct InlinedCursor {
    cursor_metadata: CursorMetadata,
    inlined_offset: u64,
    index: usize,
}

impl InlinedCursor {
    pub fn at_head(inlined_offset: u64, head_offset: u64, total_items: usize) -> Result<Self> {
        if total_items == 0 || inlined_offset == 0 {
            return Err(JournalError::EmptyInlineCursor);
        } else if total_items > 1 && head_offset == 0 {
            return Err(JournalError::EmptyOffsetArrayList);
        }

        let cursor_metadata = CursorMetadata::new(head_offset, total_items.saturating_sub(1));

        Ok(Self {
            cursor_metadata,
            inlined_offset,
            index: 0,
        })
    }

    pub fn at_tail<M: MemoryMap>(&self, object_file: &ObjectFile<M>) -> Result<Self> {
        if self.cursor_metadata.total_items == 0 {
            // We don't have an offset array list, `self` should be positioned
            // at the inlined_offset
            debug_assert_eq!(self.index, 0);
            return Ok(*self);
        }

        // get the offset array list
        let offset_array_list = List::new(
            object_file,
            self.cursor_metadata.head_offset,
            self.cursor_metadata.total_items,
        )?;
        // construct a cursor at the tail
        let cursor = Cursor::at_tail(offset_array_list)?;
        let cursor_metadata = cursor.get_metadata();

        // Advance the index
        let mut ic = *self;
        ic.index = cursor_metadata.total_items + 1;
        Ok(ic)
    }

    pub fn next<M: MemoryMap>(&self, object_file: &ObjectFile<M>) -> Result<Option<Self>> {
        if self.index == 0 {
            if self.cursor_metadata.total_items == 0 {
                return Ok(None);
            } else {
                let mut ic = *self;
                ic.index += 1;
                return Ok(Some(ic));
            }
        } else if self.index == (self.cursor_metadata.total_items + 1) {
            return Ok(None);
        }

        let cursor = Cursor::from_metadata(object_file, &self.cursor_metadata)?;
        if let Some(cursor) = cursor.next()?.as_ref() {
            let mut ic = *self;
            ic.cursor_metadata = cursor.get_metadata();
            ic.index += 1;
            Ok(Some(ic))
        } else {
            Ok(None)
        }
    }

    pub fn previous<M: MemoryMap>(&self, object_file: &ObjectFile<M>) -> Result<Option<Self>> {
        if self.index == 0 {
            // Already at the first item (inlined offset)
            return Ok(None);
        } else if self.index == 1 {
            // We're at the first item in the offset array list, go back to inlined offset
            let mut ic = *self;
            ic.index = 0;
            return Ok(Some(ic));
        }

        // We're in the offset array list
        let cursor = Cursor::from_metadata(object_file, &self.cursor_metadata)?;
        if let Some(cursor) = cursor.previous()?.as_ref() {
            let mut ic = *self;
            ic.cursor_metadata = cursor.get_metadata();
            ic.index -= 1;
            Ok(Some(ic))
        } else {
            Ok(None)
        }
    }

    pub fn value<M: MemoryMap>(&self, object_file: &ObjectFile<M>) -> Result<u64> {
        if self.index == 0 {
            Ok(self.inlined_offset)
        } else {
            let cursor = Cursor::from_metadata(object_file, &self.cursor_metadata)?;
            cursor.value()
        }
    }
}
