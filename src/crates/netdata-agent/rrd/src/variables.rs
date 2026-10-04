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

    /// `rrdvar_get_custom_*_variable_value()`: by the name as it was stored (sanitized).
    pub(crate) fn get(&self, name: &[u8]) -> Option<f64> {
        let variables = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        variables.iter().find(|(known, _)| known.as_bytes() == name).map(|&(_, value)| value)
    }

    pub(crate) fn all(&self) -> Vec<(String, f64)> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(crate) fn clear(&self) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_variable_is_found_by_the_name_it_was_stored_under() {
        let variables = Variables::default();
        let stored = variables.put("my var!", 7.0);
        assert_ne!(stored, "my var!", "the name is sanitized");
        assert_eq!(variables.get(stored.as_bytes()), Some(7.0));
        assert_eq!(variables.get(b"my var!"), None);
        assert_eq!(variables.get(b"nope"), None);
        variables.put("my var!", 8.0);
        assert_eq!(variables.get(stored.as_bytes()), Some(8.0));
    }
}
