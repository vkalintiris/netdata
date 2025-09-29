#![allow(unused_imports, dead_code)]

use notify::{
    Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{ModifyKind, RenameMode},
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, mpsc};

pub struct Monitor {
    /// Set of directories currently being watched
    watched_dirs: Arc<RwLock<HashSet<String>>>,
    /// The watcher instance
    watcher: Arc<Mutex<RecommendedWatcher>>,
    /// Channel for events
    event_receiver: Arc<Mutex<mpsc::UnboundedReceiver<Event>>>,
}

impl Monitor {
    pub fn new() -> Result<Self, notify::Error> {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();

        let watcher = RecommendedWatcher::new(
            move |res| {
                if let Ok(event) = res {
                    let _ = event_sender.send(event);
                }
            },
            notify::Config::default(),
        )?;

        Ok(Self {
            watched_dirs: Default::default(),
            watcher: Arc::new(Mutex::new(watcher)),
            event_receiver: Arc::new(Mutex::new(event_receiver)),
        })
    }

    pub async fn watch_directory(&self, path: &str) -> Result<(), notify::Error> {
        // Start watching with notify
        self.watcher
            .lock()
            .await
            .watch(Path::new(path), RecursiveMode::NonRecursive)?;

        // Track the directory
        self.watched_dirs.write().await.insert(String::from(path));

        Ok(())
    }

    pub async fn unwatch_directory(&self, path: &str) -> Result<(), notify::Error> {
        // Stop watching
        self.watcher.lock().await.unwatch(Path::new(path))?;

        // Remove from tracked set
        self.watched_dirs.write().await.remove(path);

        Ok(())
    }

    /// Collect all events from the queue
    pub async fn collect(&self, events: &mut Vec<Event>) {
        events.clear();

        let mut receiver = self.event_receiver.lock().await;
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }
    }
}
