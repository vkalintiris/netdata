//! The replication sender (`src/streaming/stream-replication-sender.c`): the parents' `REPLAY_CHART` requests,
//! one per chart and sender, ordered by their `after` across every sender (D105.2), and the threads that answer them,
//! `REPLAY[1]`, a static thread, and its workers `REPLAY[2..threads]`, each taking the earliest request at a time
//! (D105.3: C with `[db] replication prefetch = 1`). The queue lives in the connector (D111.1); the answer is
//! `rrd::upstream::replay` (D111.5). Map: `knowledge/map-m7-commit6-replication.md` §2, §3 in the status repository.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_rrd::chart::flags;
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::stream_control;
use netdata_agent_rrd::upstream::replay::{self, Answered, Request};

use crate::sender::Sender;

/// `MAX_REPLICATION_MESSAGE_PERCENT_SENDER_BUFFER`: an answer is cut past this share of the sender's buffer.
const MAX_MESSAGE_PERCENT_OF_BUFFER: usize = 25;
/// `MAX_SENDER_BUFFER_PERCENTAGE_ALLOWED` and `MIN_SENDER_BUFFER_PERCENTAGE_ALLOWED`: a sender's requests wait
/// unindexed while its buffer is used above the first, until it drops below the second.
const PARK_ABOVE_PERCENT: usize = 50;
const UNPARK_BELOW_PERCENT: usize = 10;
/// `ITERATIONS_IDLE_WITHOUT_PENDING_TO_RUN_SENDER_VERIFICATION`: statistics ticks after the last execution before the
/// main thread slows down again.
const IDLE_TICKS: i64 = 30;

/// A sender's side of the queue (`s->replication`): its key, and what its stream thread and the collectors read
/// without the queue's lock.
#[derive(Debug)]
pub(crate) struct SenderQueue {
    id: u64,
    /// `pending_requests`: its requests waiting to be picked.
    pending_requests: AtomicUsize,
    /// `charts_replicating`: its requests in the queue, picked or not.
    charts_replicating: AtomicUsize,
    /// `stream_circular_buffer_last_flush_ut()` mirrored where the buffer is flushed (D111.4): a request remembers it,
    /// and an answer goes out only into the session it was asked in.
    pub(crate) last_flush_ut: AtomicU64,
    /// `reached_max`: its buffer is too full for answers; set and cleared under the queue's lock.
    parked: AtomicBool,
    /// `s->replication.oldest_request_after_t`: the earliest start asked since its requests were last deleted; 0 when
    /// none (a request that asks from 0 unsets it, as C's).
    oldest_request_after_s: AtomicI64,
    /// `s->replication.latest_completed_before_t`: the end of the last of its queries that ran (not the largest).
    latest_completed_before_s: AtomicI64,
}

impl SenderQueue {
    pub(crate) fn new() -> Arc<SenderQueue> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Arc::new(SenderQueue {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            pending_requests: AtomicUsize::new(0),
            charts_replicating: AtomicUsize::new(0),
            last_flush_ut: AtomicU64::new(0),
            parked: AtomicBool::new(false),
            oldest_request_after_s: AtomicI64::new(0),
            latest_completed_before_s: AtomicI64::new(0),
        })
    }

    /// The two stamps the host's status computes the replication's completion from: the oldest start asked, the
    /// latest end answered.
    pub(crate) fn stamps(&self) -> (i64, i64) {
        (self.oldest_request_after_s.load(Ordering::Relaxed), self.latest_completed_before_s.load(Ordering::Relaxed))
    }

    /// The latest end answered, forgotten with the sender at its free (C frees the struct that holds it).
    pub(crate) fn latest_completed_zero(&self) {
        self.latest_completed_before_s.store(0, Ordering::Relaxed);
    }

    /// `stream_sender_pending_replication_requests()` or `stream_sender_replicating_charts()`: the idle check waits.
    pub(crate) fn busy(&self) -> bool {
        self.pending_requests.load(Ordering::Relaxed) != 0 || self.charts_replicating.load(Ordering::Relaxed) != 0
    }

    /// `dictionary_entries(s->replication.requests) != 0`: some request is in the queue.
    pub(crate) fn queued(&self) -> bool {
        self.charts_replicating.load(Ordering::Relaxed) != 0
    }

    /// `stream_sender_replicating_charts_zero()`.
    pub(crate) fn replicating_zero(&self) {
        self.charts_replicating.store(0, Ordering::Relaxed);
    }
}

/// A request's place (`indexed_in_judy`, `not_indexed_buffer_full`, `not_indexed_preprocessing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Waiting under its unique id.
    Indexed(u64),
    /// Its sender's buffer is too full: indexed again once it drains.
    Parked,
    /// Picked by a thread; it leaves the queue when that thread executes it (one that was dropped for a full buffer
    /// is indexed again with the parked ones).
    Picked,
}

#[derive(Debug)]
struct Entry {
    after: i64,
    before: i64,
    start_streaming: bool,
    /// The sender's buffer flush when the request came (`sender_circular_buffer_last_flush_ut`).
    flush_ut: u64,
    place: Place,
    /// Its arrival among all requests: C's dictionary walks a sender's requests in this order.
    arrival: u64,
}

/// One sender's requests (`s->replication.requests`), by chart.
#[derive(Debug)]
struct Requests {
    sender: Weak<Sender>,
    queue: Arc<SenderQueue>,
    charts: HashMap<String, Entry>,
}

/// The order and its counters (`replication_globals.unsafe`).
#[derive(Debug, Default)]
struct Order {
    /// The waiting requests by (after, unique id), to their sender and chart: every pick takes the first, C's
    /// global minimum (the map's §2.1).
    index: BTreeMap<(i64, u64), (u64, String)>,
    unique_id: u64,
    /// Requests added so far, which orders a sender's requests as they came.
    arrivals: u64,
    pending: usize,
    added: usize,
    removed: usize,
    /// Requests parked.
    pending_no_room: usize,
    /// Senders parked now.
    senders_full: usize,
    /// Unparks so far, which speed up the main thread's pace.
    sender_resets: usize,
}

impl Order {
    /// `replication_sort_entry_add()`: indexed, unless its sender is parked.
    fn add(&mut self, sender: &SenderQueue, chart: &str, e: &mut Entry) {
        if sender.parked.load(Ordering::Relaxed) {
            e.place = Place::Parked;
            self.pending_no_room += 1;
            return;
        }
        if e.place == Place::Parked {
            self.pending_no_room -= 1;
        }
        self.unique_id += 1;
        self.index.insert((e.after, self.unique_id), (sender.id, chart.to_string()));
        e.place = Place::Indexed(self.unique_id);
        sender.pending_requests.fetch_add(1, Ordering::Relaxed);
        self.added += 1;
        self.pending += 1;
    }

    /// `replication_sort_entry_unlink_and_free_unsafe()`.
    fn unlink(&mut self, sender: &SenderQueue, after: i64, uid: u64) {
        self.index.remove(&(after, uid));
        sender.pending_requests.fetch_sub(1, Ordering::Relaxed);
        self.removed += 1;
        self.pending -= 1;
    }

    /// `replication_request_delete_callback()`'s part: a request leaving the queue leaves the order.
    fn forget(&mut self, sender: &SenderQueue, e: &Entry) {
        match e.place {
            Place::Indexed(uid) => self.unlink(sender, e.after, uid),
            Place::Parked => self.pending_no_room -= 1,
            Place::Picked => {}
        }
    }
}

#[derive(Debug, Default)]
struct State {
    order: Order,
    senders: HashMap<u64, Requests>,
}

impl State {
    /// `dictionary_del()` of the request a thread took (`replication_request_delete_callback()`): only one of its
    /// session, not a newer request for the chart that came after a reconnect; unlinked if it was indexed again
    /// meanwhile (an unpark), so no other thread answers it later. One that another thread took again in between is
    /// answered twice, as C's with one prefetch slot.
    fn delete_taken(&mut self, sender_id: u64, chart: &str, flush_ut: u64) {
        let Some(requests) = self.senders.get_mut(&sender_id) else {
            return;
        };
        if requests.charts.get(chart).is_some_and(|e| e.flush_ut == flush_ut)
            && let Some(e) = requests.charts.remove(chart)
        {
            requests.queue.charts_replicating.fetch_sub(1, Ordering::Relaxed);
            self.order.forget(&requests.queue, &e);
        }
        if requests.charts.is_empty() {
            self.senders.remove(&sender_id);
        }
    }

    /// `dictionary_flush()` of a sender's requests: how many there were.
    fn flush(&mut self, sender_id: u64) -> usize {
        let Some(requests) = self.senders.remove(&sender_id) else {
            return 0;
        };
        let n = requests.charts.len();
        for e in requests.charts.values() {
            requests.queue.charts_replicating.fetch_sub(1, Ordering::Relaxed);
            self.order.forget(&requests.queue, e);
        }
        n
    }
}

/// A request a thread took, with its sender.
#[derive(Debug)]
struct Taken {
    sender: Weak<Sender>,
    sender_id: u64,
    request: Request,
    flush_ut: u64,
}

/// `replication_globals`: the requests of every sender, and what the threads count.
#[derive(Debug, Default)]
pub struct Queue {
    state: Mutex<State>,
    received: AtomicUsize,
    executed: AtomicUsize,
    replied: AtomicUsize,
    not_found: AtomicUsize,
    duplicate: AtomicUsize,
    flushed: AtomicUsize,
    /// `latest_first_time`: the `after` of the last request taken.
    latest_first_time: AtomicI64,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `replication_sender_request_add()`: a new chart's request waits in the order (parked with its sender); one for
    /// a chart already queued is a duplicate, which a parked request takes its window from and any other ignores
    /// (C's conflict callback); an empty chart id is refused, unanswered, as C's dictionary refuses it.
    pub(crate) fn request_add(&self, sender: &Arc<Sender>, chart: String, after: i64, before: i64, start: bool) {
        self.add(Arc::downgrade(sender), sender.replication(), chart, after, before, start);
    }

    fn add(&self, sender: Weak<Sender>, queue: &Arc<SenderQueue>, chart: String, after: i64, before: i64, start: bool) {
        let flush_ut = queue.last_flush_ut.load(Ordering::Relaxed);
        // lowered before the dictionary is asked, so a duplicate and a refused empty chart id count too
        // (stream-replication-sender.c:1312-1313)
        let _ = queue.oldest_request_after_s.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |oldest| {
            (oldest == 0 || after < oldest).then_some(after)
        });
        if !chart.is_empty() {
            let mut state = self.lock();
            let State { order, senders } = &mut *state;
            let requests = senders.entry(queue.id).or_insert_with(|| Requests {
                sender,
                queue: Arc::clone(queue),
                charts: HashMap::new(),
            });
            match requests.charts.entry(chart) {
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    // replication_request_conflict_callback(): a parked request takes the newer window; one indexed or
                    // picked stays as it is
                    self.duplicate.fetch_add(1, Ordering::Relaxed);
                    let e = slot.get_mut();
                    if e.place == Place::Parked {
                        (e.after, e.before, e.start_streaming) = (after, before, start);
                    }
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    order.arrivals += 1;
                    let mut e = Entry {
                        after,
                        before,
                        start_streaming: start,
                        flush_ut,
                        place: Place::Picked,
                        arrival: order.arrivals,
                    };
                    order.add(queue, slot.key(), &mut e);
                    slot.insert(e);
                    queue.charts_replicating.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        self.received.fetch_add(1, Ordering::Relaxed);
    }

    /// `replication_sender_delete_pending_requests()`, at the sender's every connection and disconnection.
    pub(crate) fn delete_pending(&self, queue: &SenderQueue) {
        let n = self.lock().flush(queue.id);
        self.flushed.fetch_add(n, Ordering::Relaxed);
        queue.oldest_request_after_s.store(0, Ordering::Relaxed);
    }

    /// `replication_sender_recalculate_buffer_used_ratio_unsafe()`, under the sender's buffer lock after each commit,
    /// each send and the flush at dispatch: past half its buffer a sender's waiting requests are parked; below a
    /// tenth they, and those dropped at their pick meanwhile, are indexed again.
    pub(crate) fn recalculate(&self, queue: &SenderQueue, used_percent: usize) {
        let park = used_percent > PARK_ABOVE_PERCENT;
        if (!park && used_percent >= UNPARK_BELOW_PERCENT) || park == queue.parked.load(Ordering::Relaxed) {
            return;
        }
        let mut state = self.lock();
        let State { order, senders } = &mut *state;
        // under the lock, as C's dictionary walk excludes the adds
        if queue.parked.swap(park, Ordering::Relaxed) == park {
            return;
        }
        let charts = senders.get_mut(&queue.id).map(|r| &mut r.charts);
        if park {
            for e in charts.into_iter().flat_map(|c| c.values_mut()) {
                if let Place::Indexed(uid) = e.place {
                    order.unlink(queue, e.after, uid);
                    e.place = Place::Parked;
                    order.pending_no_room += 1;
                }
            }
            order.senders_full += 1;
        } else {
            let mut unindexed: Vec<(&String, &mut Entry)> = charts
                .into_iter()
                .flat_map(|c| c.iter_mut())
                .filter(|(_, e)| !matches!(e.place, Place::Indexed(_)))
                .collect();
            // in the order they came, as C's dictionary walk
            unindexed.sort_by_key(|(_, e)| e.arrival);
            for (chart, e) in unindexed {
                order.add(queue, chart, e);
            }
            order.senders_full -= 1;
            order.sender_resets += 1;
        }
    }

    /// `replication_request_get_first_available()`: the earliest request of all, picked.
    fn take(&self) -> Option<Taken> {
        let mut state = self.lock();
        let State { order, senders } = &mut *state;
        let ((after, uid), (sender_id, chart)) = order.index.first_key_value().map(|(k, v)| (*k, v.clone()))?;
        let Some(requests) = senders.get_mut(&sender_id) else {
            // every indexed request has its sender's entry: never reached
            order.index.remove(&(after, uid));
            return None;
        };
        order.unlink(&requests.queue, after, uid);
        let e = requests.charts.get_mut(&chart)?;
        e.place = Place::Picked;
        let request = Request { chart_id: chart, after: e.after, before: e.before, start_streaming: e.start_streaming };
        Some(Taken { sender: requests.sender.clone(), sender_id, request, flush_ut: e.flush_ut })
    }

    /// `replication_pipeline_execute_next()` with one slot: the earliest request answered, unless its sender's buffer
    /// was flushed since it came (the parent asks again); false when there was none to answer.
    fn execute_next(&self, buffer: &mut Vec<u8>) -> bool {
        let Some(taken) = self.take() else {
            return false;
        };
        let Some(sender) = taken.sender.upgrade() else {
            // the host was freed with its sender: nothing asks for its requests any more
            self.lock().flush(taken.sender_id);
            return false;
        };
        // stays picked until the sender's reset flushes it, as C's
        if sender.replication().last_flush_ut.load(Ordering::Relaxed) != taken.flush_ut {
            return false;
        }
        // indexed again when its sender unparks
        if sender.replication().parked.load(Ordering::Relaxed) {
            return false;
        }
        self.lock().delete_taken(taken.sender_id, &taken.request.chart_id, taken.flush_ut);
        self.latest_first_time.store(taken.request.after, Ordering::Relaxed);
        let Some(host) = sender.host() else {
            return true;
        };
        let capabilities = sender.negotiated.load(Ordering::Relaxed);
        let max_msg_size = sender.out().buffer.stats().bytes_max_size * MAX_MESSAGE_PERCENT_OF_BUFFER / 100;
        let mut committed = false;
        let answered = replay::answer(&host, &taken.request, capabilities, max_msg_size, buffer, |bytes| {
            committed = sender.commit_replication(bytes, taken.flush_ut);
            committed
        });
        // an answer dropped for a newer session is not one it got (D105.6)
        if committed {
            sender.counter_out.fetch_add(1, Ordering::Relaxed);
            self.replied.fetch_add(1, Ordering::Relaxed);
        }
        match answered {
            Answered::NotFound => self.not_found.fetch_add(1, Ordering::Relaxed),
            Answered::Executed(end) => {
                if let Some(before_s) = end {
                    sender.replication().latest_completed_before_s.store(before_s, Ordering::Relaxed);
                }
                self.executed.fetch_add(1, Ordering::Relaxed)
            }
        };
        true
    }

    /// Nothing waits (`pending` and `pending_no_room` both 0).
    fn idle(&self) -> bool {
        let state = self.lock();
        state.order.pending == 0 && state.order.pending_no_room == 0
    }

    /// `pending` and `sender_resets`, for the main thread's pace.
    fn pace(&self) -> (usize, usize) {
        let state = self.lock();
        (state.order.pending, state.order.sender_resets)
    }
}

/// Every host (`rrdhost_root_index`), for the summary.
pub type AllHosts = Arc<dyn Fn() -> Vec<Arc<Host>> + Send + Sync>;

/// `replication_globals.main_thread.last_*`: the counters at the last summary.
#[derive(Debug, Default)]
struct SummaryBase {
    received: usize,
    executed: usize,
    replied: usize,
    not_found: usize,
    duplicate: usize,
    flushed: usize,
}

/// `verify_host_charts_are_streaming_now()`: a host's charts that stream (not obsolete, not filtered out) and have
/// not finished their replication, or are still in it.
fn charts_waiting_parent(host: &Host) -> usize {
    host.charts()
        .all()
        .iter()
        .map(|chart| chart.flags())
        .filter(|f| f & (flags::OBSOLETE | flags::UPSTREAM_IGNORE) == 0)
        .filter(|f| f & flags::SENDER_REPLICATION_FINISHED == 0 || f & flags::SENDER_REPLICATION_IN_PROGRESS != 0)
        .count()
}

/// `verify_all_hosts_charts_are_streaming_now()`: once nothing waits, what the senders did since the last summary.
fn summary(queue: &Queue, hosts: &[Arc<Host>], base: &mut SummaryBase) {
    let waiting: usize = hosts.iter().map(|h| charts_waiting_parent(h)).sum();
    let pending: usize = queue.lock().senders.values().map(|r| r.charts.len()).sum();
    let load = |c: &AtomicUsize| c.load(Ordering::Relaxed);
    let now = SummaryBase {
        received: load(&queue.received),
        executed: load(&queue.executed),
        replied: load(&queue.replied),
        not_found: load(&queue.not_found),
        duplicate: load(&queue.duplicate),
        flushed: load(&queue.flushed),
    };
    let mut parts = Vec::new();
    if pending != 0 {
        parts.push(format!("{pending} requests pending"));
    }
    if waiting != 0 {
        parts.push(format!("{waiting} instances waiting parent"));
    }
    for (n, what) in [
        (now.not_found - base.not_found, "ignored-not-found"),
        (now.duplicate - base.duplicate, "ignored-merged"),
        (now.flushed - base.flushed, "were flushed"),
    ] {
        if n != 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    nd_log!(
        Source::Daemon,
        Priority::Notice,
        "REPLICATION SEND SUMMARY: all senders finished replication. Received {}, executed {} and replied to {} \
         requests. {}",
        now.received - base.received,
        now.executed - base.executed,
        now.replied - base.replied,
        parts.join(", ")
    );
    *base = now;
}

/// `service_running(SERVICE_REPLICATION)`, with a wakeup for the wait.
#[derive(Debug, Default)]
struct Stop {
    stopped: Mutex<bool>,
    cv: Condvar,
}

impl Stop {
    /// Waits up to `d`; false once stopped.
    fn wait(&self, d: Duration) -> bool {
        let stopped = self.stopped.lock().unwrap_or_else(PoisonError::into_inner);
        let (stopped, _) =
            self.cv.wait_timeout_while(stopped, d, |s| !*s).unwrap_or_else(PoisonError::into_inner);
        !*stopped
    }

    fn request(&self) {
        *self.stopped.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.cv.notify_all();
    }
}

fn spawn(name: String, stack_size: usize, body: impl FnOnce() + Send + 'static) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name.clone())
        .stack_size(stack_size)
        .spawn(move || {
            netdata_agent_log::thread_created();
            body();
            netdata_agent_log::thread_finished();
        })
        .map_err(|err| netdata_agent_evloop::thread_create_failed(&name, &err))
}

/// `replication_worker_thread()`: one request at a time, a second's sleep (its buffer freed) when none waits.
fn worker(queue: &Queue, stop: &Stop, exiting: fn() -> bool) {
    let mut buffer = Vec::new();
    while !exiting() {
        if !stream_control::replication_should_be_running() {
            if !stop.wait(stream_control::throttle_wait()) {
                return;
            }
            continue;
        }
        if !queue.execute_next(&mut buffer) {
            buffer = Vec::new();
            if !stop.wait(Duration::from_secs(1)) {
                return;
            }
        }
    }
}

/// `replication_thread_main()`'s loop: the same work as a worker's, at a pace set each `update_every` seconds by
/// whether requests were executed: 10 ms between empty rounds for 30 ticks after an execution (100 ms or a second
/// while requests wait), then, once nothing waits, the summary and a second.
fn main_loop(queue: &Queue, stop: &Stop, exiting: fn() -> bool, update_every: Duration, hosts: &AllHosts) {
    let mut base = SummaryBase::default();
    let mut buffer = Vec::new();
    let mut countdown = i64::MAX;
    let mut slow = true;
    let mut last_tick = Instant::now();
    let mut last_executed = 0;
    let mut last_sender_resets = 0;
    while !exiting() {
        if !stream_control::replication_should_be_running() {
            if !stop.wait(stream_control::throttle_wait()) {
                return;
            }
            continue;
        }
        if last_tick.elapsed() > update_every {
            last_tick = Instant::now();
            let executed = queue.executed.load(Ordering::Relaxed);
            if executed != last_executed {
                countdown = IDLE_TICKS;
                last_executed = executed;
                slow = false;
            }
            countdown -= 1;
            if countdown == 0 {
                if queue.idle() {
                    queue.latest_first_time.store(0, Ordering::Relaxed);
                    summary(queue, &hosts(), &mut base);
                    countdown = i64::MAX;
                    slow = true;
                } else {
                    countdown = IDLE_TICKS;
                }
            }
        }
        if !queue.execute_next(&mut buffer) {
            if slow {
                buffer = Vec::new();
            }
            let (pending, sender_resets) = queue.pace();
            let timeout = idle_wait(slow, pending, sender_resets, &mut last_sender_resets);
            if !stop.wait(timeout) {
                return;
            }
        }
    }
}

/// The main thread's wait after an empty round (`replication_thread_main()`): a second while slow; while requests
/// wait, 100 ms after an unpark since the last empty round, else a second; 10 ms when none waits. The unparks are
/// remembered except while slow, as C's.
fn idle_wait(slow: bool, pending: usize, sender_resets: usize, last_sender_resets: &mut usize) -> Duration {
    if slow {
        return Duration::from_secs(1);
    }
    let unparked = sender_resets != *last_sender_resets;
    *last_sender_resets = sender_resets;
    if pending == 0 {
        Duration::from_millis(10)
    } else if unparked {
        Duration::from_millis(100)
    } else {
        Duration::from_secs(1)
    }
}

/// The running replication threads.
#[derive(Debug)]
pub struct ReplicationThreads {
    stop: Arc<Stop>,
    main: JoinHandle<()>,
}

impl ReplicationThreads {
    /// `replication_thread_main()`: `REPLAY[1]` starts `threads - 1` workers, and joins them when it ends; all answer
    /// the requests of `queue`, the main thread's pace ticking every `update_every` seconds (`nd_profile`'s), its
    /// summary over `hosts`.
    pub fn spawn(
        threads: usize,
        stack_size: usize,
        queue: Arc<Queue>,
        update_every: Duration,
        hosts: AllHosts,
    ) -> io::Result<ReplicationThreads> {
        Self::spawn_with(threads, stack_size, queue, update_every, hosts, netdata_agent_sys::exit::initiated)
    }

    /// [`ReplicationThreads::spawn`] with `exiting` telling the exit's start (`service_running(SERVICE_REPLICATION)`,
    /// tested at the top of each loop).
    fn spawn_with(
        threads: usize,
        stack_size: usize,
        queue: Arc<Queue>,
        update_every: Duration,
        hosts: AllHosts,
        exiting: fn() -> bool,
    ) -> io::Result<ReplicationThreads> {
        let stop = Arc::new(Stop::default());
        let main_stop = Arc::clone(&stop);
        let main = spawn("REPLAY[1]".to_string(), stack_size, move || {
            let workers: Vec<JoinHandle<()>> = (2..=threads)
                .filter_map(|n| {
                    let (stop, queue) = (Arc::clone(&main_stop), Arc::clone(&queue));
                    // nd_thread_create() failures leave the slot empty, as C carries on
                    spawn(format!("REPLAY[{n}]"), stack_size, move || worker(&queue, &stop, exiting)).ok()
                })
                .collect();
            main_loop(&queue, &main_stop, exiting, update_every, &hosts);
            for worker in workers {
                let _ = worker.join();
            }
        })?;
        Ok(ReplicationThreads { stop, main })
    }

    /// `service_wait_exit(SERVICE_REPLICATION, limit)` once the exit started: the threads leave on their own and are
    /// waited for at most `limit` without being woken (not by one of them); then they are asked to stop and left.
    pub fn join_within(self, limit: Duration) {
        if self.main.thread().id() != std::thread::current().id() {
            let deadline = Instant::now() + limit;
            while Instant::now() < deadline && !self.main.is_finished() {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        self.stop.request();
        if self.main.is_finished() {
            let _ = self.main.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    fn no_hosts() -> AllHosts {
        Arc::new(Vec::new)
    }

    /// Before the exit starts a join waits out its limit, then asks the threads to stop.
    #[test]
    fn a_join_waits_its_limit_then_asks() {
        let threads =
            ReplicationThreads::spawn_with(3, 256 * 1024, Arc::default(), SECOND, no_hosts(), || false).unwrap();
        let started = Instant::now();
        threads.join_within(Duration::from_millis(200));
        assert!(started.elapsed() >= Duration::from_millis(200));
    }

    /// The exit's start ends them within their second's wait, before any stop.
    #[test]
    fn the_threads_end_when_the_exit_starts() {
        let threads =
            ReplicationThreads::spawn_with(3, 256 * 1024, Arc::default(), SECOND, no_hosts(), || true).unwrap();
        let started = Instant::now();
        while !threads.main.is_finished() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(threads.main.is_finished() && started.elapsed() < Duration::from_millis(1500));
    }

    /// `stream_control_replication_should_be_running()` gates every pass of REPLAY[1] and of each worker: a request
    /// waits while a user data query runs and is executed once it ends.
    #[test]
    fn replication_waits_while_a_user_query_runs() {
        let q = Arc::new(Queue::default());
        let sender = SenderQueue::new();
        q.add(Weak::new(), &sender, "c".into(), 10, 20, false);
        let query = netdata_agent_rrd::stream_control::UserDataQuery::start();
        let threads =
            ReplicationThreads::spawn_with(2, 256 * 1024, Arc::clone(&q), SECOND, no_hosts(), || false).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(requests(&q), [(10, "c".into())]);
        drop(query);
        let started = Instant::now();
        while !(q.idle() && requests(&q).is_empty()) && started.elapsed() < 3 * SECOND {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(q.idle() && requests(&q).is_empty());
        threads.join_within(Duration::ZERO);
    }

    /// A request asked before its sender's buffer was last flushed (a reconnect since) is dropped at its pick: not
    /// answered and not counted, left picked until the sender's reset flushes it; one asked since is answered.
    #[test]
    fn a_request_from_before_the_last_flush_is_dropped_at_its_pick() {
        use crate::connector::tests::{connector, info};
        let (_pool, c) = connector();
        let host = Arc::new(Host::new("5a1e0000-0000-4000-8000-0000000000c7", false, info("127.0.0.1:1", "k")));
        let s = Sender::attach(&host, &c).expect("created");
        let q = Queue::default();
        let counted = |q: &Queue| [&q.executed, &q.replied, &q.not_found, &q.flushed].map(|n| n.load(Ordering::Relaxed));
        let held = |s: &Sender| {
            let r = s.replication();
            (r.pending_requests.load(Ordering::Relaxed), r.charts_replicating.load(Ordering::Relaxed))
        };
        q.request_add(&s, "c".into(), 10, 20, false);
        s.replication().last_flush_ut.store(7, Ordering::Relaxed);
        assert!(!q.execute_next(&mut Vec::new()));
        assert!(requests(&q).is_empty());
        assert_eq!(counted(&q), [0, 0, 0, 0]);
        assert_eq!(held(&s), (0, 1), "picked, still the sender's");
        q.request_add(&s, "d".into(), 30, 40, false);
        assert!(q.execute_next(&mut Vec::new()));
        assert_eq!(counted(&q), [0, 0, 1, 0], "answered: no chart d");
        assert_eq!(held(&s), (0, 1));
        q.delete_pending(s.replication());
        assert_eq!(counted(&q), [0, 0, 1, 1]);
        assert!(!s.replication().busy());
    }

    /// The main thread's pace after an empty round: (slow, pending, sender resets, the resets last seen) to the wait
    /// and the resets seen after it.
    #[test]
    fn the_main_threads_idle_wait_follows_its_pace() {
        let ms = Duration::from_millis;
        let cases = [
            ("slow: a second, the unpark not taken", (true, 3, 5, 1), (ms(1000), 1)),
            ("waiting, no unpark", (false, 3, 1, 1), (ms(1000), 1)),
            ("waiting, an unpark", (false, 3, 2, 1), (ms(100), 2)),
            ("none waiting", (false, 0, 4, 1), (ms(10), 4)),
        ];
        for (name, (slow, pending, resets, mut last), want) in cases {
            let wait = idle_wait(slow, pending, resets, &mut last);
            assert_eq!((wait, last), want, "{name}");
        }
    }

    /// The answers' wall clock.
    const T: i64 = 1_700_000_000;

    /// An answer is cut once past a quarter of its sender's buffer maximum (`MAX_REPLICATION_MESSAGE_PERCENT_SENDER_
    /// BUFFER`): at exactly a quarter it takes one more step.
    #[test]
    fn an_answer_is_cut_at_a_quarter_of_the_senders_buffer() {
        use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
        use netdata_agent_rrd::mode::DbMode;
        let (_pool, c) = crate::connector::tests::connector();
        let host = Arc::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000d1",
            false,
            crate::connector::tests::info("127.0.0.1:1", "key"),
        ));
        let s = Sender::attach(&host, &c).expect("created");
        let chart = host
            .charts()
            .create(&ChartSpec {
                type_: "t",
                id: "a",
                name: None,
                family: Some("fam"),
                context: Some("t.a"),
                title: "Title",
                units: "u",
                plugin: "p",
                module: None,
                priority: 1000,
                update_every: 1,
                chart_type: ChartType::Line,
                mode: DbMode::Ram,
                history_entries: 3600,
                page_size: 4096,
            })
            .0;
        let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        for t in T..T + 10 {
            dim.store_metric(t as u64 * 1_000_000, (t - T) as f64, 0);
        }
        dim.set_exposed_upstream(chart.version());
        assert!(dim.is_sent_upstream());
        let q = c.replication();
        let answer = |max_size: usize| -> Vec<String> {
            s.out().buffer.set_max_size(max_size, true);
            q.request_add(&s, "t.a".into(), T + 2, T + 8, false);
            let mut buf = Vec::new();
            assert!(q.execute_next(&mut buf));
            String::from_utf8(buf).unwrap().lines().map(str::to_string).collect()
        };
        // steps and the REND's verdict and window
        let shape = |lines: &[String]| -> (usize, Vec<String>) {
            let steps = lines.iter().filter(|l| l.starts_with("RBEGIN '' ")).count();
            let rend: Vec<String> = lines.last().unwrap().split(' ').skip(4).take(3).map(str::to_string).collect();
            (steps, rend)
        };
        let full = answer(1 << 30);
        assert_eq!(full[1], format!("RBEGIN '' {} {} {}", T + 2, T + 3, full[1].rsplit(' ').next().unwrap()));
        assert_eq!(full[2], "RSET \"d\" 3 ''");
        assert_eq!(shape(&full), (6, vec!["false".to_string(), (T + 2).to_string(), (T + 8).to_string()]));
        // the sender's latest completed end is each query's own, as REND prints it: the last one's, not the largest
        assert_eq!(s.replication().stamps().1, T + 8);
        // and the sender's status reads both stamps in their places
        let (oldest, latest) = s.replication().stamps();
        assert_ne!(oldest, latest);
        let status = netdata_agent_rrd::upstream::Upstream::status(&*s);
        assert_eq!((status.oldest_request_after_s, status.latest_completed_before_s), (oldest, latest));
        // the chart's line and one step
        let one_step: usize = full[..3].iter().map(|l| l.len() + 1).sum();
        let at = answer(4 * one_step);
        assert_eq!(shape(&at), (2, vec!["false".to_string(), (T + 2).to_string(), (T + 4).to_string()]));
        let below = answer(4 * one_step - 1);
        assert_eq!(shape(&below), (1, vec!["false".to_string(), (T + 2).to_string(), (T + 3).to_string()]));
        assert_eq!(s.replication().stamps().1, T + 3);
        // an answer that runs no query leaves it: one for a chart that is not there, and one asked without a window
        // (the start of streaming), which answers the chart without querying it
        q.request_add(&s, "no.such".into(), T + 2, T + 8, false);
        let _ = netdata_agent_log::capture(|| assert!(q.execute_next(&mut Vec::new())));
        assert_eq!(s.replication().stamps().1, T + 3);
        s.out().buffer.set_max_size(1 << 30, true);
        q.request_add(&s, "t.a".into(), 0, 0, false);
        let mut buf = Vec::new();
        let _ = netdata_agent_log::capture(|| assert!(q.execute_next(&mut buf)));
        assert!(String::from_utf8_lossy(&buf).contains("REND"), "{}", String::from_utf8_lossy(&buf));
        assert_eq!(s.replication().stamps().1, T + 3);
        // a free forgets both stamps with the sender (C frees the struct that holds them)
        s.replication().latest_completed_before_s.store(T + 5, Ordering::Relaxed);
        s.replication().oldest_request_after_s.store(T + 1, Ordering::Relaxed);
        netdata_agent_rrd::upstream::Upstream::free(&*s);
        assert_eq!(s.replication().stamps(), (0, 0));
    }

    /// A replication answer goes only into the session it was asked in: once the sender's buffer was flushed since,
    /// the commit refuses it (and the answer finishes nothing); with no session at all it goes nowhere but counts.
    #[test]
    fn an_answer_goes_only_into_the_session_it_was_asked_in() {
        let (_pool, c) = crate::connector::tests::connector();
        let host = Arc::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000d2",
            false,
            crate::connector::tests::info("127.0.0.1:1", "key"),
        ));
        let s = Sender::attach(&host, &c).expect("created");
        // a stream thread the pool lacks: the commit's wake-up fails with a record
        s.out().session = Some(crate::sender::Session { thread: 9, id: 1 });
        let asked = s.replication().last_flush_ut.load(Ordering::Relaxed);
        {
            let mut out = s.out();
            s.flush_buffer(&mut out);
        }
        let flushed = s.replication().last_flush_ut.load(Ordering::Relaxed);
        assert_ne!(asked, flushed);
        let (committed, _) = netdata_agent_log::capture(|| {
            (s.commit_replication(b"REND\n", asked), s.commit_replication(b"REND\n", flushed))
        });
        assert_eq!(committed, (false, true));
        assert_eq!(s.out().buffer.stats().adds, 1);
        // without a session (a reconnect's delay) the bytes go nowhere, yet the answer counts, as C's (R55 M5)
        s.out().session = None;
        assert!(s.commit_replication(b"REND\n", flushed));
        assert_eq!(s.out().buffer.stats().adds, 1);
    }

    fn requests(q: &Queue) -> Vec<(i64, String)> {
        let state = q.lock();
        state.order.index.iter().map(|((after, _), (_, chart))| (*after, chart.clone())).collect()
    }

    /// Requests of every sender wait in the order of their `after`, then of their arrival; each is counted on its
    /// sender until picked, and in the queue until executed.
    #[test]
    fn requests_wait_by_after_across_senders() {
        let q = Queue::default();
        let (a, b) = (SenderQueue::new(), SenderQueue::new());
        q.add(Weak::new(), &a, "a.2".into(), 20, 30, false);
        q.add(Weak::new(), &b, "b.1".into(), 10, 30, false);
        q.add(Weak::new(), &a, "a.1".into(), 10, 30, true);
        assert_eq!(requests(&q), [(10, "b.1".into()), (10, "a.1".into()), (20, "a.2".into())]);
        assert_eq!(a.pending_requests.load(Ordering::Relaxed), 2);
        assert_eq!(a.charts_replicating.load(Ordering::Relaxed), 2);
        let taken = q.take().unwrap();
        assert_eq!(taken.request, Request { chart_id: "b.1".into(), after: 10, before: 30, start_streaming: false });
        assert_eq!(b.pending_requests.load(Ordering::Relaxed), 0);
        assert!(b.busy(), "picked, not executed");
        q.lock().delete_taken(taken.sender_id, &taken.request.chart_id, taken.flush_ut);
        assert!(!b.busy());
        assert_eq!(q.take().unwrap().request.chart_id, "a.1");
        assert_eq!(q.received.load(Ordering::Relaxed), 3);
    }

    /// A second request for a queued chart, picked or not, is a duplicate; an empty chart id is received and
    /// dropped.
    #[test]
    fn duplicates_and_empty_ids_are_not_queued() {
        let q = Queue::default();
        let s = SenderQueue::new();
        q.add(Weak::new(), &s, "c".into(), 10, 20, false);
        q.add(Weak::new(), &s, "c".into(), 5, 20, true);
        assert_eq!(requests(&q), [(10, "c".into())]);
        q.take().unwrap();
        q.add(Weak::new(), &s, "c".into(), 5, 20, true);
        q.add(Weak::new(), &s, String::new(), 1, 2, true);
        assert!(requests(&q).is_empty());
        assert_eq!(q.duplicate.load(Ordering::Relaxed), 2);
        assert_eq!(q.received.load(Ordering::Relaxed), 4);
        assert_eq!(s.charts_replicating.load(Ordering::Relaxed), 1);
    }

    /// The oldest start asked (`stream-replication-sender.c:1312-1313`) is lowered by every request received, a
    /// duplicate's and a refused empty chart id's too; a request from 0 unsets it, the next one sets it again; the
    /// delete of the sender's requests zeroes it (`:1323`).
    #[test]
    fn the_oldest_start_asked_counts_every_request_until_the_delete() {
        let q = Queue::default();
        let s = SenderQueue::new();
        let oldest = |s: &SenderQueue| s.stamps().0;
        q.add(Weak::new(), &s, "c".into(), 30, 40, false);
        assert_eq!(oldest(&s), 30);
        q.add(Weak::new(), &s, "c".into(), 20, 40, false);
        assert_eq!(oldest(&s), 20, "a duplicate");
        q.add(Weak::new(), &s, String::new(), 10, 40, false);
        assert_eq!(oldest(&s), 10, "an empty chart id");
        q.add(Weak::new(), &s, "d".into(), 15, 40, false);
        assert_eq!(oldest(&s), 10);
        q.add(Weak::new(), &s, "e".into(), 0, 40, false);
        assert_eq!(oldest(&s), 0);
        q.add(Weak::new(), &s, "f".into(), 25, 40, false);
        assert_eq!(oldest(&s), 25);
        q.delete_pending(&s);
        assert_eq!(s.stamps(), (0, 0));
    }

    /// The delete after a pick takes only the request picked: a newer one for the chart, asked after a reconnect,
    /// stays queued.
    #[test]
    fn a_newer_request_survives_the_delete_of_the_one_picked() {
        let q = Queue::default();
        let s = SenderQueue::new();
        q.add(Weak::new(), &s, "c".into(), 10, 20, false);
        let taken = q.take().unwrap();
        q.delete_pending(&s);
        s.last_flush_ut.store(7, Ordering::Relaxed);
        q.add(Weak::new(), &s, "c".into(), 30, 40, true);
        q.lock().delete_taken(taken.sender_id, &taken.request.chart_id, taken.flush_ut);
        assert_eq!(requests(&q), [(30, "c".into())]);
        assert_eq!(s.charts_replicating.load(Ordering::Relaxed), 1);
    }

    /// A picked request indexed again meanwhile (an unpark) leaves the order with its delete: answered once.
    #[test]
    fn a_picked_request_indexed_again_is_deleted_from_the_order() {
        let q = Queue::default();
        let s = SenderQueue::new();
        q.add(Weak::new(), &s, "c".into(), 10, 20, false);
        let taken = q.take().unwrap();
        {
            let mut state = q.lock();
            let State { order, senders } = &mut *state;
            let e = senders.get_mut(&s.id).unwrap().charts.get_mut("c").unwrap();
            order.add(&s, "c", e);
        }
        q.lock().delete_taken(taken.sender_id, &taken.request.chart_id, taken.flush_ut);
        assert!(requests(&q).is_empty());
        assert_eq!(s.pending_requests.load(Ordering::Relaxed), 0);
        assert!(!s.busy());
    }

    fn counts(q: &Queue) -> (usize, usize, usize, usize) {
        let o = &q.lock().order;
        (o.pending, o.pending_no_room, o.senders_full, o.sender_resets)
    }

    /// Past half its buffer a sender's waiting requests are parked, and one arriving then too, a duplicate of a
    /// parked one taking the newer window; below a tenth they are indexed again, with one dropped at its pick; the
    /// crossings count once each.
    #[test]
    fn a_full_buffer_parks_its_senders_requests() {
        let q = Queue::default();
        let (s, other) = (SenderQueue::new(), SenderQueue::new());
        q.add(Weak::new(), &s, "a".into(), 10, 20, false);
        q.add(Weak::new(), &s, "b".into(), 30, 40, false);
        q.add(Weak::new(), &other, "o".into(), 20, 30, false);
        q.recalculate(&s, 50);
        assert_eq!(counts(&q), (3, 0, 0, 0), "at half: nothing parked");
        q.recalculate(&s, 51);
        q.recalculate(&s, 70);
        assert_eq!(counts(&q), (1, 2, 1, 0));
        assert_eq!(requests(&q), [(20, "o".into())]);
        assert_eq!(s.pending_requests.load(Ordering::Relaxed), 0);
        assert!(s.busy(), "parked requests still count");
        q.add(Weak::new(), &s, "c".into(), 5, 6, false);
        q.add(Weak::new(), &s, "a".into(), 1, 2, true);
        assert_eq!(counts(&q), (1, 3, 1, 0));
        q.recalculate(&s, 10);
        assert_eq!(counts(&q), (1, 3, 1, 0), "at a tenth: still parked");
        q.recalculate(&s, 9);
        assert_eq!(requests(&q), [(1, "a".into()), (5, "c".into()), (20, "o".into()), (30, "b".into())]);
        assert_eq!(counts(&q), (4, 0, 0, 1));
        let taken = q.take().unwrap();
        assert_eq!(taken.request, Request { chart_id: "a".into(), after: 1, before: 2, start_streaming: true });
        q.recalculate(&s, 60);
        q.recalculate(&s, 0);
        assert_eq!(requests(&q).first(), Some(&(1, "a".into())), "the dropped pick is indexed again");
        assert_eq!(counts(&q), (4, 0, 0, 2));
    }

    /// A request picked, indexed again by an unpark and parked again leaves the queue with its delete, its parked
    /// count given back; an unpark indexes a sender's requests in the order they came.
    #[test]
    fn parked_requests_come_back_in_their_order_and_leave_once() {
        let q = Queue::default();
        let s = SenderQueue::new();
        for chart in ["z", "a", "m"] {
            q.add(Weak::new(), &s, chart.into(), 10, 20, false);
        }
        q.recalculate(&s, 60);
        q.recalculate(&s, 0);
        let order: Vec<String> = requests(&q).into_iter().map(|(_, c)| c).collect();
        assert_eq!(order, ["z", "a", "m"]);
        let taken = q.take().unwrap();
        q.recalculate(&s, 60);
        q.recalculate(&s, 0);
        q.recalculate(&s, 60);
        q.lock().delete_taken(taken.sender_id, &taken.request.chart_id, taken.flush_ut);
        assert_eq!(counts(&q), (0, 2, 1, 2));
        q.delete_pending(&s);
        assert_eq!(counts(&q), (0, 0, 1, 2));
        assert!(!s.busy());
    }

    /// A reset flushes parked requests too.
    #[test]
    fn a_reset_flushes_parked_requests() {
        let q = Queue::default();
        let s = SenderQueue::new();
        q.add(Weak::new(), &s, "a".into(), 10, 20, false);
        q.recalculate(&s, 90);
        q.delete_pending(&s);
        assert_eq!(counts(&q), (0, 0, 1, 0));
        assert!(q.idle());
        assert!(!s.busy());
    }

    /// The summary counts what happened since the last one, lists the waiting requests and its non-zero deltas, and
    /// ends in a space when there is none, as C's.
    #[test]
    fn the_summary_reports_the_deltas_since_the_last() {
        let q = Queue::default();
        let s = SenderQueue::new();
        q.add(Weak::new(), &s, "a".into(), 1, 2, false);
        q.add(Weak::new(), &s, "a".into(), 1, 2, false);
        q.add(Weak::new(), &s, String::new(), 1, 2, false);
        q.executed.store(5, Ordering::Relaxed);
        q.replied.store(6, Ordering::Relaxed);
        q.not_found.store(1, Ordering::Relaxed);
        let mut base = SummaryBase::default();
        let mut texts = Vec::new();
        for step in 0..3 {
            if step == 2 {
                q.delete_pending(&s);
            }
            let ((), records) = netdata_agent_log::capture(|| summary(&q, &[], &mut base));
            texts.extend(records.into_iter().map(|r| r.message.unwrap_or_default()));
        }
        let head = "REPLICATION SEND SUMMARY: all senders finished replication. Received";
        assert_eq!(
            texts,
            [
                format!(
                    "{head} 3, executed 5 and replied to 6 requests. 1 requests pending, 1 ignored-not-found, 1 \
                     ignored-merged"
                ),
                format!("{head} 0, executed 0 and replied to 0 requests. 1 requests pending"),
                format!("{head} 0, executed 0 and replied to 0 requests. 1 were flushed"),
            ]
        );
        let ((), records) = netdata_agent_log::capture(|| summary(&q, &[], &mut base));
        let last = format!("{head} 0, executed 0 and replied to 0 requests. ");
        assert_eq!(records[0].message.as_deref(), Some(last.as_str()), "a trailing space without deltas");
    }

    /// A sender's reset flushes its requests, picked or not, and leaves the others'.
    #[test]
    fn a_reset_flushes_one_senders_requests() {
        let q = Queue::default();
        let (a, b) = (SenderQueue::new(), SenderQueue::new());
        q.add(Weak::new(), &a, "a.1".into(), 10, 20, false);
        q.add(Weak::new(), &a, "a.2".into(), 30, 40, false);
        q.add(Weak::new(), &b, "b.1".into(), 20, 30, false);
        assert_eq!(q.take().unwrap().request.chart_id, "a.1");
        q.delete_pending(&a);
        assert_eq!(requests(&q), [(20, "b.1".into())]);
        assert!(!a.busy());
        assert_eq!(q.flushed.load(Ordering::Relaxed), 2);
        assert_eq!(q.pace().0, 1);
    }

    /// A request whose sender is gone is taken and dropped, not answered.
    #[test]
    fn a_request_of_a_gone_sender_is_dropped() {
        let q = Queue::default();
        let s = SenderQueue::new();
        q.add(Weak::new(), &s, "c".into(), 10, 20, false);
        assert!(!q.execute_next(&mut Vec::new()));
        assert!(q.idle());
        assert_eq!(q.replied.load(Ordering::Relaxed), 0);
    }
}
