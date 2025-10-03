use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tracing::{debug, error, info, warn};

struct ProcessManager {
    executable_path: PathBuf,
    current_child: Arc<Mutex<Option<Child>>>,
}

impl ProcessManager {
    fn new(executable_path: PathBuf) -> Self {
        Self {
            executable_path,
            current_child: Arc::new(Mutex::new(None)),
        }
    }

    fn spawn_process(&self) -> io::Result<Child> {
        info!("Starting process: {:?}", self.executable_path);

        Command::new(&self.executable_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    }

    fn restart_process(&self) -> io::Result<()> {
        // Kill the existing process if it's running
        self.kill_current()?;

        // Start the new process
        let mut child = self.spawn_process()?;

        // Set up stdout forwarding (binary passthrough)
        if let Some(mut stdout) = child.stdout.take() {
            thread::spawn(move || {
                let mut buffer = [0u8; 8192];
                let mut stdout_writer = io::stdout();

                loop {
                    match stdout.read(&mut buffer) {
                        Ok(0) => break, // EOF
                        Ok(n) => {
                            if let Err(e) = stdout_writer.write_all(&buffer[..n]) {
                                error!("Failed to write to stdout: {}", e);
                                break;
                            }
                            if let Err(e) = stdout_writer.flush() {
                                error!("Failed to flush stdout: {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            error!("Failed to read from child stdout: {}", e);
                            break;
                        }
                    }
                }
                debug!("Child stdout stream ended");
            });
        }

        // Set up stderr forwarding (binary passthrough)
        if let Some(mut stderr) = child.stderr.take() {
            thread::spawn(move || {
                let mut buffer = [0u8; 8192];
                let mut stderr_writer = io::stderr();

                loop {
                    match stderr.read(&mut buffer) {
                        Ok(0) => break, // EOF
                        Ok(n) => {
                            if let Err(e) = stderr_writer.write_all(&buffer[..n]) {
                                error!("Failed to write to stderr: {}", e);
                                break;
                            }
                            if let Err(e) = stderr_writer.flush() {
                                error!("Failed to flush stderr: {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            error!("Failed to read from child stderr: {}", e);
                            break;
                        }
                    }
                }
                debug!("Child stderr stream ended");
            });
        }

        // Store the child process
        *self.current_child.lock().unwrap() = Some(child);

        Ok(())
    }

    fn kill_current(&self) -> io::Result<()> {
        let mut child_guard = self.current_child.lock().unwrap();
        if let Some(mut child) = child_guard.take() {
            info!("Stopping previous process");

            // Try graceful shutdown first
            match child.kill() {
                Ok(_) => debug!("Sent kill signal to child process"),
                Err(e) => warn!("Failed to kill child process: {}", e),
            }

            // Wait for the process to actually exit
            match child.wait() {
                Ok(status) => debug!("Child process exited with: {:?}", status),
                Err(e) => warn!("Failed to wait for child process: {}", e),
            }
        }
        Ok(())
    }

    fn forward_stdin(&self) {
        let child_ref = Arc::clone(&self.current_child);

        thread::spawn(move || {
            let mut stdin = io::stdin();
            let mut buffer = [0u8; 8192];

            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) => {
                        debug!("stdin closed");
                        break;
                    }
                    Ok(n) => {
                        let mut child_guard = child_ref.lock().unwrap();
                        if let Some(ref mut child) = *child_guard {
                            if let Some(ref mut child_stdin) = child.stdin {
                                if let Err(e) = child_stdin.write_all(&buffer[..n]) {
                                    error!("Failed to write to child stdin: {}", e);
                                }
                                if let Err(e) = child_stdin.flush() {
                                    error!("Failed to flush child stdin: {}", e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to read from stdin: {}", e);
                        break;
                    }
                }
            }
        });
    }
}

fn watch_file(path: &Path) -> notify::Result<Receiver<Event>> {
    let (tx, rx) = channel();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default().with_poll_interval(Duration::from_secs(1)),
    )?;

    watcher.watch(path, RecursiveMode::NonRecursive)?;

    // Keep the watcher alive
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(1));
            // Watcher is moved here and kept alive
            let _watcher = &watcher;
        }
    });

    Ok(rx)
}

fn setup_tracing() {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_writer(io::stderr)
                .with_target(false)
                .with_thread_ids(false)
                .with_thread_names(false)
                .with_file(false)
                .with_line_number(false)
                .compact(),
        )
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    setup_tracing();

    let args: Vec<String> = std::env::args().collect();

    if args.len() != 2 {
        error!("Usage: {} <executable_path>", args[0]);
        std::process::exit(1);
    }

    let executable_path = PathBuf::from(&args[1]);

    if !executable_path.exists() {
        error!("Executable not found: {:?}", executable_path);
        std::process::exit(1);
    }

    let manager = ProcessManager::new(executable_path.clone());

    // Start the initial process
    manager.restart_process()?;

    // Set up stdin forwarding
    manager.forward_stdin();

    // Watch for changes
    let rx = watch_file(&executable_path)?;

    info!("Watching for changes to: {:?}", executable_path);
    info!("Press Ctrl+C to exit");

    // Main loop - wait for file change events
    loop {
        match rx.recv() {
            Ok(event) => {
                // Check if this is a modify event
                if matches!(
                    event.kind,
                    notify::EventKind::Modify(_) | notify::EventKind::Create(_)
                ) {
                    info!("Detected change, restarting process...");

                    // Small delay to ensure file write is complete
                    thread::sleep(Duration::from_millis(100));

                    if let Err(e) = manager.restart_process() {
                        error!("Error restarting process: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("Watch error: {}", e);
                break;
            }
        }
    }

    // Cleanup on exit
    manager.kill_current()?;
    Ok(())
}
