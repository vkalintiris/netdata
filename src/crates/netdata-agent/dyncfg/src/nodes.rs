//! DynCfg's nodes (`src/daemon/dyncfg/dyncfg.c`, `dyncfg-files.c`): C's `dyncfg_globals.nodes` dictionary in its
//! insertion order (B12), its insert and conflict callbacks, the saved files' load at start and their saves and
//! deletes, and the status and delete a plugin's CONFIG asks for.
//!
//! The lock is a leaf: nothing here registers a method, calls a plugin or answers a call while holding it (a refused
//! call answers synchronously, and its callback may come back here).

use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use indexmap::IndexMap;
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_nrpc::Handler;
use netdata_agent_rrd::clock::now_realtime_ut;
use netdata_agent_text::print::print_uuid_lower;
use rustix::fs::{Dir, FileType, Mode, OFlags};

use crate::files::{self, Payload, Saved};
use crate::model::{Cmds, SourceType, Status, Type};

/// The node's state as the plugin runs it (`df->current`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Current {
    pub status: Status,
    pub source_type: SourceType,
    pub source: Vec<u8>,
    pub created_ut: u64,
    pub modified_ut: u64,
}

/// The node's state as the user changed it and its file saves it (`df->dyncfg`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stored {
    pub saves: u32,
    pub restart_required: bool,
    pub plugin_rejected: bool,
    pub user_disabled: bool,
    pub status: Status,
    pub source_type: SourceType,
    pub source: Vec<u8>,
    pub payload: Option<Payload>,
    pub created_ut: u64,
    pub modified_ut: u64,
}

/// `DYNCFG`: one configuration.
#[derive(Debug, Clone, Default)]
pub struct Node {
    pub host_uuid: [u8; 16],
    /// `config <id>`, its method's name.
    pub function: Vec<u8>,
    pub template: Option<Vec<u8>>,
    pub path: Vec<u8>,
    pub cmds: Cmds,
    pub kind: Type,
    pub view_access: u32,
    pub edit_access: u32,
    pub current: Current,
    pub stored: Stored,
    pub sync: bool,
    /// The plugin's handler (C's `handler`, `handler_data` and the transport it pins); none for a loaded orphan.
    pub handler: Option<Handler>,
    /// Which insertion of the id this is: C's echo holds the node it was sent for, so its answer changes nothing once
    /// the id was deleted and set again.
    pub(crate) serial: u64,
}

impl Node {
    /// `dyncfg_set_current_from_dyncfg()`: the stored state becomes the current one; the current times only widen.
    pub fn set_current_from_stored(&mut self) {
        self.current.status = self.stored.status;
        self.current.source_type = self.stored.source_type;
        self.current.source = self.stored.source.clone();
        if self.stored.created_ut < self.current.created_ut {
            self.current.created_ut = self.stored.created_ut;
        }
        if self.stored.modified_ut > self.current.modified_ut {
            self.current.modified_ut = self.stored.modified_ut;
        }
    }

    /// `dyncfg_update_status_on_successful_add_or_update()`: accepted by the plugin, a restart required on 299, the
    /// stored state current.
    pub fn on_successful_add_or_update(&mut self, code: u16) {
        self.stored.plugin_rejected = false;
        self.stored.restart_required = code == crate::model::RESP_ACCEPTED_RESTART_REQUIRED;
        self.set_current_from_stored();
    }

    /// The part of the node its file holds.
    fn to_saved(&self) -> Saved {
        Saved {
            template: self.template.clone(),
            host_uuid: self.host_uuid,
            path: self.path.clone(),
            kind: self.kind,
            cmds: self.cmds,
            sync: self.sync,
            source_type: self.stored.source_type,
            source: self.stored.source.clone(),
            created_ut: self.stored.created_ut,
            modified_ut: self.stored.modified_ut,
            user_disabled: self.stored.user_disabled,
            saves: self.stored.saves,
            payload: self.stored.payload.clone(),
        }
    }

    /// `dyncfg_normalize()`: unset current times are now.
    fn normalize(&mut self, now_ut: u64) {
        if self.current.created_ut == 0 {
            self.current.created_ut = now_ut;
        }
        if self.current.modified_ut == 0 {
            self.current.modified_ut = now_ut;
        }
    }
}

/// What a set did to the dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Set {
    /// A new node (`dyncfg_insert_cb()`).
    Inserted,
    /// An existing node took the new one's values (`dyncfg_conflict_cb()`), `true` when anything changed.
    Merged(bool),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `dyncfg_globals`: the nodes and the directory of their files.
#[derive(Debug)]
pub struct Nodes {
    dir: PathBuf,
    map: Mutex<IndexMap<Vec<u8>, Node>>,
    /// The last insertion's serial.
    serial: AtomicU64,
}

impl Nodes {
    /// `dyncfg_init_low_level()`: `<varlib>/config` created (0755 under the process umask; an existing one is fine, any
    /// other failure C's CRIT), then, with `load_saved`, every saved file loaded.
    pub fn init(varlib: &Path, load_saved: bool) -> Self {
        let dir = varlib.join("config");
        if let Err(e) = fs::DirBuilder::new().mode(0o755).create(&dir)
            && e.kind() != std::io::ErrorKind::AlreadyExists
        {
            nd_log!(
                Source::Daemon,
                Priority::Crit,
                "DYNCFG: failed to create dynamic configuration directory '{}'",
                dir.display()
            );
        }
        let nodes = Nodes { dir, map: Mutex::new(IndexMap::new()), serial: AtomicU64::new(0) };
        if load_saved {
            nodes.load_all();
        }
        nodes
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The nodes, under the leaf lock.
    pub fn lock(&self) -> MutexGuard<'_, IndexMap<Vec<u8>, Node>> {
        lock(&self.map)
    }

    /// `dictionary_set()` with DynCfg's callbacks: a new id is inserted (`dyncfg_insert_cb()`), an existing one takes
    /// the new node's values (`dyncfg_conflict_cb()`; its handler only when it has none, or with `overwrite_handler`
    /// when the new one differs). `None` for an empty id, which C's dictionary refuses.
    pub fn set(&self, id: &[u8], node: Node, overwrite_handler: bool) -> Option<Set> {
        if id.is_empty() {
            return None;
        }
        let now_ut = now_realtime_ut();
        let mut map = self.lock();
        Some(match map.get_mut(id) {
            None => {
                let mut node = inserted(id, node, now_ut);
                node.serial = self.serial.fetch_add(1, Ordering::Relaxed) + 1;
                map.insert(id.to_vec(), node);
                Set::Inserted
            }
            Some(old) => Set::Merged(merge(id, old, node, overwrite_handler, now_ut)),
        })
    }

    /// `dyncfg_load_all()`: every entry of the directory ending `.dyncfg` whose `d_type` says a regular file or a link
    /// (a filesystem that reports none loads nothing, as C), in the directory's order (P2).
    pub fn load_all(&self) {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let Ok(dir) = rustix::fs::open(&self.dir, flags, Mode::empty()).and_then(Dir::read_from) else {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: cannot open directory '{}'", self.dir.display());
            return;
        };
        for entry in dir {
            let Ok(entry) = entry else {
                break;
            };
            let name = entry.file_name().to_bytes();
            if matches!(entry.file_type(), FileType::RegularFile | FileType::Symlink) && name.ends_with(b".dyncfg") {
                self.load_file(std::ffi::OsStr::from_bytes(name));
            }
        }
    }

    /// `dyncfg_file_load()`: the file parsed, its node an orphan of its saved state, set without its handler; then a
    /// file whose name is not its id's canonical one is renamed to it.
    pub fn load_file(&self, d_name: &std::ffi::OsStr) {
        let filename = self.dir.join(d_name);
        let shown = filename.display().to_string();
        let Ok(file) = fs::File::open(&filename) else {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: cannot open file '{shown}'");
            return;
        };
        let Some((id, saved)) = files::load(file, &shown) else {
            return;
        };
        let mut node = Node {
            host_uuid: saved.host_uuid,
            template: saved.template,
            path: saved.path,
            cmds: saved.cmds,
            kind: saved.kind,
            sync: saved.sync,
            stored: Stored {
                saves: saved.saves,
                user_disabled: saved.user_disabled,
                status: Status::Orphan,
                source_type: saved.source_type,
                source: saved.source,
                payload: saved.payload,
                created_ut: saved.created_ut,
                modified_ut: saved.modified_ut,
                ..Stored::default()
            },
            ..Node::default()
        };
        node.set_current_from_stored();
        if self.set(&id, node, false).is_none() {
            return;
        }
        let fixed = self.dir.join(std::ffi::OsStr::from_bytes(&files::file_name(&id)));
        if fixed != filename && fs::rename(&filename, &fixed).is_err() {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "DYNCFG: cannot rename file '{shown}' into '{}'. Saving a new configuraton may not overwrite the old one.",
                fixed.display()
            );
        }
    }

    /// `dyncfg_file_save()`: the node's file written in place (C's `fopen("w")`), its save stamped and counted first.
    pub fn save(&self, id: &[u8], node: &mut Node) {
        let filename = self.dir.join(std::ffi::OsStr::from_bytes(&files::file_name(id)));
        let Ok(mut file) = fs::File::create(&filename) else {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: cannot create file '{}'", filename.display());
            return;
        };
        let mut saved = node.to_saved();
        let text = files::render(id, &mut saved, now_realtime_ut());
        node.stored.created_ut = saved.created_ut;
        node.stored.modified_ut = saved.modified_ut;
        node.stored.saves = saved.saves;
        // C writes through stdio and does not check the result
        let _ = file.write_all(&text);
    }

    /// `dyncfg_file_delete()`.
    pub fn delete_file(&self, id: &[u8]) {
        let _ = fs::remove_file(self.dir.join(std::ffi::OsStr::from_bytes(&files::file_name(id))));
    }

    /// `dyncfg_is_user_disabled()`.
    pub fn is_user_disabled(&self, id: &[u8]) -> bool {
        self.lock().get(id).is_some_and(|n| n.stored.user_disabled)
    }

    /// `dyncfg_job_has_registered_template()`: the id up to its last `:` names a template node.
    pub fn job_has_registered_template(&self, id: &[u8]) -> bool {
        let Some(colon) = id.iter().rposition(|&c| c == b':') else {
            return false;
        };
        self.lock().get(&id[..colon]).is_some_and(|n| n.kind == Type::Template)
    }

    /// `dyncfg_status_low_level()`: a plugin's status for a node.
    pub fn status(&self, id: &[u8], status: Status) {
        let shown = String::from_utf8_lossy(id);
        if !crate::model::is_valid_id(id) {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: id '{shown}' is invalid. Ignoring dynamic configuration for it.");
            return;
        }
        if status == Status::None {
            nd_log!(Source::Daemon, Priority::Err, "DYNCFG: status provided to id '{shown}' is invalid. Ignoring it.");
            return;
        }
        if let Some(node) = self.lock().get_mut(id) {
            node.current.status = status;
        }
    }

    /// `dyncfg_del_low_level()`'s dictionary half: the node's method name (to unregister), the node itself gone when
    /// it was never saved. `None` for an id not here.
    pub fn delete(&self, id: &[u8]) -> Option<Vec<u8>> {
        let mut map = self.lock();
        let node = map.get(id)?;
        let function = node.function.clone();
        if node.stored.saves == 0 {
            map.shift_remove(id);
        }
        Some(function)
    }
}

/// `dyncfg_insert_cb()`: times stamped, the method name made, a job without a template given the id up to its last
/// `:` (C's WARNING when there is none).
fn inserted(id: &[u8], mut node: Node, now_ut: u64) -> Node {
    node.normalize(now_ut);
    node.function = [b"config ".as_slice(), id].concat();
    if node.kind == Type::Job && node.template.is_none() {
        match id.iter().rposition(|&c| c == b':') {
            Some(colon) => node.template = Some(id[..colon].to_vec()),
            None => nd_log!(
                Source::Daemon,
                Priority::Warning,
                "DYNCFG: id '{}' is a job, but does not contain a colon to find the template",
                String::from_utf8_lossy(id)
            ),
        }
    }
    node
}

/// `dyncfg_conflict_cb()`: the old node takes what the new one changes (C's NOTICEs for a host and a path), the
/// current times only widen, the stored half stays; the handler moves when the old node has none, or on
/// `overwrite_handler` when the new one differs.
fn merge(id: &[u8], old: &mut Node, mut new: Node, overwrite_handler: bool, now_ut: u64) -> bool {
    new.normalize(now_ut);
    let shown = String::from_utf8_lossy(id);
    let mut changes = 0;
    if old.host_uuid != new.host_uuid {
        let (mut from, mut to) = (Vec::new(), Vec::new());
        print_uuid_lower(&mut from, &old.host_uuid);
        print_uuid_lower(&mut to, &new.host_uuid);
        nd_log!(
            Source::Daemon,
            Priority::Notice,
            "DYNCFG: configuration '{shown}' changed host id from '{}' to '{}'",
            String::from_utf8_lossy(&from),
            String::from_utf8_lossy(&to)
        );
        old.host_uuid = new.host_uuid;
        changes += 1;
    }
    if old.path != new.path {
        nd_log!(
            Source::Daemon,
            Priority::Notice,
            "DYNCFG: configuration '{shown}' changed path from '{}' to '{}'",
            String::from_utf8_lossy(&old.path),
            String::from_utf8_lossy(&new.path)
        );
        old.path = new.path;
        changes += 1;
    }
    if old.cmds != new.cmds {
        old.cmds = new.cmds;
        changes += 1;
    }
    if old.kind != new.kind {
        old.kind = new.kind;
        changes += 1;
    }
    if old.view_access != new.view_access {
        old.view_access = new.view_access;
        changes += 1;
    }
    if old.edit_access != new.edit_access {
        old.edit_access = new.edit_access;
        changes += 1;
    }
    if old.current.status != new.current.status {
        old.current.status = new.current.status;
        changes += 1;
    }
    if old.current.source_type != new.current.source_type {
        old.current.source_type = new.current.source_type;
        changes += 1;
    }
    if old.current.source != new.current.source {
        old.current.source = std::mem::take(&mut new.current.source);
        changes += 1;
    }
    if new.current.created_ut < old.current.created_ut {
        old.current.created_ut = new.current.created_ut;
        changes += 1;
    }
    if new.current.modified_ut > old.current.modified_ut {
        old.current.modified_ut = new.current.modified_ut;
        changes += 1;
    }
    let transfer = match (&old.handler, &new.handler) {
        (None, _) => true,
        (Some(current), Some(offered)) => overwrite_handler && !current.same(offered),
        (Some(_), None) => false,
    };
    if transfer {
        old.sync = new.sync;
        old.handler = new.handler;
        changes += 1;
    }
    changes > 0
}

#[cfg(test)]
mod tests {
    use netdata_agent_log::{Captured, capture};
    use netdata_agent_nrpc::reply::{Payload as CallPayload, Reply};
    use netdata_agent_nrpc::testing::inert;

    use super::*;

    fn texts(records: Vec<Captured>) -> Vec<(Priority, String)> {
        records.into_iter().map(|r| (r.priority, r.message.unwrap_or_default())).collect()
    }

    fn other(_: &mut Reply, _: &[u8], _: Option<&CallPayload>, _: &[u8]) -> u16 {
        200
    }

    fn nodes() -> (tempfile::TempDir, Nodes) {
        let dir = tempfile::tempdir().unwrap();
        let nodes = Nodes::init(dir.path(), false);
        (dir, nodes)
    }

    /// `dyncfg_insert_cb()`: the method name, a job's template from its id, C's WARNING for a job without a colon,
    /// the current times stamped.
    #[test]
    fn an_insert_is_cs() {
        let (_dir, nodes) = nodes();
        let job = Node { kind: Type::Job, ..Node::default() };
        assert_eq!(nodes.set(b"go.d:nginx:local", job.clone(), true), Some(Set::Inserted));
        assert_eq!(nodes.set(b"", job.clone(), true), None, "C's dictionary takes no empty name");
        let ((), logged) = capture(|| {
            nodes.set(b"lonely", job, true);
        });
        assert_eq!(
            texts(logged),
            [(Priority::Warning, "DYNCFG: id 'lonely' is a job, but does not contain a colon to find the template".into())]
        );
        let map = nodes.lock();
        let node = &map[b"go.d:nginx:local".as_slice()];
        assert_eq!((node.function.as_slice(), node.template.as_deref()), (&b"config go.d:nginx:local"[..], Some(&b"go.d:nginx"[..])));
        assert!(node.current.created_ut > 0 && node.current.modified_ut == node.current.created_ut);
        assert_eq!(map[b"lonely".as_slice()].template, None);
        assert_eq!(map.keys().map(|k| k.as_slice()).collect::<Vec<_>>(), [b"go.d:nginx:local".as_slice(), b"lonely"]);
    }

    /// `dyncfg_conflict_cb()`: the old node takes each changed field, C's NOTICEs for a host and a path, the times only
    /// widen, the stored half stays; the handler moves to an empty node always, to a held one only on overwrite and
    /// when it differs.
    #[test]
    fn a_merge_is_cs() {
        let (_dir, nodes) = nodes();
        let first = Node {
            path: b"/a".to_vec(),
            current: Current { created_ut: 10, modified_ut: 20, ..Current::default() },
            stored: Stored { saves: 3, ..Stored::default() },
            ..Node::default()
        };
        nodes.set(b"s", first, false);
        let second = Node {
            host_uuid: [1; 16],
            path: b"/b".to_vec(),
            cmds: Cmds::GET,
            kind: Type::Single,
            edit_access: 0x47,
            current: Current { status: Status::Running, created_ut: 5, modified_ut: 15, ..Current::default() },
            sync: true,
            handler: Some(Handler::Builtin(inert)),
            ..Node::default()
        };
        let (set, logged) = capture(|| nodes.set(b"s", second, false));
        assert_eq!(set, Some(Set::Merged(true)));
        assert_eq!(
            texts(logged),
            [
                (
                    Priority::Notice,
                    "DYNCFG: configuration 's' changed host id from '00000000-0000-0000-0000-000000000000' to \
                     '01010101-0101-0101-0101-010101010101'"
                        .into()
                ),
                (Priority::Notice, "DYNCFG: configuration 's' changed path from '/a' to '/b'".into()),
            ]
        );
        {
            let map = nodes.lock();
            let node = &map[b"s".as_slice()];
            assert_eq!((node.cmds, node.edit_access, node.current.status), (Cmds::GET, 0x47, Status::Running));
            assert_eq!((node.current.created_ut, node.current.modified_ut, node.stored.saves), (5, 20, 3));
            assert!(node.sync && node.handler.as_ref().is_some_and(|h| h.same(&Handler::Builtin(inert))));
        }
        let held = |handler: Handler, overwrite: bool| {
            let node = Node { host_uuid: [1; 16], path: b"/b".to_vec(), cmds: Cmds::GET, edit_access: 0x47, handler: Some(handler), ..Node::default() };
            // times that do not widen, so only the handler can change (a zero time is stamped now, which widens)
            let current = Current { status: Status::Running, created_ut: 5, modified_ut: 15, ..Current::default() };
            let set = nodes.set(b"s", Node { current, ..node }, overwrite).unwrap();
            (set, nodes.lock()[b"s".as_slice()].handler.as_ref().is_some_and(|h| h.same(&Handler::Builtin(other))))
        };
        assert_eq!(held(Handler::Builtin(other), false), (Set::Merged(false), false), "kept without overwrite");
        assert_eq!(held(Handler::Builtin(inert), true), (Set::Merged(false), false), "the same one");
        assert_eq!(held(Handler::Builtin(other), true), (Set::Merged(true), true), "moved");
    }

    /// `dyncfg_file_load()` through `dyncfg_load_all()`: files in the directory's order, each an orphan of its saved
    /// state (its current creation now, its modification the file's), commands sanitized, a non-canonical name renamed;
    /// other names, a directory and a file without an id ignored.
    #[test]
    fn saved_files_load_as_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        fs::create_dir(&config).unwrap();
        fs::write(config.join("x.dyncfg"), b"id=go.d:j\ntype=job\nsource_type=dyncfg\nmodified=7\ncreated=3\ncmds=get\n").unwrap();
        fs::write(config.join("noid.dyncfg"), b"path=/p\n").unwrap();
        fs::write(config.join("note.txt"), b"id=t\n").unwrap();
        fs::create_dir(config.join("d.dyncfg")).unwrap();
        // a link is loaded by its d_type; to a directory, it opens and fails its first read
        std::os::unix::fs::symlink(config.join("d.dyncfg"), config.join("l.dyncfg")).unwrap();
        let (nodes, logged) = capture(|| Nodes::init(dir.path(), true));
        let logged = texts(logged);
        assert!(logged.iter().any(|(p, t)| *p == Priority::Err && t.ends_with("noid.dyncfg' does not include a unique id. Ignoring it.")));
        let unreadable = format!("DYNCFG: failed while reading metadata from file '{}'. Ignoring it.", config.join("l.dyncfg").display());
        assert!(logged.contains(&(Priority::Err, unreadable)), "{logged:?}");
        let map = nodes.lock();
        assert_eq!(map.keys().collect::<Vec<_>>(), [&b"go.d:j".to_vec()]);
        let node = &map[b"go.d:j".as_slice()];
        assert_eq!((node.current.status, node.stored.status, node.kind), (Status::Orphan, Status::Orphan, Type::Job));
        assert_eq!((node.current.modified_ut, node.stored.created_ut), (7, 3));
        assert!(node.current.created_ut > 7, "C's set_current keeps a zero creation, then the insert stamps it now");
        assert_eq!(node.cmds, Cmds::GET | Cmds::SCHEMA | Cmds::REMOVE);
        assert_eq!((node.view_access, node.edit_access, node.handler.is_none()), (0, 0, true));
        assert!(!config.join("x.dyncfg").exists() && config.join("go.d%3Aj.dyncfg").exists(), "renamed");
    }

    /// `dyncfg_file_save()` and `dyncfg_file_delete()`: the file under the id's name, the save stamped and counted in
    /// the node, read back the same.
    #[test]
    fn a_node_is_saved_and_deleted() {
        let (dir, nodes) = nodes();
        let mut node = Node {
            template: Some(b"t".to_vec()),
            kind: Type::Job,
            cmds: Cmds::GET | Cmds::SCHEMA | Cmds::REMOVE,
            stored: Stored { source_type: SourceType::Dyncfg, source: b"user".to_vec(), ..Stored::default() },
            ..Node::default()
        };
        nodes.save(b"t:j", &mut node);
        assert_eq!(node.stored.saves, 1);
        assert!(node.stored.created_ut > 0 && node.stored.created_ut == node.stored.modified_ut);
        let file = dir.path().join("config").join("t%3Aj.dyncfg");
        let (id, saved) = files::parse(&fs::read(&file).unwrap(), "f").unwrap();
        assert_eq!((id.as_slice(), saved.saves, saved.source.as_slice()), (&b"t:j"[..], 1, &b"user"[..]));
        nodes.delete_file(b"t:j");
        assert!(!file.exists());
    }

    /// `dyncfg_status_low_level()`, `dyncfg_del_low_level()`'s dictionary half, `dyncfg_is_user_disabled()` and
    /// `dyncfg_job_has_registered_template()`.
    #[test]
    fn status_delete_and_lookups_are_cs() {
        let (_dir, nodes) = nodes();
        nodes.set(b"t", Node { kind: Type::Template, ..Node::default() }, true);
        nodes.set(b"s", Node { stored: Stored { saves: 1, user_disabled: true, ..Stored::default() }, ..Node::default() }, true);
        let ((), logged) = capture(|| {
            nodes.status(b"a b", Status::Running);
            nodes.status(b"s", Status::None);
            nodes.status(b"s", Status::Failed);
        });
        assert_eq!(
            texts(logged),
            [
                (Priority::Err, "DYNCFG: id 'a b' is invalid. Ignoring dynamic configuration for it.".into()),
                (Priority::Err, "DYNCFG: status provided to id 's' is invalid. Ignoring it.".into()),
            ]
        );
        assert_eq!(nodes.lock()[b"s".as_slice()].current.status, Status::Failed);
        assert_eq!((nodes.is_user_disabled(b"s"), nodes.is_user_disabled(b"t")), (true, false));
        assert_eq!(
            [b"t:j".as_slice(), b"s:j", b"t"].map(|id| nodes.job_has_registered_template(id)),
            [true, false, false]
        );
        assert_eq!(nodes.delete(b"s"), Some(b"config s".to_vec()));
        assert!(nodes.lock().contains_key(b"s".as_slice()), "a saved node stays");
        assert_eq!(nodes.delete(b"t"), Some(b"config t".to_vec()));
        assert!(!nodes.lock().contains_key(b"t".as_slice()));
        assert_eq!(nodes.delete(b"nope"), None);
    }
}
