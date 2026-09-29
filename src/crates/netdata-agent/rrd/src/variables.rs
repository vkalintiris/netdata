//! `host->rrdvars` and `st->rrdvars` (`src/health/rrdvar.c`): the custom variables, in insertion order, their names
//! sanitized as C does.

use std::sync::{Mutex, PoisonError};

use netdata_agent_text::sanitize::rrdvar_fix_name;

#[derive(Debug, Default)]
pub(crate) struct Variables(Mutex<Vec<(String, f64)>>);

impl Variables {
    /// A VARIABLE line: `rrdvar_*_variable_add_and_acquire()`, then `rrdvar_*_variable_set()`. The add stores NaN,
    /// over an existing value too (the dictionary's conflict callback), so the set always counts as a change. The
    /// sanitized name is returned.
    pub(crate) fn put(&self, name: &str, value: f64) -> String {
        let fixed = String::from_utf8_lossy(&rrdvar_fix_name(name.as_bytes()).0).into_owned();
        let mut variables = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match variables.iter_mut().find(|(n, _)| *n == fixed) {
            Some((_, v)) => *v = value,
            None => variables.push((fixed.clone(), value)),
        }
        fixed
    }

    pub(crate) fn all(&self) -> Vec<(String, f64)> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(crate) fn clear(&self) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
}
