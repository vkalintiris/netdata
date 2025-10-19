use super::error::Result;
use crossbeam_channel::{unbounded, Receiver, Sender};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;

pub struct Monitor {
    /// The watcher instance
    watcher: RecommendedWatcher,
    /// Channel for events
    event_receiver: Receiver<Event>,
}

impl Monitor {
    pub fn new() -> Result<Self> {
        let (event_sender, event_receiver): (Sender<Event>, Receiver<Event>) = unbounded();

        let watcher = RecommendedWatcher::new(
            move |res| {
                if let Ok(event) = res {
                    let _ = event_sender.send(event);
                }
            },
            notify::Config::default(),
        )?;

        Ok(Self {
            watcher,
            event_receiver,
        })
    }

    pub fn watch_directory(&mut self, path: &str) -> Result<()> {
        // Start watching with notify
        self.watcher
            .watch(Path::new(path), RecursiveMode::NonRecursive)?;

        Ok(())
    }

    pub fn unwatch_directory(&mut self, path: &str) -> Result<()> {
        // Stop watching
        self.watcher.unwatch(Path::new(path))?;
        Ok(())
    }

    /// Collect all events from the queue
    pub fn collect(&mut self, events: &mut Vec<Event>) {
        events.clear();

        while let Ok(event) = self.event_receiver.try_recv() {
            events.push(event);
        }
    }
}
