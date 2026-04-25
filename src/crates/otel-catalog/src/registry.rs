//! Registry for catalog files on local disk.
//!
//! Catalog files are immutable snapshots produced by a `CatalogBuilder`
//! whenever a per-scope accumulator is rotated. Each file is named
//! `{machine_id}-{boot_id}-{max_seq}.catalog` and lives under a
//! date-partitioned directory: `{base}/{YYYY-MM-DD}/{name}.catalog`.
//!
//! The registry tracks locally-present catalog files, mirrors the API
//! shape of [`sfst::Registry`], and is consulted by retention
//! and by query-time discovery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::NaiveDate;
use file_registry::{ByteSize, TenantId, TimestampNs};
use uuid::Uuid;

const CATALOG_EXT: &str = "catalog";

/// One catalog file present on disk.
#[derive(Debug, Clone)]
pub struct File {
    pub date: NaiveDate,
    pub machine_id: Uuid,
    pub boot_id: Uuid,
    /// Highest SFST sequence number contained in this catalog.
    pub max_seq: u64,
    pub created_at_ns: TimestampNs,
    pub size: ByteSize,
    pending_deletion: bool,
}

impl File {
    /// Build a new `File` entry with `pending_deletion = false`. Used by the
    /// ledger when a new catalog file is written.
    pub fn new(
        date: NaiveDate,
        machine_id: Uuid,
        boot_id: Uuid,
        max_seq: u64,
        created_at_ns: TimestampNs,
        size: ByteSize,
    ) -> Self {
        Self {
            date,
            machine_id,
            boot_id,
            max_seq,
            created_at_ns,
            size,
            pending_deletion: false,
        }
    }

    pub fn is_pending_deletion(&self) -> bool {
        self.pending_deletion
    }
}

pub struct Registry {
    /// Shared base directory (typically `logs_config.catalog.dir`). Per-tenant
    /// catalog files live under `{base_dir}/{date}/{tenant_id}/` — matching
    /// the flat-per-tenant convention used for WAL and SFST files. The
    /// remote key layout adds a `catalog/` segment to discriminate artifact
    /// types inside the shared bucket.
    base_dir: PathBuf,
    /// The tenant this `Registry` owns. Recovery filters to this tenant.
    tenant_id: TenantId,
    /// Keyed by on-disk path. Catalog files are identified by their full
    /// `(date, machine, boot, max_seq)` tuple which the path encodes.
    files: BTreeMap<PathBuf, File>,
}

impl Registry {
    pub fn new(base_dir: &Path, tenant_id: TenantId) -> Self {
        Self {
            base_dir: base_dir.to_path_buf(),
            tenant_id,
            files: BTreeMap::new(),
        }
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    /// Derive the canonical on-disk path for a catalog file.
    pub fn file_path(
        &self,
        date: NaiveDate,
        machine_id: Uuid,
        boot_id: Uuid,
        max_seq: u64,
    ) -> PathBuf {
        self.base_dir
            .join(date.format("%Y-%m-%d").to_string())
            .join(self.tenant_id.as_str())
            .join(filename(machine_id, boot_id, max_seq))
    }

    /// Register a catalog file that has been written to disk.
    pub fn track(&mut self, file: File, path: PathBuf) {
        self.files.insert(path, file);
    }

    pub fn remove(&mut self, path: &Path) -> Option<File> {
        self.files.remove(path)
    }

    pub fn get(&self, path: &Path) -> Option<&File> {
        self.files.get(path)
    }

    pub fn values(&self) -> impl Iterator<Item = &File> {
        self.files.values()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PathBuf, &File)> {
        self.files.iter()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn mark_pending_deletion(&mut self, path: &Path) {
        if let Some(entry) = self.files.get_mut(path) {
            entry.pending_deletion = true;
        }
    }

    /// Clear the `pending_deletion` flag on `path` if it's tracked.
    /// Returns `true` if `path` was found (the flag may or may not have
    /// been set), `false` if the path isn't tracked at all — so callers
    /// iterating per-tenant registries can stop on the first match.
    pub fn clear_pending_deletion(&mut self, path: &Path) -> bool {
        if let Some(entry) = self.files.get_mut(path) {
            entry.pending_deletion = false;
            true
        } else {
            false
        }
    }

    /// Return paths of catalog files whose date is strictly older than
    /// `today - max_days`. Files already `pending_deletion` are excluded
    /// to avoid double-scheduling. Does not mutate retention state — the
    /// caller is expected to `mark_pending_deletion` on each returned
    /// path before dispatching the delete.
    pub fn evaluate_retention(&self, max_days: u32, today: NaiveDate) -> Vec<PathBuf> {
        let cutoff = match today.checked_sub_signed(chrono::Duration::days(max_days as i64)) {
            Some(d) => d,
            None => return Vec::new(),
        };
        self.files
            .iter()
            .filter(|(_, f)| !f.pending_deletion && f.date < cutoff)
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// Scan `{base_dir}/{date}/{tenant_id}/*.catalog` and reconstruct
    /// registry state from disk. Only files belonging to this `Registry`'s
    /// tenant are loaded; other tenants' subdirs under the same date are
    /// skipped.
    ///
    /// Files with unparseable names are logged and skipped. Date subdirectories
    /// that don't parse as `YYYY-MM-DD` are logged and skipped.
    pub fn recover(&mut self) {
        let date_entries = match std::fs::read_dir(&self.base_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!(
                    dir = %self.base_dir.display(),
                    "failed to read catalog base dir: {e}"
                );
                return;
            }
        };

        for date_entry in date_entries.flatten() {
            let date_path = date_entry.path();
            if !date_entry
                .file_type()
                .map(|ft| ft.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let date_str = match date_entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let date = match NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
                Ok(d) => d,
                Err(_) => continue, // non-date subdirs (e.g. per-tenant sfst dirs) live here too
            };

            let tenant_catalog_dir = date_path.join(self.tenant_id.as_str());
            let files = match std::fs::read_dir(&tenant_catalog_dir) {
                Ok(e) => e,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    tracing::warn!(
                        path = %tenant_catalog_dir.display(),
                        "failed to read tenant catalog dir: {e}"
                    );
                    continue;
                }
            };

            for entry in files.flatten() {
                let path = entry.path();
                let name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    None => continue,
                };
                let stem = match name.strip_suffix(&format!(".{CATALOG_EXT}")) {
                    Some(s) => s,
                    None => continue,
                };
                let (machine_id, boot_id, max_seq) = match parse_stem(stem) {
                    Some(v) => v,
                    None => {
                        tracing::warn!(
                            file = %path.display(),
                            "skipping catalog file with unparseable name"
                        );
                        continue;
                    }
                };
                let meta = match std::fs::metadata(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(
                            file = %path.display(),
                            "failed to stat catalog file: {e}"
                        );
                        continue;
                    }
                };
                let size = ByteSize(meta.len());
                let created_at_ns = TimestampNs(
                    meta.modified()
                        .unwrap_or(SystemTime::UNIX_EPOCH)
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as u64,
                );
                self.files.insert(
                    path,
                    File {
                        date,
                        machine_id,
                        boot_id,
                        max_seq,
                        created_at_ns,
                        size,
                        pending_deletion: false,
                    },
                );
            }
        }
    }
}

/// Format a catalog filename: `{machine:32}-{boot:32}-{max_seq:010}.catalog`.
pub fn filename(machine_id: Uuid, boot_id: Uuid, max_seq: u64) -> String {
    format!(
        "{}-{}-{:010}.{CATALOG_EXT}",
        machine_id.as_simple(),
        boot_id.as_simple(),
        max_seq,
    )
}

/// Parse the stem `{machine:32}-{boot:32}-{max_seq}` into its components.
pub fn parse_stem(stem: &str) -> Option<(Uuid, Uuid, u64)> {
    // machine_id: 32 hex chars, boot_id: 32 hex chars, max_seq: decimal.
    if stem.len() < 32 + 1 + 32 + 1 + 1 {
        return None;
    }
    let machine_str = &stem[..32];
    if stem.as_bytes().get(32)? != &b'-' {
        return None;
    }
    let boot_str = &stem[33..65];
    if stem.as_bytes().get(65)? != &b'-' {
        return None;
    }
    let max_seq_str = &stem[66..];

    let machine_id = Uuid::try_parse(machine_str).ok()?;
    let boot_id = Uuid::try_parse(boot_str).ok()?;
    let max_seq: u64 = max_seq_str.parse().ok()?;
    Some((machine_id, boot_id, max_seq))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> Uuid {
        Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff)
    }

    fn boot() -> Uuid {
        Uuid::from_u128(0xaaaa_bbbb_cccc_dddd_eeee_ffff_0000_1111)
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 17).unwrap()
    }

    #[test]
    fn filename_and_parse_roundtrip() {
        let name = filename(machine(), boot(), 42);
        assert!(name.ends_with(".catalog"));
        let stem = name.strip_suffix(".catalog").unwrap();
        let (m, b, s) = parse_stem(stem).unwrap();
        assert_eq!(m, machine());
        assert_eq!(b, boot());
        assert_eq!(s, 42);
    }

    #[test]
    fn parse_stem_rejects_unknown_shapes() {
        assert!(parse_stem("").is_none());
        assert!(parse_stem("not-a-uuid").is_none());
        assert!(parse_stem(&format!("{}-not-a-uuid-1", machine().as_simple())).is_none());
    }

    const TENANT: &str = "tenant1";

    fn write_catalog_at(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"{}").unwrap();
    }

    #[test]
    fn file_path_is_base_date_tenant_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = Registry::new(tmp.path(), TenantId::from(TENANT));
        let p = reg.file_path(date(), machine(), boot(), 7);
        assert!(p.starts_with(tmp.path()));
        let s = p.to_str().unwrap();
        assert!(s.contains("2026-04-17"));
        assert!(s.contains(&format!("/{TENANT}/")));
        assert!(!s.contains("/catalog/"), "no catalog/ subdir locally");
        assert!(s.ends_with(".catalog"));
    }

    #[test]
    fn track_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(tmp.path(), TenantId::from(TENANT));
        let path = reg.file_path(date(), machine(), boot(), 10);
        let file = File {
            date: date(),
            machine_id: machine(),
            boot_id: boot(),
            max_seq: 10,
            created_at_ns: TimestampNs(0),
            size: ByteSize(1024),
            pending_deletion: false,
        };
        reg.track(file, path.clone());
        assert_eq!(reg.len(), 1);
        assert!(reg.get(&path).is_some());

        let removed = reg.remove(&path).unwrap();
        assert_eq!(removed.max_seq, 10);
        assert!(reg.is_empty());
    }

    #[test]
    fn pending_deletion_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(tmp.path(), TenantId::from(TENANT));
        let path = reg.file_path(date(), machine(), boot(), 1);
        reg.track(
            File {
                date: date(),
                machine_id: machine(),
                boot_id: boot(),
                max_seq: 1,
                created_at_ns: TimestampNs(0),
                size: ByteSize(1),
                pending_deletion: false,
            },
            path.clone(),
        );
        assert!(!reg.get(&path).unwrap().is_pending_deletion());
        reg.mark_pending_deletion(&path);
        assert!(reg.get(&path).unwrap().is_pending_deletion());
        reg.clear_pending_deletion(&path);
        assert!(!reg.get(&path).unwrap().is_pending_deletion());
    }

    #[test]
    fn recover_picks_up_files_written_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let expected =
            tmp.path()
                .join("2026-04-17")
                .join(TENANT)
                .join(filename(machine(), boot(), 42));
        write_catalog_at(&expected);

        let mut reg = Registry::new(tmp.path(), TenantId::from(TENANT));
        reg.recover();

        assert_eq!(reg.len(), 1);
        let entry = reg.get(&expected).unwrap();
        assert_eq!(entry.max_seq, 42);
        assert_eq!(entry.date, date());
    }

    #[test]
    fn recover_filters_to_this_tenant() {
        let tmp = tempfile::tempdir().unwrap();
        // Same date, two tenants.
        write_catalog_at(&tmp.path().join("2026-04-17").join(TENANT).join(filename(
            machine(),
            boot(),
            1,
        )));
        write_catalog_at(
            &tmp.path()
                .join("2026-04-17")
                .join("other-tenant")
                .join(filename(machine(), boot(), 2)),
        );

        let mut reg = Registry::new(tmp.path(), TenantId::from(TENANT));
        reg.recover();

        assert_eq!(reg.len(), 1, "must not load other tenants' catalogs");
        assert_eq!(reg.values().next().unwrap().max_seq, 1);
    }

    #[test]
    fn recover_skips_non_date_subdirs_and_unparseable_names() {
        let tmp = tempfile::tempdir().unwrap();
        // Non-date top-level subdir: ignored.
        std::fs::create_dir_all(tmp.path().join("not-a-date")).unwrap();
        // Date subdir with garbage-named catalog file.
        write_catalog_at(
            &tmp.path()
                .join("2026-04-17")
                .join(TENANT)
                .join("garbage-name.catalog"),
        );

        let mut reg = Registry::new(tmp.path(), TenantId::from(TENANT));
        reg.recover();

        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn recover_nonexistent_base_dir_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("no-such-dir");
        let mut reg = Registry::new(&missing, TenantId::from(TENANT));
        reg.recover();
        assert!(reg.is_empty());
    }

    fn track_at(reg: &mut Registry, d: NaiveDate, max_seq: u64) -> PathBuf {
        let path = reg.file_path(d, machine(), boot(), max_seq);
        reg.track(
            File::new(
                d,
                machine(),
                boot(),
                max_seq,
                TimestampNs(0),
                ByteSize(1024),
            ),
            path.clone(),
        );
        path
    }

    #[test]
    fn evaluate_retention_evicts_files_older_than_cutoff() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(tmp.path(), TenantId::from(TENANT));

        let today = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let d_old = today - chrono::Duration::days(10);
        let d_boundary = today - chrono::Duration::days(7);
        let d_fresh = today - chrono::Duration::days(3);

        let p_old = track_at(&mut reg, d_old, 1);
        let _p_boundary = track_at(&mut reg, d_boundary, 2);
        let _p_fresh = track_at(&mut reg, d_fresh, 3);

        // max_days = 7 → cutoff = today - 7 days = d_boundary. Strictly
        // older means d_old only; the file dated exactly on the cutoff
        // (d_boundary) is kept.
        let evicted = reg.evaluate_retention(7, today);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0], p_old);
    }

    #[test]
    fn evaluate_retention_excludes_pending_deletion() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(tmp.path(), TenantId::from(TENANT));

        let today = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let d_old = today - chrono::Duration::days(30);

        let p = track_at(&mut reg, d_old, 1);
        reg.mark_pending_deletion(&p);

        let evicted = reg.evaluate_retention(7, today);
        assert!(
            evicted.is_empty(),
            "pending_deletion entries must be skipped"
        );
    }

    #[test]
    fn evaluate_retention_with_huge_max_days_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(tmp.path(), TenantId::from(TENANT));

        let today = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        track_at(&mut reg, today - chrono::Duration::days(1000), 1);

        // max_days so large that cutoff underflows → eviction list empty.
        let evicted = reg.evaluate_retention(u32::MAX, today);
        assert!(evicted.is_empty());
    }
}
