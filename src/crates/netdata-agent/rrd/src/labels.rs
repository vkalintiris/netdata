//! Host and chart labels, ported from `src/database/rrdlabels.{h,c}`: sanitized name/value pairs with a source and
//! the marks `CLABEL`/`CLABEL_COMMIT` and the host `LABEL`/`OVERWRITE` cycles use.
//!
//! C keeps a list keyed by the address of an interned (name, value) pair, so its iteration order is heap order and
//! differs between runs; nothing may depend on it. This port keeps insertion order, a replaced value taking the place
//! of the old one.

use netdata_agent_text::json::JsonWriter;
use netdata_agent_text::sanitize::{rrdlabels_sanitize_name, rrdlabels_sanitize_value};
use netdata_agent_text::simple_pattern::{SimplePattern, SimplePatternResult};

/// `RRDLABEL_SRC_AUTO`: found by automation.
pub const SRC_AUTO: u32 = 1 << 0;
/// `RRDLABEL_SRC_CONFIG`: configured by the user.
pub const SRC_CONFIG: u32 = 1 << 1;
/// `RRDLABEL_SRC_K8S`.
pub const SRC_K8S: u32 = 1 << 2;
/// `RRDLABEL_SRC_ACLK`.
pub const SRC_ACLK: u32 = 1 << 3;
/// `RRDLABEL_FLAG_DONT_DELETE`: never removed by the unmarked sweep (can be overwritten).
pub const FLAG_DONT_DELETE: u32 = 1 << 29;
/// `RRDLABEL_FLAG_OLD`: seen again in the current cycle.
pub const FLAG_OLD: u32 = 1 << 30;
/// `RRDLABEL_FLAG_NEW`: added in the current cycle.
pub const FLAG_NEW: u32 = 1 << 31;
/// `RRDLABEL_FLAG_INTERNAL`.
pub const FLAG_INTERNAL: u32 = FLAG_OLD | FLAG_NEW | FLAG_DONT_DELETE;

/// `RRDLABELS_MAX_NAME_LENGTH` and `RRDLABELS_MAX_VALUE_LENGTH`.
pub const MAX_NAME_LENGTH: usize = 200;
pub const MAX_VALUE_LENGTH: usize = 800;

/// What `rrdlabels_match_simple_pattern_parsed()` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelsMatch {
    Pattern(SimplePatternResult),
    /// With `name=value` matching, a label whose name alone matches stops the walk with C's `-1`, which is not a
    /// positive match.
    NameMatched,
}

impl LabelsMatch {
    pub fn is_positive(self) -> bool {
        self == LabelsMatch::Pattern(SimplePatternResult::MatchedPositive)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub name: Vec<u8>,
    pub value: Vec<u8>,
    /// The `RRDLABEL_SRC_*` bits and the `RRDLABEL_FLAG_*` marks.
    pub flags: u32,
}

/// `RRDLABELS`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Labels {
    labels: Vec<Label>,
    /// `labels->version`: bumped by every change.
    version: u32,
}

impl Labels {
    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn iter(&self) -> impl Iterator<Item = &Label> {
        self.labels.iter()
    }

    pub fn len(&self) -> usize {
        self.labels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    pub fn get(&self, name: &[u8]) -> Option<&[u8]> {
        self.labels
            .iter()
            .find(|l| l.name == name)
            .map(|l| l.value.as_slice())
    }

    /// `rrdlabels_add_changed()`: sanitizes and adds; `Err` carries C's log line when the name sanitizes to nothing.
    /// Returns whether anything changed.
    pub fn add_changed(&mut self, name: &[u8], value: &[u8], source: u32) -> Result<bool, String> {
        let name_sanitized = rrdlabels_sanitize_name(name, MAX_NAME_LENGTH);
        let value_sanitized = rrdlabels_sanitize_value(value, MAX_VALUE_LENGTH);
        if name_sanitized.is_empty() {
            return Err(format!(
                "rrdlabels_add_changed: cannot add name '{}' (value '{}') which is sanitized as empty string",
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(value)
            ));
        }
        Ok(self.add_sanitized(name_sanitized, value_sanitized, source))
    }

    /// `labels_add_already_sanitized()`: the same pair is re-marked OLD; a new pair is marked NEW and replaces the
    /// label with the same name.
    fn add_sanitized(&mut self, name: Vec<u8>, value: Vec<u8>, source: u32) -> bool {
        let flags = source & !(FLAG_NEW | FLAG_OLD);
        let changed = if let Some(same) = self
            .labels
            .iter_mut()
            .find(|l| l.name == name && l.value == value)
        {
            let old = same.flags;
            same.flags = flags | FLAG_OLD;
            (old & !FLAG_INTERNAL) != (same.flags & !FLAG_INTERNAL)
        } else {
            let label = Label {
                name,
                value,
                flags: flags | FLAG_NEW,
            };
            match self.labels.iter_mut().find(|l| l.name == label.name) {
                Some(slot) => *slot = label,
                None => self.labels.push(label),
            }
            true
        };
        if changed {
            self.version = self.version.wrapping_add(1);
        }
        changed
    }

    /// `rrdlabels_add()`.
    pub fn add(&mut self, name: &[u8], value: &[u8], source: u32) {
        let _ = self.add_changed(name, value, source);
    }

    /// `rrdlabels_unmark_all()`.
    pub fn unmark_all(&mut self) {
        for label in &mut self.labels {
            label.flags &= !(FLAG_OLD | FLAG_NEW);
        }
    }

    /// `rrdlabels_remove_all_unmarked()`: labels neither seen in this cycle nor protected go.
    pub fn remove_all_unmarked(&mut self) -> usize {
        let before = self.labels.len();
        self.labels.retain(|l| l.flags & FLAG_INTERNAL != 0);
        before - self.labels.len()
    }

    /// `rrdlabels_migrate_to_these()`: this set becomes `src` (pairs kept, added or replaced), protected labels stay;
    /// the version follows `src`. Whether anything changed.
    pub fn migrate_to_these(&mut self, src: &Labels) -> bool {
        self.unmark_all();
        let mut added = 0;
        let mut cleaned = 0;
        for label in &src.labels {
            if let Some(same) = self
                .labels
                .iter_mut()
                .find(|l| l.name == label.name && l.value == label.value)
            {
                same.flags |= FLAG_OLD;
                continue;
            }
            added += 1;
            let new = Label {
                name: label.name.clone(),
                value: label.value.clone(),
                flags: (label.flags & !(FLAG_OLD | FLAG_NEW)) | FLAG_NEW,
            };
            let before = self.labels.len();
            self.labels.retain(|l| l.name != new.name);
            cleaned += before - self.labels.len();
            self.labels.push(new);
        }
        let removed = self.remove_all_unmarked();
        self.version = src.version;
        added > 0 || removed > 0 || cleaned > 0
    }

    /// `rrdlabels_match_simple_pattern_parsed()`: the first label that matches decides. With `equal` 0 the names are
    /// matched (a negative match counts as none); otherwise `name<equal>value`, after the name alone.
    pub fn match_simple_pattern_parsed(&self, pattern: &SimplePattern, equal: u8) -> LabelsMatch {
        for label in &self.labels {
            let result = if equal == 0 {
                match pattern.matches_extract(&label.name, 0).0 {
                    SimplePatternResult::MatchedNegative => SimplePatternResult::NotMatched,
                    other => other,
                }
            } else {
                if pattern.matches(&label.name) {
                    return LabelsMatch::NameMatched;
                }
                let mut pair = label.name.clone();
                pair.push(equal);
                pair.extend_from_slice(&label.value);
                pattern.matches_extract(&pair, 0).0
            };
            if result != SimplePatternResult::NotMatched {
                return LabelsMatch::Pattern(result);
            }
        }
        LabelsMatch::Pattern(SimplePatternResult::NotMatched)
    }

    /// `rrdlabels_to_buffer_json_members()`.
    pub fn to_json_members(&self, w: &mut JsonWriter) {
        for label in &self.labels {
            w.member_add_string(&label.name, &label.value);
        }
    }

    /// `rrdlabels_exist()`.
    pub fn exists(&self, name: &[u8]) -> bool {
        self.labels.iter().any(|l| l.name == name)
    }

    /// `rrdlabels_remove_all_unmarked_and_changed()`: whether the cycle added or removed anything.
    pub fn remove_all_unmarked_and_changed(&mut self) -> bool {
        let added = self
            .labels
            .iter()
            .filter(|l| l.flags & FLAG_NEW != 0)
            .count();
        let removed = self.remove_all_unmarked();
        added > 0 || removed > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clabel_cycle_replaces_the_set() {
        let mut labels = Labels::default();
        labels.add(b"_collect_plugin", b"go.d", SRC_AUTO | FLAG_DONT_DELETE);
        assert_eq!(labels.add_changed(b"a", b"1", SRC_CONFIG), Ok(true));
        assert_eq!(labels.add_changed(b"b", b"2", SRC_CONFIG), Ok(true));
        assert!(labels.remove_all_unmarked_and_changed());
        // The next cycle: `a` again, `b` gone, `c` new, `a`'s value unchanged.
        labels.unmark_all();
        assert_eq!(labels.add_changed(b"a", b"1", SRC_CONFIG), Ok(false));
        assert_eq!(labels.add_changed(b"c", b"3", SRC_CONFIG), Ok(true));
        assert!(labels.remove_all_unmarked_and_changed());
        let names: Vec<_> = labels.iter().map(|l| l.name.as_slice()).collect();
        assert_eq!(names, [&b"_collect_plugin"[..], b"a", b"c"]);
        // A new value for a name replaces it; the same pair again changes nothing.
        labels.unmark_all();
        assert_eq!(labels.add_changed(b"a", b"9", SRC_CONFIG), Ok(true));
        assert_eq!(labels.get(b"a"), Some(&b"9"[..]));
        assert_eq!(labels.len(), 3);
        labels.unmark_all();
        assert_eq!(labels.add_changed(b"c", b"3", SRC_CONFIG), Ok(false));
        assert!(labels.remove_all_unmarked_and_changed());
        assert_eq!(labels.len(), 2);
        assert!(labels.add_changed(b"", b"x", SRC_CONFIG).is_err());
    }
}
