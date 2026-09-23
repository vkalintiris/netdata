//! Database modes, ported from `src/database/rrd-database-mode.{h,c}` and `align_entries_to_pagesize()`
//! (`src/database/rrd.c`).

/// `RRD_DB_MODE`: the numeric values are persisted in SQLite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DbMode {
    None = 0,
    Ram = 1,
    Alloc = 4,
    Dbengine = 5,
}

/// `RRD_HISTORY_ENTRIES_MAX`.
pub const HISTORY_ENTRIES_MAX: i64 = 86400 * 365;

impl DbMode {
    /// `rrd_memory_mode_id()`: an exact engine name; anything else is ram.
    pub fn from_name(name: &str) -> Self {
        match name {
            "none" => DbMode::None,
            "alloc" => DbMode::Alloc,
            "dbengine" => DbMode::Dbengine,
            _ => DbMode::Ram,
        }
    }

    /// `rrd_memory_mode_name()`.
    pub fn name(self) -> &'static str {
        match self {
            DbMode::None => "none",
            DbMode::Ram => "ram",
            DbMode::Alloc => "alloc",
            DbMode::Dbengine => "dbengine",
        }
    }
}

/// `align_entries_to_pagesize()`: dbengine keeps no ring (0), none keeps 5; otherwise 5..=`HISTORY_ENTRIES_MAX`,
/// and ram rounds the ring up to whole pages of 4-byte slots.
pub fn align_entries_to_pagesize(mode: DbMode, entries: i64, page_size: i64) -> i64 {
    match mode {
        DbMode::Dbengine => 0,
        DbMode::None => 5,
        DbMode::Ram | DbMode::Alloc => {
            let entries = entries.clamp(5, HISTORY_ENTRIES_MAX);
            let size = entries * 4;
            if mode == DbMode::Ram && size % page_size != 0 {
                (size - size % page_size + page_size) / 4
            } else {
                entries
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_alignment() {
        assert_eq!(align_entries_to_pagesize(DbMode::Ram, 3600, 4096), 4096);
        assert_eq!(align_entries_to_pagesize(DbMode::Ram, 1024, 4096), 1024);
        assert_eq!(align_entries_to_pagesize(DbMode::Alloc, 3600, 4096), 3600);
        assert_eq!(align_entries_to_pagesize(DbMode::Alloc, 2, 4096), 5);
        assert_eq!(align_entries_to_pagesize(DbMode::None, 3600, 4096), 5);
        assert_eq!(align_entries_to_pagesize(DbMode::Dbengine, 3600, 4096), 0);
        assert_eq!(DbMode::from_name("save"), DbMode::Ram);
    }
}
