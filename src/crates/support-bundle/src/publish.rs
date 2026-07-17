//! Staging, archiving, and no-clobber publishing. The bundle is built inside
//! a private 0700 staging directory with an unpredictable name, then the
//! final artifacts are published with O_EXCL (create_new) so a pre-existing
//! file OR symlink planted in a shared tmp dir can never be followed or
//! overwritten — there is no check/open TOCTOU window.

use crate::sanitize::MapRow;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Staging {
    pub dir: PathBuf,
    keep: bool,
}

impl Staging {
    /// Create the private staging dir under the system temp dir (0700 on
    /// unix, per-user %TEMP% on Windows), with a random name.
    pub fn create(keep: bool) -> std::io::Result<Staging> {
        let base = std::env::temp_dir();
        let dir = tempfile::Builder::new()
            .prefix("netdata-support-bundle.")
            .tempdir_in(base)?
            .keep();
        Ok(Staging { dir, keep })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

fn create_new_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// Publish `content_path` at `target` refusing to touch any pre-existing
/// file or symlink.
fn publish_no_clobber(content_path: &Path, target: &Path) -> std::io::Result<()> {
    // source first: failing to open it must not leave an empty target behind
    let mut src = std::fs::File::open(content_path)?;
    let mut dst = create_new_private(target)?;
    if let Err(e) = std::io::copy(&mut src, &mut dst) {
        // a partial artifact would block every later run behind O_EXCL
        drop(dst);
        let _ = std::fs::remove_file(target);
        return Err(e);
    }
    Ok(())
}

#[cfg(unix)]
pub fn build_archive(staging: &Path, work: &Path, bundle_name: &str) -> std::io::Result<PathBuf> {
    // zstd, per maintainer preference (smaller and faster than gzip on this
    // volume of text); the encoder is compiled in, so there is no
    // tool-availability fallback to gzip
    let archive_path = staging.join("bundle.tar.zst");
    let file = create_new_private(&archive_path)?;
    let enc = zstd::stream::write::Encoder::new(file, 3)?;
    let mut builder = tar::Builder::new(enc);
    builder.follow_symlinks(false);
    append_dir_anonymized(&mut builder, work, bundle_name)?;
    // explicit finish + fsync: a swallowed terminal flush would let a
    // truncated archive publish undetected
    builder.into_inner()?.finish()?.sync_all()?;
    Ok(archive_path)
}

/// Append the work tree with anonymized ownership: uid/gid 0 and no account
/// names in the tar headers, so the invoking user's identity never rides
/// along in archive metadata.
#[cfg(unix)]
fn append_dir_anonymized<W: std::io::Write>(
    builder: &mut tar::Builder<W>,
    dir: &Path,
    prefix: &str,
) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        // lstat, never stat: a symlink in staging (none are ever created)
        // must not be followed into its target, and a broken one must not
        // abort the whole archive
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if meta.is_dir() {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            // mask the file-type bits: POSIX tar carries the type in
            // typeflag, not the mode field
            header.set_mode(meta.permissions().mode() & 0o7777);
            header.set_size(0);
            header.set_mtime(mtime);
            header.set_uid(0);
            header.set_gid(0);
            builder.append_data(&mut header, format!("{name}/"), std::io::empty())?;
            append_dir_anonymized(builder, &path, &name)?;
        } else if meta.is_file() {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(meta.permissions().mode() & 0o7777);
            header.set_size(meta.len());
            header.set_mtime(mtime);
            header.set_uid(0);
            header.set_gid(0);
            let file = std::fs::File::open(&path)?;
            builder.append_data(&mut header, &name, file)?;
        }
        // anything else (symlinks cannot appear in our staging) is skipped
    }
    Ok(())
}

#[cfg(windows)]
pub fn build_archive(staging: &Path, work: &Path, bundle_name: &str) -> std::io::Result<PathBuf> {
    let archive_path = staging.join("bundle.zip");
    let file = create_new_private(&archive_path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    add_dir_recursive(&mut zip, work, bundle_name, &opts)?;
    zip.finish()?;
    Ok(archive_path)
}

#[cfg(windows)]
fn add_dir_recursive(
    zip: &mut zip::ZipWriter<std::fs::File>,
    dir: &Path,
    prefix: &str,
    opts: &zip::write::FileOptions<'_, ()>,
) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        if path.is_dir() {
            add_dir_recursive(zip, &path, &name, opts)?;
        } else if path.is_file() {
            zip.start_file(&name, *opts)
                .map_err(std::io::Error::other)?;
            let data = std::fs::read(&path)?;
            zip.write_all(&data)?;
        }
    }
    Ok(())
}

pub const ARCHIVE_EXT: &str = if cfg!(windows) { "zip" } else { "tar.zst" };

/// Publish the archive next to nothing else the user has; returns the final
/// path or the error message the caller reports.
pub fn publish_archive(
    archive: &Path,
    outdir: &Path,
    bundle_name: &str,
) -> Result<PathBuf, String> {
    if let Err(e) = std::fs::create_dir_all(outdir) {
        return Err(format!(
            "cannot create output dir {}: {e}",
            outdir.display()
        ));
    }
    let target = outdir.join(format!("{bundle_name}.{ARCHIVE_EXT}"));
    match publish_no_clobber(archive, &target) {
        Ok(()) => Ok(target),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(format!(
            "refusing to write {} (a file or symlink already exists there)",
            target.display()
        )),
        Err(e) => Err(format!("failed to write {}: {e}", target.display())),
    }
}

/// Write the private pseudonym map NEXT TO the bundle, never inside it. On a
/// name collision, retry with a pid-qualified name; if that also exists, the
/// map is discarded (the caller warns).
pub fn publish_map(rows: &[MapRow], outdir: &Path, bundle_name: &str) -> Option<PathBuf> {
    if rows.is_empty() {
        return None;
    }
    let mut tsv = String::new();
    for r in rows {
        tsv.push_str(&format!(
            "{}\t{}\t{}\n",
            r.kind.as_str(),
            r.real,
            r.pseudonym
        ));
    }
    let primary = outdir.join(format!("{bundle_name}.pseudonym-map.tsv"));
    let fallback = outdir.join(format!(
        "{bundle_name}.pseudonym-map.{}.tsv",
        std::process::id()
    ));
    for target in [primary, fallback] {
        if let Ok(mut f) = create_new_private(&target) {
            if f.write_all(tsv.as_bytes()).is_ok() {
                return Some(target);
            }
            // a partial map is real PII on disk AND blocks later runs
            // behind O_EXCL: remove it before trying the fallback
            drop(f);
            let _ = std::fs::remove_file(&target);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "support-bundle-publish-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn publish_refuses_existing_target() {
        let dir = scratch("noclobber");
        let src = dir.join("src.bin");
        std::fs::write(&src, b"payload").unwrap();
        let target = dir.join("out.bin");
        std::fs::write(&target, b"pre-existing").unwrap();
        let err = publish_no_clobber(&src, &target).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        // the pre-existing content was never touched
        assert_eq!(std::fs::read(&target).unwrap(), b"pre-existing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn publish_never_follows_a_planted_symlink() {
        let dir = scratch("symlink");
        let src = dir.join("src.bin");
        std::fs::write(&src, b"payload").unwrap();
        let victim = dir.join("victim.txt");
        std::fs::write(&victim, b"victim-content").unwrap();
        let target = dir.join("out.bin");
        std::os::unix::fs::symlink(&victim, &target).unwrap();
        let err = publish_no_clobber(&src, &target).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        // the symlink target was never written through
        assert_eq!(std::fs::read(&victim).unwrap(), b"victim-content");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_source_open_leaves_no_empty_target() {
        let dir = scratch("srcfail");
        let target = dir.join("out.bin");
        let missing = dir.join("does-not-exist.bin");
        assert!(publish_no_clobber(&missing, &target).is_err());
        assert!(
            !target.exists(),
            "an empty target must not be created before the source opens"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn map_collision_falls_back_to_pid_name() {
        let dir = scratch("mapname");
        let rows = vec![crate::sanitize::MapRow {
            kind: crate::sanitize::MapKind::Ip,
            real: "203.0.113.9".to_string(),
            pseudonym: "ip-1".to_string(),
        }];
        // occupy the primary name
        std::fs::write(dir.join("b.pseudonym-map.tsv"), b"taken").unwrap();
        let out = publish_map(&rows, &dir, "b").unwrap();
        assert!(
            out.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(&std::process::id().to_string())
        );
        let tsv = std::fs::read_to_string(&out).unwrap();
        assert_eq!(tsv, "ip\t203.0.113.9\tip-1\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
