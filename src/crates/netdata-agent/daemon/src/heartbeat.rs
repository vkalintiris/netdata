//! The shell of the daemon's periodic threads (C's `heartbeat_t` loops): a named thread that wakes on the wall-clock
//! grid of its period until a stop request, with the start and end records every thread writes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The stop request: a flag the work checks without a lock, and the condition the idle thread waits on.
#[derive(Default)]
struct Stop {
    requested: AtomicBool,
    lock: Mutex<()>,
    wake: Condvar,
}

impl Stop {
    fn requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    /// Waits for `wait` unless a stop comes first; whether one did.
    fn wait(&self, wait: Duration) -> bool {
        let guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = self
            .wake
            .wait_timeout_while(guard, wait, |_| !self.requested())
            .unwrap_or_else(PoisonError::into_inner);
        self.requested()
    }

    fn request(&self) {
        self.requested.store(true, Ordering::Release);
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.wake.notify_all();
    }
}

/// `HEARTBEAT_MIN_OFFSET_UT` and `HEARTBEAT_RANDOM_OFFSET_UT`.
const MIN_OFFSET_UT: u64 = 150_000;
const RANDOM_OFFSET_UT: u64 = 350_000;

/// Where in its period a thread wakes.
#[derive(Debug, Clone, Copy)]
pub enum Phase {
    /// On the tick (PULSE sets `hb.randomness = 0`, to stay clear of the other threads).
    OnTheTick,
    /// `heartbeat_randomness()`: a fixed offset of the thread, 150 to 500 ms past the tick.
    Randomized,
}

impl Phase {
    fn offset(self) -> Duration {
        match self {
            Phase::OnTheTick => Duration::ZERO,
            Phase::Randomized => Duration::from_micros(randomness(random_u64(), system_hz())),
        }
    }
}

/// `heartbeat_randomness()`: 150 ms plus up to 350 ms, moved a quarter of a scheduler tick away from one.
fn randomness(hash: u64, hz: u64) -> u64 {
    let mut offset_ut = MIN_OFFSET_UT + hash % RANDOM_OFFSET_UT;
    let scheduler_step_ut = (1_000_000 / hz.max(1)).clamp(1, 10_000);
    if offset_ut % scheduler_step_ut < scheduler_step_ut / 4 {
        offset_ut += scheduler_step_ut / 4;
    }
    offset_ut
}

/// C hashes the thread, the time and the heartbeat's id: any per-thread random value serves.
fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hash, Hasher};
    let mut hasher = std::hash::RandomState::new().build_hasher();
    std::thread::current().id().hash(&mut hasher);
    SystemTime::now().hash(&mut hasher);
    hasher.finish()
}

/// `system_hz` (`os_get_system_HZ()`): the clock ticks per second, 100 when unknown.
fn system_hz() -> u64 {
    use nix::unistd::{SysconfVar, sysconf};
    match sysconf(SysconfVar::CLK_TCK) {
        Ok(Some(ticks)) if ticks > 0 => ticks as u64,
        _ => 100,
    }
}

/// `heartbeat_next()`'s sleep from `now`: to the next tick of the period's wall-clock grid, plus the offset.
fn wait_from(now: Duration, period: Duration, offset: Duration) -> Duration {
    period - Duration::from_nanos((now.as_nanos() % period.as_nanos()) as u64) + offset
}

/// What a periodic thread's body waits on.
pub struct Ticker {
    stop: Arc<Stop>,
    period: Duration,
    offset: Duration,
}

impl Ticker {
    /// `heartbeat_next()`: waits for the next tick of the period's wall-clock grid, plus the thread's offset; false
    /// once a stop came.
    pub fn next(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        !self.stop.wait(wait_from(now, self.period, self.offset))
    }

    /// Sleeps `wait` unless a stop comes first; false once one came.
    pub fn sleep(&self, wait: Duration) -> bool {
        !self.stop.wait(wait)
    }

    /// Whether no stop was requested.
    pub fn running(&self) -> bool {
        !self.stop.requested()
    }
}

pub struct Thread {
    stop: Arc<Stop>,
    thread: JoinHandle<()>,
}

impl Thread {
    /// Starts `name`, whose `body` waits on a ticker of `period` at `phase`.
    pub fn spawn(
        name: &str,
        stack_size: usize,
        period: Duration,
        phase: Phase,
        body: impl FnOnce(&Ticker) + Send + 'static,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(Stop::default());
        let ticker = Ticker {
            stop: Arc::clone(&stop),
            period,
            offset: phase.offset(),
        };
        let thread = std::thread::Builder::new()
            .name(name.into())
            .stack_size(stack_size)
            .spawn(move || {
                netdata_agent_log::thread_created();
                body(&ticker);
                netdata_agent_log::thread_finished();
            })
            .map_err(|err| netdata_agent_evloop::thread_create_failed(name, &err))?;
        Ok(Thread { stop, thread })
    }

    /// `service_wait_exit()` of a thread that leaves on its own once the exit started: waits at most `limit` without
    /// waking it, then asks it to stop and leaves it (not waited for by itself).
    pub fn join_within(self, limit: Duration) {
        if self.thread.thread().id() != std::thread::current().id() {
            let deadline = Instant::now() + limit;
            while Instant::now() < deadline && !self.thread.is_finished() {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        self.stop.request();
        if self.thread.is_finished() {
            let _ = self.thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stop reaches a thread in the middle of its work at once: the flag needs no lock the work holds.
    #[test]
    fn a_stop_is_seen_during_work() {
        let stop = Arc::new(Stop::default());
        let working = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let started = Instant::now();
            while !working.requested() && started.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(1));
            }
            working.requested()
        });
        std::thread::sleep(Duration::from_millis(20));
        let started = Instant::now();
        stop.request();
        assert!(thread.join().unwrap());
        assert!(started.elapsed() < Duration::from_secs(1));
        let waited = Instant::now();
        assert!(stop.wait(Duration::from_secs(10)));
        assert!(
            waited.elapsed() < Duration::from_secs(1),
            "a waiter returns at once"
        );
    }

    /// `heartbeat_next()`'s sleep: to the next tick of the grid (never the current one), plus the offset.
    #[test]
    fn a_ticker_waits_for_the_next_tick_and_its_offset() {
        let second = Duration::from_secs(1);
        let at = |ms: u64| Duration::from_millis(1_790_000_000_000 + ms);
        assert_eq!(wait_from(at(0), second, Duration::ZERO), second);
        assert_eq!(
            wait_from(at(250), second, Duration::ZERO),
            Duration::from_millis(750)
        );
        assert_eq!(
            wait_from(at(100), second, Duration::from_millis(300)),
            Duration::from_millis(1200),
            "an offset still ahead in this second waits for the next one, as C's"
        );
        assert_eq!(
            wait_from(at(500), 2 * second, Duration::ZERO),
            Duration::from_millis(1500)
        );
    }

    /// `heartbeat_randomness()`: 150 to 500 ms, a quarter of a scheduler tick away from one.
    #[test]
    fn randomness_as_c() {
        assert_eq!(randomness(0, 100), 150_000 + 2_500, "on a 10 ms tick");
        assert_eq!(randomness(1_000, 100), 151_000 + 2_500, "near a tick");
        assert_eq!(randomness(5_000, 100), 155_000, "clear of the ticks");
        assert_eq!(randomness(349_999, 100), 499_999);
        assert_eq!(randomness(0, 1000), 150_000 + 250, "a 1 ms tick");
        assert_eq!(randomness(0, 50), 150_000 + 2_500, "at most 10 ms");
        for _ in 0..100 {
            let offset = Phase::Randomized.offset();
            assert!(
                (Duration::from_millis(150)..Duration::from_millis(503)).contains(&offset),
                "{offset:?}"
            );
        }
    }

    /// A join waits for a thread that leaves on its own, and waits out its limit without waking one that does not,
    /// which it then asks to stop (C's service wait).
    #[test]
    fn a_join_waits_without_waking_the_thread() {
        let leaves = Thread::spawn("LEAVES", 64 * 1024, Duration::from_millis(20), Phase::OnTheTick, |t| {
            for _ in 0..3 {
                t.next();
            }
        })
        .unwrap();
        let started = Instant::now();
        leaves.join_within(Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1), "it left on its own");
        let (sender, stopped) = std::sync::mpsc::channel();
        let stays = Thread::spawn("STAYS", 64 * 1024, Duration::from_millis(20), Phase::OnTheTick, move |t| {
            while t.next() {}
            let _ = sender.send(());
        })
        .unwrap();
        let started = Instant::now();
        stays.join_within(Duration::from_millis(200));
        assert!(started.elapsed() >= Duration::from_millis(200), "not woken before the limit");
        stopped.recv_timeout(Duration::from_secs(5)).expect("asked to stop at the limit");
    }
}
