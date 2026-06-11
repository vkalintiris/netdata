//! The date-partitioned per-tenant directory layout:
//! `{base}/{YYYY-MM-DD}/{tenant}/<files>`.
//!
//! Scattered copies of this layout's walk and path-build are how
//! layout bugs happen (a scanner that doesn't know a layout exists
//! can't bound a counter seeded from it), so the structure lives here;
//! per-file policy (which files to read, how to react to read errors)
//! stays with the callers. The flat per-tenant layout
//! (`{base}/{tenant}/<files>`) is owned by [`FileDir`](crate::FileDir)
//! and [`scan_max_sequence_recursive`](crate::scan_max_sequence_recursive).

use std::io;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

/// The directory name for `date` (`YYYY-MM-DD`).
pub fn date_dir_name(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// Parse a directory name as a layout date partition.
pub fn parse_date_dir(name: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(name, "%Y-%m-%d").ok()
}

/// The directory holding `tenant`'s files for `date`.
pub fn date_tenant_dir(base: &Path, date: NaiveDate, tenant: &str) -> PathBuf {
    base.join(date_dir_name(date)).join(tenant)
}

/// One `{date}/{tenant}` partition found on disk.
#[derive(Debug, Clone)]
pub struct DateTenantDir {
    pub date: NaiveDate,
    pub tenant: String,
    pub path: PathBuf,
}

/// Enumerate every `{date}/{tenant}` partition under `base`.
///
/// Structural policy only: a missing `base` yields an empty list,
/// non-directory entries and non-date directory names are skipped
/// (other artifact types may share the base), and tenant names that
/// aren't valid UTF-8 are skipped. Hard I/O errors propagate — callers
/// with a softer policy (recovery walks) wrap the result themselves.
pub fn date_tenant_dirs(base: &Path) -> io::Result<Vec<DateTenantDir>> {
    let mut out = Vec::new();
    let date_entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for date_entry in date_entries.flatten() {
        if !date_entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }
        let Some(date) = date_entry.file_name().to_str().and_then(parse_date_dir) else {
            continue;
        };
        let tenant_entries = match std::fs::read_dir(date_entry.path()) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for tenant_entry in tenant_entries.flatten() {
            if !tenant_entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                continue;
            }
            let Some(tenant) = tenant_entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            out.push(DateTenantDir {
                date,
                tenant,
                path: tenant_entry.path(),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_partitions_and_skips_foreign_entries() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("2026-06-11").join("tenant-a")).unwrap();
        std::fs::create_dir_all(tmp.path().join("2026-06-11").join("tenant-b")).unwrap();
        std::fs::create_dir_all(tmp.path().join("2026-06-12").join("tenant-a")).unwrap();
        // Foreign entries: non-date dir, plain file, file inside a date dir.
        std::fs::create_dir_all(tmp.path().join("not-a-date")).unwrap();
        std::fs::write(tmp.path().join("stray.bin"), b"x").unwrap();
        std::fs::write(tmp.path().join("2026-06-12").join("stray"), b"x").unwrap();

        let mut got: Vec<(String, String)> = date_tenant_dirs(tmp.path())
            .unwrap()
            .into_iter()
            .map(|p| (date_dir_name(p.date), p.tenant))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("2026-06-11".into(), "tenant-a".into()),
                ("2026-06-11".into(), "tenant-b".into()),
                ("2026-06-12".into(), "tenant-a".into()),
            ]
        );

        // Missing base: empty, not an error.
        assert!(date_tenant_dirs(&tmp.path().join("nope")).unwrap().is_empty());
    }

    #[test]
    fn path_build_matches_walk() {
        let date = parse_date_dir("2026-06-11").unwrap();
        assert_eq!(
            date_tenant_dir(Path::new("/b"), date, "t"),
            Path::new("/b/2026-06-11/t")
        );
    }
}
