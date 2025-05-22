use crate::journal_filter::FilterExpr;
use error::{JournalError, Result};
use object_file::{offset_array, offset_array::Direction, ObjectFile};
use window_manager::MemoryMap;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Location {
    Head,
    Tail,
    Realtime(u64),
    Monotonic(u64, [u8; 16]),
    Seqnum(u64, Option<[u8; 16]>),
    XorHash(u64),
    Entry(u64),
    ResolvedEntry(u64),
}

impl Default for Location {
    fn default() -> Self {
        Self::Head
    }
}

#[derive(Debug)]
pub struct JournalCursor {
    pub location: Location,
    pub filter_expr: Option<FilterExpr>,
    pub array_cursor: Option<offset_array::Cursor>,
}

impl JournalCursor {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            location: Location::Head,
            filter_expr: None,
            array_cursor: None,
        }
    }

    pub fn set_location(&mut self, location: Location) {
        self.location = location;
        self.array_cursor = None;
    }

    pub fn set_filter(&mut self, filter_expr: FilterExpr) {
        self.filter_expr = Some(filter_expr);
        // FIXME: should we set cursor to None?
    }

    pub fn clear_filter(&mut self) {
        self.filter_expr = None;
        self.array_cursor = None;
        self.set_location(Location::Head);
    }

    pub fn step<M: MemoryMap>(
        &mut self,
        object_file: &ObjectFile<M>,
        direction: Direction,
    ) -> Result<bool> {
        let new_location = if self.filter_expr.is_some() {
            self.resolve_filter_location(object_file, direction)?
        } else {
            self.resolve_array_cursor(object_file, direction)?
        };

        if let Some(location) = new_location {
            self.location = location;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn position(&self) -> Result<u64> {
        match self.location {
            Location::Entry(entry_offset) => Ok(entry_offset),
            Location::ResolvedEntry(entry_offset) => Ok(entry_offset),
            _ => Err(JournalError::UnsetCursor),
        }
    }

    fn resolve_array_cursor<M: MemoryMap>(
        &mut self,
        object_file: &ObjectFile<M>,
        direction: Direction,
    ) -> Result<Option<Location>> {
        let new_location = match (self.location, direction) {
            (Location::Head, Direction::Forward) => {
                let entry_list = object_file
                    .entry_list()
                    .ok_or(JournalError::InvalidOffsetArrayOffset)?;

                let cursor = entry_list.cursor_head();
                let offset = cursor.value(object_file)?;
                self.array_cursor = Some(cursor);
                Some(Location::ResolvedEntry(offset))
            }
            (Location::Head, Direction::Backward) => None,
            (Location::Tail, Direction::Forward) => None,
            (Location::Tail, Direction::Backward) => {
                let entry_list = object_file
                    .entry_list()
                    .ok_or(JournalError::InvalidOffsetArrayOffset)?;

                let cursor = entry_list.cursor_tail(object_file)?;
                let offset = cursor.value(object_file)?;
                self.array_cursor = Some(cursor);
                Some(Location::ResolvedEntry(offset))
            }
            (Location::Realtime(realtime), _) => {
                let entry_list = object_file
                    .entry_list()
                    .ok_or(JournalError::InvalidOffsetArrayOffset)?;

                let predicate = |entry_offset| {
                    let entry_object = object_file.entry_object(entry_offset)?;
                    Ok(entry_object.header.realtime < realtime)
                };

                let Some(cursor) = entry_list.directed_partition_point(
                    object_file,
                    predicate,
                    Direction::Forward,
                )?
                else {
                    return Ok(None);
                };

                let offset = cursor.value(object_file)?;
                self.array_cursor = Some(cursor);
                Some(Location::ResolvedEntry(offset))
            }
            (Location::ResolvedEntry(_), Direction::Forward) => {
                let Some(cursor) = self.array_cursor.unwrap().next(object_file)? else {
                    return Ok(None);
                };

                let offset = cursor.value(object_file)?;
                self.array_cursor = Some(cursor);
                Some(Location::ResolvedEntry(offset))
            }
            (Location::ResolvedEntry(_), Direction::Backward) => {
                let Some(cursor) = self.array_cursor.unwrap().previous(object_file)? else {
                    return Ok(None);
                };

                let offset = cursor.value(object_file)?;
                self.array_cursor = Some(cursor);
                Some(Location::ResolvedEntry(offset))
            }
            _ => {
                unimplemented!()
            }
        };

        Ok(new_location)
    }

    fn resolve_filter_location<M: MemoryMap>(
        &mut self,
        object_file: &ObjectFile<M>,
        direction: Direction,
    ) -> Result<Option<Location>> {
        let resolved_location = match (self.location, direction) {
            (Location::Head, Direction::Forward) => {
                self.filter_expr.as_mut().unwrap().head();

                self.filter_expr
                    .as_mut()
                    .unwrap()
                    .next(object_file, u64::MIN)?
                    .map(Location::ResolvedEntry)
            }
            (Location::Head, Direction::Backward) => None,
            (Location::Tail, Direction::Forward) => None,
            (Location::Tail, Direction::Backward) => {
                self.filter_expr.as_mut().unwrap().tail(object_file)?;

                self.filter_expr
                    .as_mut()
                    .unwrap()
                    .previous(object_file, u64::MAX)?
                    .map(Location::ResolvedEntry)
            }
            (Location::Realtime(realtime), direction) => {
                let predicate = |entry_offset| {
                    let entry_object = object_file.entry_object(entry_offset)?;
                    Ok(entry_object.header.realtime < realtime)
                };

                let entry_list = object_file
                    .entry_list()
                    .ok_or(JournalError::InvalidOffsetArrayOffset)?;

                let entry_offset = entry_list
                    .directed_partition_point(object_file, predicate, direction)?
                    .map(|c| c.value(object_file))
                    .transpose()?;

                if let Some(location_offset) = entry_offset {
                    let needle_offset = match direction {
                        Direction::Forward => location_offset - 1,
                        Direction::Backward => location_offset + 1,
                    };
                    self.location = Location::Entry(needle_offset);
                    self.resolve_filter_location(object_file, direction)?
                } else {
                    None
                }
            }
            (Location::Entry(location_offset), Direction::Forward) => {
                self.filter_expr.as_mut().unwrap().head();

                self.filter_expr
                    .as_mut()
                    .unwrap()
                    .next(object_file, location_offset)?
                    .map(Location::ResolvedEntry)
            }
            (Location::Entry(location_offset), Direction::Backward) => {
                self.filter_expr.as_mut().unwrap().tail(object_file)?;

                self.filter_expr
                    .as_mut()
                    .unwrap()
                    .previous(object_file, location_offset)?
                    .map(Location::ResolvedEntry)
            }
            (Location::ResolvedEntry(location_offset), Direction::Forward) => self
                .filter_expr
                .as_mut()
                .unwrap()
                .next(object_file, location_offset + 1)?
                .map(Location::ResolvedEntry),
            (Location::ResolvedEntry(location_offset), Direction::Backward) => self
                .filter_expr
                .as_mut()
                .unwrap()
                .previous(object_file, location_offset - 1)?
                .map(Location::ResolvedEntry),
            _ => {
                panic!();
            }
        };

        Ok(resolved_location)
    }
}
