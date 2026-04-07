use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::FileId;

/// A directory handle for files with a specific extension.
///
/// Provides path derivation and directory scanning. The extension determines
/// which files are recognized during scans (e.g. `"wal"`, `"sfst"`).
#[derive(Clone)]
pub struct FileDir {
    path: PathBuf,
    ext: &'static str,
}

impl FileDir {
    pub fn new(path: &Path, ext: &'static str) -> Self {
        Self {
            path: path.to_path_buf(),
            ext,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn ext(&self) -> &str {
        self.ext
    }

    /// Derive the on-disk path for a file from its [`FileId`].
    pub fn file_path(&self, id: FileId) -> PathBuf {
        self.path.join(id.to_filename(self.ext))
    }

    /// Parse a path into a [`FileId`], if it matches the given extension.
    pub fn parse(path: &Path, ext: &str) -> Option<FileId> {
        let name = path.file_name()?.to_str()?;
        let stem = name.strip_suffix(&format!(".{ext}"))?;
        FileId::parse_stem(stem)
    }

    /// Scan the directory for files matching this extension.
    ///
    /// Returns `(FileId, Metadata)` pairs for all parseable files.
    /// Unparseable filenames are logged as warnings and skipped.
    pub fn scan(&self) -> io::Result<Vec<(FileId, fs::Metadata)>> {
        let entries = match fs::read_dir(&self.path) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };

        let mut result = Vec::new();

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(
                        directory = %self.path.display(),
                        error = %e,
                        "failed to read directory entry"
                    );
                    continue;
                }
            };

            let path = entry.path();

            let Some(id) = Self::parse(&path, self.ext) else {
                tracing::warn!(
                    directory = %self.path.display(),
                    file = %path.display(),
                    "skipping file with unparseable name"
                );
                continue;
            };

            let meta = match fs::metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(
                        directory = %self.path.display(),
                        file = %path.display(),
                        error = %e,
                        "failed to stat file"
                    );
                    continue;
                }
            };

            result.push((id, meta));
        }

        Ok(result)
    }

    /// Scan the directory for the highest existing sequence number.
    ///
    /// The sequence number is monotonically increasing across boots, so all
    /// files in the directory are considered regardless of their origin.
    pub fn scan_max_sequence(&self) -> io::Result<u64> {
        let entries = self.scan()?;
        Ok(entries.iter().map(|(id, _)| id.seq).max().unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn test_machine_id() -> Uuid {
        Uuid::try_parse("550e8400e29b41d4a716446655440000").unwrap()
    }

    fn test_boot_id() -> Uuid {
        Uuid::try_parse("7f3b2a1e9c4d4f8ab1c2d3e4f5a6b7c8").unwrap()
    }

    #[test]
    fn file_path_derivation() {
        let dir = FileDir::new(Path::new("/tmp/wal"), "wal");
        let id = FileId::new(test_machine_id(), test_boot_id(), 1, 0);
        let path = dir.file_path(id);
        assert!(path.to_str().unwrap().ends_with(".wal"));
        assert!(path.starts_with("/tmp/wal"));
    }

    #[test]
    fn parse_matching_extension() {
        let id = FileId::new(test_machine_id(), test_boot_id(), 42, 0);
        let filename = id.to_filename("sfst");
        let path = Path::new(&filename);

        assert!(FileDir::parse(path, "sfst").is_some());
        assert!(FileDir::parse(path, "wal").is_none());
    }

    #[test]
    fn scan_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let fd = FileDir::new(dir.path(), "wal");
        let entries = fd.scan().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn scan_nonexistent_directory() {
        let fd = FileDir::new(Path::new("/tmp/nonexistent-dir-test"), "wal");
        let entries = fd.scan().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn scan_max_sequence_empty() {
        let dir = tempfile::tempdir().unwrap();
        let fd = FileDir::new(dir.path(), "wal");
        assert_eq!(fd.scan_max_sequence().unwrap(), 0);
    }
}
