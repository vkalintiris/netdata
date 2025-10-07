use crate::error::Result;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::Path;
use tokio::sync::mpsc;

pub struct Monitor {
    /// Set of directories currently being watched
    watched_dirs: HashSet<String>,
    /// The watcher instance
    watcher: RecommendedWatcher,
    /// Channel for events
    event_receiver: mpsc::UnboundedReceiver<Event>,
}

impl Monitor {
    pub fn new() -> Result<Self> {
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
            watcher,
            event_receiver,
        })
    }

    pub fn watch_directory(&mut self, path: &str) -> Result<()> {
        // Start watching with notify
        self.watcher
            .watch(Path::new(path), RecursiveMode::NonRecursive)?;

        // Track the directory
        self.watched_dirs.insert(String::from(path));

        Ok(())
    }

    pub fn unwatch_directory(&mut self, path: &str) -> Result<()> {
        // Stop watching
        self.watcher.unwatch(Path::new(path))?;

        // Remove from tracked set
        self.watched_dirs.remove(path);

        Ok(())
    }

    /// Collect all events from the queue
    pub async fn collect(&mut self, events: &mut Vec<Event>) {
        events.clear();

        while let Ok(event) = self.event_receiver.try_recv() {
            events.push(event);
        }
    }
}
