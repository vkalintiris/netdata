use std::iter::Filter;

use error::{JournalError, Result};
use object_file::{
    offset_array::{Direction, InlinedCursor},
    ObjectFile,
};
use window_manager::MemoryMap;

#[derive(Clone, Debug)]
pub enum FilterExpr {
    Match(u64, Option<InlinedCursor>),
    Conjunction(Vec<FilterExpr>),
    Disjunction(Vec<FilterExpr>),
}

impl FilterExpr {
    pub fn lookup<M: MemoryMap>(
        &self,
        object_file: &ObjectFile<M>,
        needle_offset: u64,
        direction: Direction,
    ) -> Result<Option<u64>> {
        let predicate =
            move |entry_offset: u64| -> Result<bool> { Ok(entry_offset < needle_offset) };

        match self {
            FilterExpr::Match(data_offset, _) => {
                let entry_offset = object_file.data_object_directed_partition_point(
                    *data_offset,
                    predicate,
                    direction,
                )?;
                Ok(entry_offset)
            }
            FilterExpr::Conjunction(filter_exprs) => {
                let mut current_offset = needle_offset;

                loop {
                    let previous_offset = current_offset;

                    for filter_expr in filter_exprs {
                        if direction == Direction::Backward {
                            current_offset = current_offset.saturating_add(1);
                        }

                        match filter_expr.lookup(object_file, current_offset, direction)? {
                            Some(new_offset) => current_offset = new_offset,
                            None => return Ok(None),
                        }
                    }

                    if current_offset == previous_offset {
                        return Ok(Some(current_offset));
                    }
                }
            }
            FilterExpr::Disjunction(filter_exprs) => {
                let cmp = match direction {
                    Direction::Forward => std::cmp::min,
                    Direction::Backward => std::cmp::max,
                };

                filter_exprs.iter().try_fold(None, |acc, expr| {
                    let result = expr.lookup(object_file, needle_offset, direction)?;

                    Ok(match (acc, result) {
                        (None, Some(offset)) => Some(offset),
                        (Some(best), Some(offset)) => Some(cmp(best, offset)),
                        (acc, None) => acc,
                    })
                })
            }
        }
    }

    pub fn reset(&mut self) {
        match self {
            FilterExpr::Match(_, None) => (),
            FilterExpr::Match(_, Some(ic)) => {
                *ic = ic.head();
            }
            FilterExpr::Conjunction(filter_exprs) => {
                for filter_expr in filter_exprs.iter_mut() {
                    filter_expr.reset();
                }
            }
            FilterExpr::Disjunction(filter_exprs) => {
                for filter_expr in filter_exprs.iter_mut() {
                    filter_expr.reset();
                }
            }
        }
    }

    pub fn next<M: MemoryMap>(
        &mut self,
        object_file: &ObjectFile<M>,
        needle_offset: u64,
    ) -> Result<Option<u64>> {
        match self {
            FilterExpr::Match(_, None) => Ok(None),
            FilterExpr::Match(_, Some(ic)) => ic.skip_until(object_file, needle_offset),
            FilterExpr::Conjunction(filter_exprs) => {
                let mut needle_offset = needle_offset;

                loop {
                    let previous_offset = needle_offset;

                    for fe in filter_exprs.iter_mut() {
                        if let Some(new_offset) = fe.next(object_file, needle_offset)? {
                            needle_offset = new_offset;
                        } else {
                            return Ok(None);
                        }
                    }

                    if needle_offset == previous_offset {
                        return Ok(Some(needle_offset));
                    }
                }
            }
            FilterExpr::Disjunction(filter_exprs) => {
                let mut best_offset: Option<u64> = None;

                for fe in filter_exprs.iter_mut() {
                    if let Some(fe_offset) = fe.next(object_file, needle_offset)? {
                        best_offset = match best_offset {
                            Some(offset) => Some(fe_offset.min(offset)),
                            None => Some(fe_offset),
                        };
                    }
                }

                todo!()
            }
        }
    }

    // Moves to the next entry that matches this filter expression after the current position
    // Returns the offset of the next matching entry, if any
    // pub fn next<M: MemoryMap>(&mut self, _object_file: &ObjectFile<M>) -> Result<Option<u64>> {
    // match self {
    //     FilterExpr::Match(data_offset, cursor_opt) => {
    //         // If we have a cursor, advance it to the next position
    //         if let Some(cursor) = cursor_opt {
    //             // Try to move to the next position
    //             if let Some(next_cursor) = cursor.next(object_file)? {
    //                 // Update the cursor and return the new position
    //                 *cursor = next_cursor;
    //                 cursor.value(object_file).map(Some)
    //             } else {
    //                 // No more entries for this match
    //                 Ok(None)
    //             }
    //         } else {
    //             // If we don't have a cursor yet, initialize one
    //             let data_obj = object_file.data_object(*data_offset)?;
    //             if let Some(ic) = data_obj.inlined_cursor() {
    //                 *cursor_opt = Some(ic);
    //                 ic.value(object_file).map(Some)
    //             } else {
    //                 // No entries for this match
    //                 Ok(None)
    //             }
    //         }
    //     }
    //     FilterExpr::Conjunction(exprs) => {
    //         // For a conjunction (AND), we need to find positions that satisfy all expressions
    //         let mut current_position: Option<u64> = None;

    //         // Try to advance each expression and find the maximum position
    //         loop {
    //             let mut max_position: Option<u64> = None;

    //             // First, advance all expressions to at least the current position
    //             for expr in exprs.iter_mut() {
    //                 // If we have a current position, look for positions >= current
    //                 if let Some(curr_pos) = current_position {
    //                     let expr_pos =
    //                         match expr.lookup(object_file, curr_pos, Direction::Forward)? {
    //                             Some(pos) => pos,
    //                             None => return Ok(None), // If any expression has no matches, the conjunction fails
    //                         };

    //                     // Keep track of the maximum position
    //                     max_position = Some(max_position.map_or(expr_pos, |p| p.max(expr_pos)));
    //                 } else {
    //                     // First iteration, just get the next position for each expression
    //                     let expr_pos = match expr.next(object_file)? {
    //                         Some(pos) => pos,
    //                         None => return Ok(None), // If any expression has no matches, the conjunction fails
    //                     };

    //                     // Keep track of the maximum position
    //                     max_position = Some(max_position.map_or(expr_pos, |p| p.max(expr_pos)));
    //                 }
    //             }

    //             // If all expressions returned the same position, we've found a match
    //             let max_pos = max_position.unwrap();
    //             if exprs.iter_mut().all(|expr| {
    //                 match expr.lookup(object_file, max_pos, Direction::Forward) {
    //                     Ok(Some(pos)) => pos == max_pos,
    //                     _ => false,
    //                 }
    //             }) {
    //                 return Ok(Some(max_pos));
    //             }

    //             // Not all expressions match this position, try again with a higher position
    //             current_position = Some(max_pos);

    //             // Safety check to avoid infinite loops
    //             if current_position == max_position {
    //                 current_position = Some(max_pos + 1);
    //             }
    //         }
    //     }
    //     FilterExpr::Disjunction(exprs) => {
    //         // For a disjunction (OR), find the minimum next position from all expressions
    //         let mut min_position: Option<u64> = None;

    //         // Try to advance each expression and find the minimum valid position
    //         for expr in exprs.iter_mut() {
    //             if let Some(pos) = expr.next(object_file)? {
    //                 min_position = Some(min_position.map_or(pos, |p| p.min(pos)));
    //             }
    //         }

    //         Ok(min_position)
    //     }
    // }
    // }

    /// Moves to the previous entry that matches this filter expression before the current position
    /// Returns the offset of the previous matching entry, if any
    pub fn prev<M: MemoryMap>(&mut self, _object_file: &ObjectFile<M>) -> Result<Option<u64>> {
        todo!()
        // match self {
        //     FilterExpr::Match(data_offset, cursor_opt) => {
        //         // If we have a cursor, move it to the previous position
        //         if let Some(cursor) = cursor_opt {
        //             // Try to move to the previous position
        //             if let Some(prev_cursor) = cursor.previous(object_file)? {
        //                 // Update the cursor and return the new position
        //                 *cursor = prev_cursor;
        //                 prev_cursor.value(object_file).map(Some)
        //             } else {
        //                 // No more entries for this match
        //                 Ok(None)
        //             }
        //         } else {
        //             // If we don't have a cursor yet, initialize one from the tail
        //             let data_obj = object_file.data_object(*data_offset)?;
        //             if let Some(ic) = data_obj.inlined_cursor() {
        //                 let tail_cursor = ic.at_tail(object_file)?;
        //                 *cursor_opt = Some(tail_cursor);
        //                 tail_cursor.value(object_file).map(Some)
        //             } else {
        //                 // No entries for this match
        //                 Ok(None)
        //             }
        //         }
        //     }
        //     FilterExpr::Conjunction(exprs) => {
        //         // For a conjunction (AND), we need to find positions that satisfy all expressions
        //         let mut current_position: Option<u64> = None;

        //         // Try to move each expression backward and find the minimum position
        //         loop {
        //             let mut min_position: Option<u64> = None;

        //             // First, move all expressions to positions <= current
        //             for expr in exprs.iter_mut() {
        //                 // If we have a current position, look for positions <= current
        //                 if let Some(curr_pos) = current_position {
        //                     let expr_pos =
        //                         match expr.lookup(object_file, curr_pos, Direction::Backward)? {
        //                             Some(pos) => pos,
        //                             None => return Ok(None), // If any expression has no matches, the conjunction fails
        //                         };

        //                     // Keep track of the minimum position
        //                     min_position = Some(min_position.map_or(expr_pos, |p| p.min(expr_pos)));
        //                 } else {
        //                     // First iteration, just get the previous position for each expression
        //                     let expr_pos = match expr.prev(object_file)? {
        //                         Some(pos) => pos,
        //                         None => return Ok(None), // If any expression has no matches, the conjunction fails
        //                     };

        //                     // Keep track of the minimum position
        //                     min_position = Some(min_position.map_or(expr_pos, |p| p.min(expr_pos)));
        //                 }
        //             }

        //             // If all expressions returned the same position, we've found a match
        //             let min_pos = min_position.unwrap();
        //             if exprs.iter_mut().all(|expr| {
        //                 match expr.lookup(object_file, min_pos, Direction::Backward) {
        //                     Ok(Some(pos)) => pos == min_pos,
        //                     _ => false,
        //                 }
        //             }) {
        //                 return Ok(Some(min_pos));
        //             }

        //             // Not all expressions match this position, try again with a lower position
        //             current_position = Some(min_pos);

        //             // Safety check to avoid infinite loops
        //             if current_position == min_position {
        //                 current_position = Some(min_pos.saturating_sub(1));
        //             }
        //         }
        //     }
        //     FilterExpr::Disjunction(exprs) => {
        //         // For a disjunction (OR), find the maximum previous position from all expressions
        //         let mut max_position: Option<u64> = None;

        //         // Try to move each expression backward and find the maximum valid position
        //         for expr in exprs.iter_mut() {
        //             if let Some(pos) = expr.prev(object_file)? {
        //                 max_position = Some(max_position.map_or(pos, |p| p.max(pos)));
        //             }
        //         }

        //         Ok(max_position)
        //     }
        // }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOp {
    Conjunction,
    Disjunction,
}

#[derive(Debug)]
pub struct JournalFilter {
    filter_expr: Option<FilterExpr>,
    current_matches: Vec<Vec<u8>>,
    current_op: LogicalOp,
}

impl Default for JournalFilter {
    fn default() -> Self {
        Self {
            filter_expr: None,
            current_matches: Vec::new(),
            current_op: LogicalOp::Conjunction,
        }
    }
}

impl JournalFilter {
    fn extract_key(kv_pair: &[u8]) -> Option<&[u8]> {
        if let Some(equal_pos) = kv_pair.iter().position(|&b| b == b'=') {
            Some(&kv_pair[..equal_pos])
        } else {
            None
        }
    }

    fn convert_current_matches<M: MemoryMap>(
        &mut self,
        object_file: &ObjectFile<M>,
    ) -> Result<Option<FilterExpr>> {
        if self.current_matches.is_empty() {
            return Ok(None);
        }

        let mut elements = Vec::new();
        let mut i = 0;

        while i < self.current_matches.len() {
            let current_key = Self::extract_key(&self.current_matches[i]).unwrap_or(&[]);
            let start = i;

            // Find all matches with the same key
            while i < self.current_matches.len()
                && Self::extract_key(&self.current_matches[i]).unwrap_or(&[]) == current_key
            {
                i += 1;
            }

            // If we have multiple values for this key, create a disjunction
            if i - start > 1 {
                let mut matches = Vec::with_capacity(i - start);
                for idx in start..i {
                    let offset = object_file
                        .find_data_offset_by_payload(self.current_matches[idx].as_slice())?;

                    let ic = object_file.data_object(offset)?.inlined_cursor();
                    matches.push(FilterExpr::Match(offset, ic));
                }
                elements.push(FilterExpr::Disjunction(matches));
            } else {
                let offset = object_file
                    .find_data_offset_by_payload(self.current_matches[start].as_slice())?;

                let ic = object_file.data_object(offset)?.inlined_cursor();
                elements.push(FilterExpr::Match(offset, ic));
            }
        }

        self.current_matches.clear();

        match elements.len() {
            0 => panic!("Could not create filter elements from current matches"),
            1 => Ok(Some(elements.remove(0))),
            _ => Ok(Some(FilterExpr::Conjunction(elements))),
        }
    }

    pub fn add_match(&mut self, kv_pair: &[u8]) {
        if kv_pair.contains(&b'=') {
            let new_item = kv_pair.to_vec();
            let new_key = Self::extract_key(&new_item).unwrap_or(&[]);

            // Find the insertion position using binary search
            let pos = self
                .current_matches
                .binary_search_by(|item| {
                    let key = Self::extract_key(item).unwrap_or(&[]);
                    key.cmp(new_key)
                })
                .unwrap_or_else(|e| e);

            // Insert at the found position
            self.current_matches.insert(pos, new_item);
        }
    }

    pub fn set_operation<M: MemoryMap>(
        &mut self,
        object_file: &ObjectFile<M>,
        op: LogicalOp,
    ) -> Result<()> {
        let new_expr = self.convert_current_matches(object_file)?;
        if new_expr.is_none() {
            self.current_op = op;
            return Ok(());
        }

        if self.filter_expr.is_none() {
            self.filter_expr = new_expr;
            self.current_op = op;
            return Ok(());
        }

        let new_expr = new_expr.unwrap();
        let current_expr = self.filter_expr.take().unwrap();

        self.filter_expr = Some(match (current_expr, self.current_op) {
            (FilterExpr::Disjunction(mut exprs), LogicalOp::Disjunction) => {
                exprs.push(new_expr);
                FilterExpr::Disjunction(exprs)
            }
            (FilterExpr::Conjunction(mut exprs), LogicalOp::Conjunction) => {
                exprs.push(new_expr);
                FilterExpr::Conjunction(exprs)
            }
            (current_expr, LogicalOp::Disjunction) => {
                FilterExpr::Disjunction(vec![current_expr, new_expr])
            }
            (current_expr, LogicalOp::Conjunction) => {
                FilterExpr::Conjunction(vec![current_expr, new_expr])
            }
        });

        self.current_op = op;
        Ok(())
    }

    pub fn build<M: MemoryMap>(&mut self, object_file: &ObjectFile<M>) -> Result<FilterExpr> {
        self.set_operation(object_file, self.current_op)?;

        self.current_matches.clear();
        self.current_op = LogicalOp::Conjunction;
        self.filter_expr.take().ok_or(JournalError::MalformedFilter)
    }
}
