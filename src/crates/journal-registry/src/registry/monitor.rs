use super::error::Result;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use tokio::sync::mpsc;

/// File system watcher that sends events through an async channel
#[derive(Debug)]
pub struct Monitor {
    /// The watcher instance
    watcher: RecommendedWatcher,
}

impl Monitor {
    /// Create a new monitor and return it with its event receiver
    pub fn new() -> Result<(Self, mpsc::UnboundedReceiver<Event>)> {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();

        let watcher = RecommendedWatcher::new(
            move |res| {
                if let Ok(event) = res {
                    let _ = event_sender.send(event);
                }
            },
            notify::Config::default(),
        )?;

        Ok((Self { watcher }, event_receiver))
    }

    /// Start watching a directory for file system events
    pub fn watch_directory(&mut self, path: &str) -> Result<()> {
        self.watcher
            .watch(Path::new(path), RecursiveMode::Recursive)?;

        Ok(())
    }

    /// Stop watching a directory
    pub fn unwatch_directory(&mut self, path: &str) -> Result<()> {
        self.watcher.unwatch(Path::new(path))?;
        Ok(())
    }
}
