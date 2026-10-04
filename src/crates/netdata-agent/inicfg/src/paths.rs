//! The walk over a user configuration directory and its stock twin (`libnetdata/paths/paths.c`
//! `recursive_config_double_dir_load()`), which health's `health.d` and statsd's `statsd.d` are loaded with.
//!
//! Every `*.conf` file of the user tree is handed to the callback, then every stock file no user file of the same
//! name shadows. Entries come in the kernel's order, as `readdir()` gives them: nothing is sorted.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

use netdata_agent_log::netdata_log_error_errno;
use netdata_agent_text::c::{filename_from_path_entry_bytes, set_errno};
use rustix::fs::{Dir, FileType, Mode, OFlags};

/// The deepest subdirectory level that is still read, the top being 0.
const MAX_DEPTH: usize = 3;

/// `stat()` of `path/entry` and what it found; a failed one leaves its `errno` for the next record, as in C.
fn path_entry_type(path: &[u8], entry: &[u8]) -> Option<std::fs::FileType> {
    let filename = filename_from_path_entry_bytes(path, entry, None);
    match std::fs::metadata(OsStr::from_bytes(&filename)) {
        Ok(metadata) => Some(metadata.file_type()),
        Err(error) => {
            set_errno(error.raw_os_error().unwrap_or(0));
            None
        }
    }
}

/// `path_entry_is_dir()`, without the creation.
fn path_entry_is_dir(path: &[u8], entry: &[u8]) -> bool {
    path_entry_type(path, entry).is_some_and(|file_type| file_type.is_dir())
}

/// `path_entry_is_file()`.
fn path_entry_is_file(path: &[u8], entry: &[u8]) -> bool {
    path_entry_type(path, entry).is_some_and(|file_type| file_type.is_file())
}

/// `opendir()`; a failure leaves its `errno` for the caller's record.
fn opendir(path: &[u8]) -> Option<Dir> {
    let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::DIRECTORY | OFlags::CLOEXEC;
    match rustix::fs::open(OsStr::from_bytes(path), flags, Mode::empty()).and_then(Dir::new) {
        Ok(dir) => Some(dir),
        Err(error) => {
            set_errno(error.raw_os_error());
            None
        }
    }
}

/// The entries of `dir` as `readdir()` gives them (a failed read ends the directory): each name and `d_type`.
fn entries(dir: Dir) -> impl Iterator<Item = (Vec<u8>, FileType)> {
    dir.map_while(Result::ok).map(|entry| (entry.file_name().to_bytes().to_vec(), entry.file_type()))
}

/// `d_type` says directory or link (never for a filesystem that reports no types), and the name is not `.` or `..`.
fn may_be_dir(name: &[u8], d_type: FileType) -> Option<bool> {
    if !matches!(d_type, FileType::Directory | FileType::Symlink) {
        return Some(false);
    }
    if name.is_empty() || name == b"." || name == b".." {
        return None;
    }
    Some(true)
}

fn may_be_file(d_type: FileType) -> bool {
    matches!(d_type, FileType::Unknown | FileType::RegularFile | FileType::Symlink)
}

/// A name longer than its `.conf` suffix.
fn is_conf_name(name: &[u8]) -> bool {
    name.len() > 5 && name.ends_with(b".conf")
}

fn lossy(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

/// `recursive_config_double_dir_load()`: calls `callback(filename, stock)` for every `*.conf` regular file under
/// `user_path/entry`, then for every one under `stock_path/entry` that no user file of the same name shadows.
/// Without a stock path (stock configuration off) the user tree alone is read. A subdirectory is entered up to
/// three levels deep; one that both trees hold is read once, with both of its sides.
pub fn recursive_config_double_dir_load(
    user_path: &[u8],
    stock_path: Option<&[u8]>,
    entry: &[u8],
    callback: &mut dyn FnMut(&[u8], bool),
) {
    walk(user_path, stock_path, entry, callback, 0);
}

fn walk(user_path: &[u8], stock_path: Option<&[u8]>, entry: &[u8], callback: &mut dyn FnMut(&[u8], bool), depth: usize) {
    let stock_path = stock_path.unwrap_or(user_path);
    if depth > MAX_DEPTH {
        netdata_log_error_errno!(
            "CONFIG: Max directory depth reached while reading user path '{}', stock path '{}', subpath '{}'",
            lossy(user_path),
            lossy(stock_path),
            lossy(entry)
        );
        return;
    }

    let udir = filename_from_path_entry_bytes(user_path, entry, None);
    let sdir = filename_from_path_entry_bytes(stock_path, entry, None);

    match opendir(&udir) {
        None => netdata_log_error_errno!("CONFIG cannot open user-config directory '{}'.", lossy(&udir)),
        Some(dir) => {
            for (name, d_type) in entries(dir) {
                match may_be_dir(&name, d_type) {
                    None => continue,
                    Some(true) if path_entry_is_dir(&udir, &name) => {
                        walk(&udir, Some(&sdir), &name, callback, depth + 1);
                        continue;
                    }
                    Some(_) => {}
                }
                // the stat() comes before the name's test, as in C: its errno is left behind either way
                if may_be_file(d_type) && path_entry_is_file(&udir, &name) && is_conf_name(&name) {
                    callback(&filename_from_path_entry_bytes(&udir, &name, None), false);
                }
            }
        }
    }

    match opendir(&sdir) {
        None => netdata_log_error_errno!("CONFIG cannot open stock config directory '{}'.", lossy(&sdir)),
        // with the stock configuration off both are the user directory, opened again and not read
        Some(dir) if udir != sdir => {
            for (name, d_type) in entries(dir) {
                match may_be_dir(&name, d_type) {
                    None => continue,
                    Some(true) if path_entry_is_dir(&sdir, &name) => {
                        // a subdirectory the user tree has too was read by the user pass
                        if !path_entry_is_dir(&udir, &name) {
                            walk(&udir, Some(&sdir), &name, callback, depth + 1);
                        }
                        continue;
                    }
                    Some(_) => {}
                }
                if may_be_file(d_type)
                    && path_entry_is_file(&sdir, &name)
                    && !path_entry_is_file(&udir, &name)
                    && is_conf_name(&name)
                {
                    callback(&filename_from_path_entry_bytes(&sdir, &name, None), true);
                }
            }
        }
        Some(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    use netdata_agent_log::{Captured, Priority, capture};
    use netdata_agent_text::c::take_errno;

    use super::*;

    const ENOENT: i32 = 2;
    const ENOTDIR: i32 = 20;

    /// A temporary directory with a `user` and a `stock` tree.
    struct Trees {
        root: tempfile::TempDir,
    }

    impl Trees {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("a temporary directory");
            fs::create_dir(root.path().join("user")).expect("user");
            fs::create_dir(root.path().join("stock")).expect("stock");
            Trees { root }
        }

        fn file(&self, path: &str) -> &Self {
            let path = self.root.path().join(path);
            fs::create_dir_all(path.parent().expect("a parent")).expect("directories");
            fs::write(path, b"").expect("a file");
            self
        }

        fn dir(&self, path: &str) -> &Self {
            fs::create_dir_all(self.root.path().join(path)).expect("directories");
            self
        }

        fn link(&self, target: &str, path: &str) -> &Self {
            symlink(target, self.root.path().join(path)).expect("a link");
            self
        }

        fn path(&self, path: &str) -> Vec<u8> {
            self.root.path().join(path).as_os_str().as_bytes().to_vec()
        }

        /// The callback's calls, sorted (the kernel's order is not the test's business), with paths relative to the
        /// root, and the records.
        fn load(&self, stock: bool) -> (Vec<(String, bool)>, Vec<Captured>) {
            self.load_from("user", stock.then_some("stock"))
        }

        fn load_from(&self, user: &str, stock: Option<&str>) -> (Vec<(String, bool)>, Vec<Captured>) {
            take_errno();
            let (user, stock) = (self.path(user), stock.map(|stock| self.path(stock)));
            let mut calls = Vec::new();
            let ((), records) = capture(|| {
                recursive_config_double_dir_load(&user, stock.as_deref(), b"", &mut |filename, stock| {
                    let path = Path::new(OsStr::from_bytes(filename));
                    let relative = path.strip_prefix(self.root.path()).expect("under the root");
                    calls.push((relative.to_string_lossy().into_owned(), stock));
                });
            });
            calls.sort();
            (calls, records)
        }
    }

    fn calls(expected: &[(&str, bool)]) -> Vec<(String, bool)> {
        expected.iter().map(|&(path, stock)| (path.to_owned(), stock)).collect()
    }

    /// The records as (errno, message with the root replaced by `ROOT`).
    fn shown(trees: &Trees, records: &[Captured]) -> Vec<(i32, String)> {
        let root = trees.root.path().to_string_lossy().into_owned();
        records
            .iter()
            .map(|record| {
                assert_eq!(record.priority, Priority::Err);
                (record.errno, record.message.clone().unwrap_or_default().replace(&root, "ROOT"))
            })
            .collect()
    }

    #[test]
    fn user_files_shadow_stock_files_of_the_same_name() {
        let trees = Trees::new();
        trees.file("user/a.conf").file("user/b.conf").file("stock/b.conf").file("stock/c.conf");
        let (loaded, records) = trees.load(true);
        assert_eq!(loaded, calls(&[("stock/c.conf", true), ("user/a.conf", false), ("user/b.conf", false)]));
        assert!(records.is_empty());

        // the user pass comes first
        let mut order = Vec::new();
        recursive_config_double_dir_load(&trees.path("user"), Some(&trees.path("stock")), b"", &mut |_, stock| {
            order.push(stock);
        });
        assert_eq!(order, [false, false, true]);
    }

    #[test]
    fn only_names_longer_than_the_suffix_and_ending_in_it_are_loaded() {
        let trees = Trees::new();
        trees
            .file("user/.conf")
            .file("user/x.conf")
            .file("user/.x.conf")
            .file("user/x.CONF")
            .file("user/x.conf.bak")
            .file("user/xconf")
            .file("stock/.conf")
            .file("stock/y.conf")
            .file("stock/y.txt");
        let (loaded, records) = trees.load(true);
        assert_eq!(loaded, calls(&[("stock/y.conf", true), ("user/.x.conf", false), ("user/x.conf", false)]));
        assert!(records.is_empty());
    }

    #[test]
    fn a_directory_shadows_nothing_and_is_no_file() {
        let trees = Trees::new();
        // a user directory with a stock file's name does not shadow it, and is entered
        trees.file("user/a.conf/inner.conf").file("stock/a.conf").dir("stock/b.conf").file("user/b.conf");
        let (loaded, records) = trees.load(true);
        assert_eq!(loaded, calls(&[("stock/a.conf", true), ("user/a.conf/inner.conf", false), ("user/b.conf", false)]));
        // each directory is opened on the other side too, where it is a file
        assert_eq!(
            shown(&trees, &records),
            [
                (ENOTDIR, "CONFIG cannot open stock config directory 'ROOT/stock/a.conf'.".to_owned()),
                (ENOTDIR, "CONFIG cannot open user-config directory 'ROOT/user/b.conf'.".to_owned()),
            ]
        );
    }

    #[test]
    fn subdirectories_are_read_three_levels_deep() {
        let trees = Trees::new();
        trees
            .file("user/0.conf")
            .file("user/d1/1.conf")
            .file("user/d1/d2/2.conf")
            .file("user/d1/d2/d3/3.conf")
            .file("user/d1/d2/d3/d4/4.conf")
            .dir("stock/d1/d2/d3/d4")
            .file("stock/d1/d2/s.conf");
        let (loaded, records) = trees.load(true);
        assert_eq!(
            loaded,
            calls(&[
                ("stock/d1/d2/s.conf", true),
                ("user/0.conf", false),
                ("user/d1/1.conf", false),
                ("user/d1/d2/2.conf", false),
                ("user/d1/d2/d3/3.conf", false),
            ])
        );
        // a subdirectory of both trees is entered once, by the user pass
        assert_eq!(
            shown(&trees, &records),
            [(
                0,
                "CONFIG: Max directory depth reached while reading user path 'ROOT/user/d1/d2/d3', stock path \
                 'ROOT/stock/d1/d2/d3', subpath 'd4'"
                    .to_owned()
            )]
        );
    }

    #[test]
    fn a_stock_only_subdirectory_is_read_and_its_missing_twin_logged() {
        let trees = Trees::new();
        trees.file("stock/only/s.conf").file("user/mine/u.conf");
        let (loaded, records) = trees.load(true);
        assert_eq!(loaded, calls(&[("stock/only/s.conf", true), ("user/mine/u.conf", false)]));
        assert_eq!(
            shown(&trees, &records),
            [
                (ENOENT, "CONFIG cannot open stock config directory 'ROOT/stock/mine'.".to_owned()),
                (ENOENT, "CONFIG cannot open user-config directory 'ROOT/user/only'.".to_owned()),
            ]
        );
    }

    #[test]
    fn links_are_followed_and_a_dangling_one_is_nothing() {
        let trees = Trees::new();
        trees
            .file("elsewhere/dir/in-linked-dir.conf")
            .file("elsewhere/file.conf")
            .link("../elsewhere/dir", "user/linked")
            .link("../elsewhere/file.conf", "user/linked.conf")
            .link("no-such-file", "user/dangling.conf")
            .file("stock/dangling.conf");
        let (loaded, records) = trees.load(true);
        assert_eq!(
            loaded,
            calls(&[
                ("stock/dangling.conf", true),
                ("user/linked.conf", false),
                ("user/linked/in-linked-dir.conf", false),
            ])
        );
        assert_eq!(
            shown(&trees, &records),
            [(ENOENT, "CONFIG cannot open stock config directory 'ROOT/stock/linked'.".to_owned())]
        );
    }

    #[test]
    fn without_stock_the_user_directory_is_opened_twice_and_read_once() {
        let trees = Trees::new();
        trees.file("user/a.conf").file("user/sub/b.conf").file("stock/c.conf");
        let (loaded, records) = trees.load(false);
        assert_eq!(loaded, calls(&[("user/a.conf", false), ("user/sub/b.conf", false)]));
        assert!(records.is_empty());

        // a missing user directory is reported by both passes
        let (loaded, records) = trees.load_from("missing", None);
        assert!(loaded.is_empty());
        assert_eq!(
            shown(&trees, &records),
            [
                (ENOENT, "CONFIG cannot open user-config directory 'ROOT/missing'.".to_owned()),
                (ENOENT, "CONFIG cannot open stock config directory 'ROOT/missing'.".to_owned()),
            ]
        );
    }

    #[test]
    fn a_missing_stock_directory_is_reported_and_the_user_files_load() {
        let trees = Trees::new();
        trees.file("user/a.conf");
        let (loaded, records) = trees.load_from("user", Some("missing"));
        assert_eq!(loaded, calls(&[("user/a.conf", false)]));
        assert_eq!(
            shown(&trees, &records),
            [(ENOENT, "CONFIG cannot open stock config directory 'ROOT/missing'.".to_owned())]
        );
    }

    /// C's `stat()` of the user file a stock file may be shadowed by fails when there is none, and the callback
    /// starts with that `errno`: the first record it writes shows it.
    #[test]
    fn an_unshadowed_stock_file_is_read_with_the_failed_stat_s_errno() {
        let trees = Trees::new();
        trees.file("user/u.conf").file("stock/s.conf");
        take_errno();
        let mut seen = Vec::new();
        recursive_config_double_dir_load(&trees.path("user"), Some(&trees.path("stock")), b"", &mut |_, stock| {
            seen.push((stock, take_errno()));
        });
        assert_eq!(seen, [(false, 0), (true, ENOENT)]);
    }

    #[test]
    fn the_entry_is_joined_to_both_paths() {
        let trees = Trees::new();
        trees.file("user/health.d/a.conf").file("stock/health.d/b.conf").file("user/other.conf");
        take_errno();
        let mut loaded = Vec::new();
        recursive_config_double_dir_load(
            &trees.path("user/"),
            Some(&trees.path("stock")),
            b"/health.d",
            &mut |filename, stock| loaded.push((filename.to_vec(), stock)),
        );
        loaded.sort();
        assert_eq!(loaded, [(trees.path("stock/health.d/b.conf"), true), (trees.path("user/health.d/a.conf"), false)]);
    }
}
