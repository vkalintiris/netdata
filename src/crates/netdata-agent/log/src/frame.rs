//! The per-thread stack of field frames (`ND_LOG_STACK_PUSH`, `log_stack_push()` / `log_stack_pop()` in
//! `src/libnetdata/log/nd_log-internals.c`): every record logged while a frame is pushed carries its fields.

use std::cell::RefCell;
use std::rc::Rc;

use crate::model::Field;

/// `THREAD_LOG_STACK_MAX`: frames pushed beyond this depth are dropped (their fields are not applied).
const STACK_MAX: usize = 50;

/// A lazily formatted value (`NDFT_CALLBACK`): it appends its text and returns false when it has none. It runs while
/// a record is being written, so it must not log or push frames.
pub type Lazy = Rc<dyn Fn(&mut Vec<u8>) -> bool>;

/// A field value in a frame (`struct log_stack_entry`).
#[derive(Clone)]
pub enum Value {
    /// `NDFT_TXT` / `NDFT_BFR`: an empty text is not applied.
    Txt(String),
    /// `NDFT_STR` (a `STRING *`): applied even when empty; logfmt then writes `key=""`.
    Str(String),
    U64(u64),
    I64(i64),
    Dbl(f64),
    /// `NDFT_UUID`: an all-zero UUID is not applied.
    Uuid([u8; 16]),
    /// `NDFT_CALLBACK`.
    Lazy(Lazy),
}

impl Value {
    pub fn txt(text: impl Into<String>) -> Value {
        Value::Txt(text.into())
    }

    pub fn lazy(f: impl Fn(&mut Vec<u8>) -> bool + 'static) -> Value {
        Value::Lazy(Rc::new(f))
    }

    /// `nd_logger_merge_log_stack_to_thread_fields()`: empty texts and zero UUIDs leave the field unset.
    fn applies(&self) -> bool {
        match self {
            Value::Txt(text) => !text.is_empty(),
            Value::Uuid(uuid) => uuid.iter().any(|&b| b != 0),
            _ => true,
        }
    }
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Txt(v) => f.debug_tuple("Txt").field(v).finish(),
            Value::Str(v) => f.debug_tuple("Str").field(v).finish(),
            Value::U64(v) => f.debug_tuple("U64").field(v).finish(),
            Value::I64(v) => f.debug_tuple("I64").field(v).finish(),
            Value::Dbl(v) => f.debug_tuple("Dbl").field(v).finish(),
            Value::Uuid(v) => f.debug_tuple("Uuid").field(v).finish(),
            Value::Lazy(_) => f.write_str("Lazy"),
        }
    }
}

struct Stack {
    frames: Vec<(u64, Vec<(Field, Value)>)>,
    next_id: u64,
}

thread_local! {
    static STACK: RefCell<Stack> = const {
        RefCell::new(Stack {
            frames: Vec::new(),
            next_id: 0,
        })
    };
}

/// Pops its frame when dropped.
#[must_use = "the frame is popped when the guard is dropped"]
pub struct FrameGuard {
    /// `None` when the push was dropped because the stack was full.
    id: Option<u64>,
    /// Frames belong to their thread.
    _not_send: std::marker::PhantomData<Rc<()>>,
}

/// Pushes a frame of fields onto this thread's stack until the guard is dropped.
pub fn push(fields: Vec<(Field, Value)>) -> FrameGuard {
    let id = STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if stack.frames.len() >= STACK_MAX {
            return None;
        }
        let id = stack.next_id;
        stack.next_id += 1;
        stack.frames.push((id, fields));
        Some(id)
    });
    FrameGuard {
        id,
        _not_send: std::marker::PhantomData,
    }
}

impl Drop for FrameGuard {
    fn drop(&mut self) {
        let Some(id) = self.id else {
            return;
        };
        STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            // Guards drop in LIFO order unless one is moved out of its scope; remove it wherever it is.
            if let Some(at) = stack.frames.iter().rposition(|(frame, _)| *frame == id) {
                stack.frames.remove(at);
            }
        });
    }
}

/// Calls `f` with the fields the stack sets, oldest frame first, entries in order, so a later entry for the same
/// field wins; fields that do not apply are skipped.
pub(crate) fn with_fields<R>(f: impl FnOnce(&[Option<&Value>; crate::model::FIELDS]) -> R) -> R {
    STACK.with(|stack| {
        let stack = stack.borrow();
        let mut slots: [Option<&Value>; crate::model::FIELDS] = [None; crate::model::FIELDS];
        for (_, frame) in &stack.frames {
            for (field, value) in frame {
                if value.applies() {
                    slots[*field as usize] = Some(value);
                }
            }
        }
        f(&slots)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message() -> Option<String> {
        with_fields(|slots| match slots[Field::Message as usize] {
            Some(Value::Txt(text)) => Some(text.clone()),
            _ => None,
        })
    }

    #[test]
    fn later_frames_win_and_empty_values_do_not_apply() {
        let _a = push(vec![(Field::Message, Value::txt("outer"))]);
        {
            let _b = push(vec![(Field::Message, Value::txt("inner"))]);
            assert_eq!(message().as_deref(), Some("inner"));
            let _c = push(vec![(Field::Message, Value::txt(""))]);
            assert_eq!(message().as_deref(), Some("inner"));
        }
        assert_eq!(message().as_deref(), Some("outer"));
    }

    #[test]
    fn pushes_beyond_the_maximum_depth_are_dropped() {
        let guards: Vec<_> = (0..STACK_MAX)
            .map(|i| push(vec![(Field::Message, Value::txt(i.to_string()))]))
            .collect();
        {
            let _over = push(vec![(Field::Message, Value::txt("over"))]);
            assert_eq!(message().as_deref(), Some("49"));
        }
        assert_eq!(message().as_deref(), Some("49"));
        drop(guards);
        assert_eq!(message(), None);
    }
}
