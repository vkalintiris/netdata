//! The replication sender (`src/streaming/stream-replication-sender.c`): the parents' `REPLAY_CHART` requests,
//! one per chart and sender, ordered by their `after` across every sender (D105.2), and the threads that answer them,
//! `REPLAY[1]`, a static thread, and its workers `REPLAY[2..threads]`, each taking the earliest request at a time
//! (D105.3: C with `[db] replication prefetch = 1`). The queue lives in the connector (D111.1); the answer is
//! `rrd::upstream::replay` (D111.5). Map: `knowledge/map-m7-commit6-replication.md` §2, §3 in the status repository.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use netdata_agent_rrd::stream_control;
use netdata_agent_rrd::upstream::replay::{self, Answered, Request};

use crate::sender::Sender;

/// `MAX_REPLICATION_MESSAGE_PERCENT_SENDER_BUFFER`: an answer is cut past this share of the sender's buffer.
const MAX_MESSAGE_PERCENT_OF_BUFFER: usize = 25;
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
}

impl SenderQueue {
    pub(crate) fn new() -> Arc<SenderQueue> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Arc::new(SenderQueue {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            pending_requests: AtomicUsize::new(0),
            charts_replicating: AtomicUsize::new(0),
            last_flush_ut: AtomicU64::new(0),
        })
    }

    /// `stream_sender_pending_replication_requests()` or `stream_sender_replicating_charts()`: the idle check waits.
    pub(crate) fn busy(&self) -> bool {
        self.pending_requests.load(Ordering::Relaxed) != 0 || self.charts_replicating.load(Ordering::Relaxed) != 0
    }

    /// `stream_sender_replicating_charts_zero()`.
    pub(crate) fn replicating_zero(&self) {
        self.charts_replicating.store(0, Ordering::Relaxed);
    }
}

/// A request's place (`indexed_in_judy`, `not_indexed_preprocessing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Waiting under its unique id.
    Indexed(u64),
    /// Picked by a thread; it leaves the queue when that thread executes it.
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
    pending: usize,
    added: usize,
    removed: usize,
}

impl Order {
    /// `replication_sort_entry_add()`.
    fn add(&mut self, sender: &SenderQueue, chart: &str, e: &mut Entry) {
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
}

#[derive(Debug, Default)]
struct State {
    order: Order,
    senders: HashMap<u64, Requests>,
}

impl State {
    /// `dictionary_del()` of the request a thread took (`replication_request_delete_callback()`): only one of its
    /// session, not a newer request for the chart that came after a reconnect; unlinked if it was indexed again
    /// meanwhile, so it is answered once.
    fn delete_taken(&mut self, sender_id: u64, chart: &str, flush_ut: u64) {
        let Some(requests) = self.senders.get_mut(&sender_id) else {
            return;
        };
        if requests.charts.get(chart).is_some_and(|e| e.flush_ut == flush_ut)
            && let Some(e) = requests.charts.remove(chart)
        {
            requests.queue.charts_replicating.fetch_sub(1, Ordering::Relaxed);
            if let Place::Indexed(uid) = e.place {
                self.order.unlink(&requests.queue, e.after, uid);
            }
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
            if let Place::Indexed(uid) = e.place {
                self.order.unlink(&requests.queue, e.after, uid);
            }
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

    /// `replication_sender_request_add()`: a new chart's request waits in the order; one for a chart already queued
    /// is a duplicate and ignored (C's conflict callback, whose other branches need a parked sender); an empty chart
    /// id is refused, unanswered, as C's dictionary refuses it.
    pub(crate) fn request_add(&self, sender: &Arc<Sender>, chart: String, after: i64, before: i64, start: bool) {
        self.add(Arc::downgrade(sender), sender.replication(), chart, after, before, start);
    }

    fn add(&self, sender: Weak<Sender>, queue: &Arc<SenderQueue>, chart: String, after: i64, before: i64, start: bool) {
        let flush_ut = queue.last_flush_ut.load(Ordering::Relaxed);
        if !chart.is_empty() {
            let mut state = self.lock();
            let State { order, senders } = &mut *state;
            let requests = senders.entry(queue.id).or_insert_with(|| Requests {
                sender,
                queue: Arc::clone(queue),
                charts: HashMap::new(),
            });
            match requests.charts.entry(chart) {
                std::collections::hash_map::Entry::Occupied(_) => {
                    self.duplicate.fetch_add(1, Ordering::Relaxed);
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    let mut e = Entry { after, before, start_streaming: start, flush_ut, place: Place::Picked };
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
            Answered::Executed => self.executed.fetch_add(1, Ordering::Relaxed),
        };
        true
    }

    /// Nothing waits (`pending` and `pending_no_room` both 0).
    fn idle(&self) -> bool {
        self.lock().order.pending == 0
    }

    /// `pending`, for the main thread's pace.
    fn pending(&self) -> usize {
        self.lock().order.pending
    }
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
/// while requests wait), then a second.
fn main_loop(queue: &Queue, stop: &Stop, exiting: fn() -> bool, update_every: Duration) {
    let mut buffer = Vec::new();
    let mut countdown = i64::MAX;
    let mut slow = true;
    let mut last_tick = Instant::now();
    let mut last_executed = 0;
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
                    countdown = i64::MAX;
                    slow = true;
                } else {
                    countdown = IDLE_TICKS;
                }
            }
        }
        if !queue.execute_next(&mut buffer) {
            let timeout = if slow {
                buffer = Vec::new();
                Duration::from_secs(1)
            } else if queue.pending() > 0 {
                // no sender unparks its requests before commit 6 (b): C's 100 ms follows an unpark
                Duration::from_secs(1)
            } else {
                Duration::from_millis(10)
            };
            if !stop.wait(timeout) {
                return;
            }
        }
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
    /// the requests of `queue`, the main thread's pace ticking every `update_every` seconds (`nd_profile`'s).
    pub fn spawn(
        threads: usize,
        stack_size: usize,
        queue: Arc<Queue>,
        update_every: Duration,
    ) -> io::Result<ReplicationThreads> {
        Self::spawn_with(threads, stack_size, queue, update_every, netdata_agent_sys::exit::initiated)
    }

    /// [`ReplicationThreads::spawn`] with `exiting` telling the exit's start (`service_running(SERVICE_REPLICATION)`,
    /// tested at the top of each loop).
    fn spawn_with(
        threads: usize,
        stack_size: usize,
        queue: Arc<Queue>,
        update_every: Duration,
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
            main_loop(&queue, &main_stop, exiting, update_every);
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

    /// Before the exit starts a join waits out its limit, then asks the threads to stop.
    #[test]
    fn a_join_waits_its_limit_then_asks() {
        let threads = ReplicationThreads::spawn_with(3, 256 * 1024, Arc::default(), SECOND, || false).unwrap();
        let started = Instant::now();
        threads.join_within(Duration::from_millis(200));
        assert!(started.elapsed() >= Duration::from_millis(200));
    }

    /// The exit's start ends them within their second's wait, before any stop.
    #[test]
    fn the_threads_end_when_the_exit_starts() {
        let threads = ReplicationThreads::spawn_with(3, 256 * 1024, Arc::default(), SECOND, || true).unwrap();
        let started = Instant::now();
        while !threads.main.is_finished() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(threads.main.is_finished() && started.elapsed() < Duration::from_millis(1500));
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
        assert_eq!(q.pending(), 1);
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
