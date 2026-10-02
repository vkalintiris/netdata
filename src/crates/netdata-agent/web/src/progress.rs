//! The query progress table (`src/libnetdata/query_progress/progress.c:1-392`): a row per request transaction, started
//! when the request validates and finished when its access record is written, with the steps a data query or a
//! function's plugin reports in between. `/api/v2|v3/progress` reads one row; `netdata-api-calls` lists them all.
//!
//! The finished rows are kept, oldest first, up to [`CACHE_SIZE`]: a start of an unknown transaction then takes the
//! oldest one over, so a finished request stays visible for the next 199 requests. Rows in flight are never evicted.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use netdata_agent_text::json::JsonWriter;

use crate::request::Mode;
use crate::status;

/// `PROGRESS_CACHE_SIZE`: the finished rows kept.
pub const CACHE_SIZE: usize = 200;

/// A request's transaction, its row's key.
pub type Transaction = [u8; 16];

/// What `query_progress_start_or_update()` records of a request. The row's payload is not kept: C copies it but
/// nothing reads it (D159.2).
#[derive(Debug, Clone, Copy, Default)]
pub struct Start<'a> {
    pub mode: Option<Mode>,
    pub acl: u32,
    /// The URL as received, raw.
    pub query: &'a [u8],
    /// `X-Forwarded-For`, else the client's address.
    pub client: &'a [u8],
}

/// What `query_progress_finished()` records of a response.
#[derive(Debug, Clone, Copy, Default)]
pub struct Finish {
    pub code: u16,
    pub duration_ut: u64,
    pub response_size: u64,
    pub sent_size: u64,
}

/// A row as `netdata-api-calls` reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowView<'a> {
    pub started_ut: u64,
    /// 0 while the request runs.
    pub finished_ut: u64,
    pub duration_ut: u64,
    pub mode: Option<Mode>,
    pub acl: u32,
    pub query: &'a [u8],
    pub client: &'a [u8],
    pub response_code: u16,
    pub response_size: u32,
    pub sent_size: u32,
    pub all: usize,
    pub done: usize,
}

#[derive(Debug, Default)]
struct Row {
    started_ut: u64,
    finished_ut: u64,
    duration_ut: u64,
    mode: Option<Mode>,
    acl: u32,
    query: Vec<u8>,
    client: Vec<u8>,
    response_code: u16,
    // C's `uint32_t`s, which the sizes are truncated to
    response_size: u32,
    sent_size: u32,
    all: usize,
    done: usize,
    /// In the finished list (C's `prev != NULL`).
    listed: bool,
}

impl Row {
    /// `query_progress_cleanup_to_reuse()`: everything zeroed, the buffers emptied (their capacity kept).
    fn clean(&mut self) {
        let (mut query, mut client) = (std::mem::take(&mut self.query), std::mem::take(&mut self.client));
        query.clear();
        client.clear();
        *self = Row { query, client, ..Row::default() };
    }

    /// `query_progress_update()`: the request's fields, its times and sizes reset; the URL and the client are kept
    /// once set, so a transaction restarted while in flight keeps its first ones.
    fn update(&mut self, started_ut: u64, s: &Start<'_>) {
        self.mode = s.mode;
        self.acl = s.acl;
        self.started_ut = started_ut;
        self.finished_ut = 0;
        self.duration_ut = 0;
        self.response_size = 0;
        self.sent_size = 0;
        self.response_code = 0;
        if !s.query.is_empty() && self.query.is_empty() {
            self.query.extend_from_slice(s.query);
        }
        if !s.client.is_empty() && self.client.is_empty() {
            self.client.extend_from_slice(s.client);
        }
    }

    fn view(&self) -> RowView<'_> {
        RowView {
            started_ut: self.started_ut,
            finished_ut: self.finished_ut,
            duration_ut: self.duration_ut,
            mode: self.mode,
            acl: self.acl,
            query: &self.query,
            client: &self.client,
            response_code: self.response_code,
            response_size: self.response_size,
            sent_size: self.sent_size,
            all: self.all,
            done: self.done,
        }
    }
}

#[derive(Debug, Default)]
struct Inner {
    rows: HashMap<Transaction, Row>,
    /// The finished rows' transactions, oldest first.
    finished: VecDeque<Transaction>,
}

impl Inner {
    fn unlist(&mut self, tx: &Transaction) {
        if let Some(i) = self.finished.iter().position(|t| t == tx) {
            self.finished.remove(i);
        }
    }
}

/// The table (C's `progress` global, under its spinlock).
#[derive(Debug)]
pub struct Table {
    inner: Mutex<Inner>,
    /// `now_realtime_usec()`.
    now_ut: fn() -> u64,
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}

/// `now_realtime_usec()`.
fn now_realtime_ut() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64)
}

impl Table {
    pub fn new() -> Self {
        Self::with_clock(now_realtime_ut)
    }

    /// A table reading the realtime clock from `now_ut`.
    pub fn with_clock(now_ut: fn() -> u64) -> Self {
        Self { inner: Mutex::new(Inner::default()), now_ut }
    }

    /// The process's table.
    pub fn process() -> &'static Table {
        static TABLE: OnceLock<Table> = OnceLock::new();
        TABLE.get_or_init(Table::new)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `query_progress_start_or_update()`: a finished row of this transaction is cleaned and starts again, one in
    /// flight is updated; an unknown transaction takes the oldest finished row over once [`CACHE_SIZE`] are kept.
    /// `started_ut` 0 is now.
    pub fn start(&self, tx: &Transaction, started_ut: u64, s: Start<'_>) {
        let started_ut = if started_ut == 0 { (self.now_ut)() } else { started_ut };
        let mut inner = self.lock();
        if !inner.rows.contains_key(tx)
            && inner.finished.len() >= CACHE_SIZE
            && let Some(oldest) = inner.finished.pop_front()
        {
            let mut row = inner.rows.remove(&oldest).unwrap_or_default();
            row.clean();
            inner.rows.insert(*tx, row);
        }
        let listed = inner.rows.get(tx).is_some_and(|r| r.listed);
        if listed {
            inner.unlist(tx);
        }
        let row = inner.rows.entry(*tx).or_default();
        if listed {
            row.clean();
        }
        row.update(started_ut, &s);
    }

    /// `query_progress_set_finish_line()`: the steps a query will take, the largest announced.
    pub fn set_finish_line(&self, tx: &Transaction, all: usize) {
        if let Some(row) = self.lock().rows.get_mut(tx) {
            row.all = row.all.max(all);
        }
    }

    /// `query_progress_done_step()`: `done` more steps taken.
    pub fn done_step(&self, tx: &Transaction, done: usize) {
        if let Some(row) = self.lock().rows.get_mut(tx) {
            row.done += done;
        }
    }

    /// `query_progress_functions_update()`: a plugin's totals, each replacing the row's when not 0.
    pub fn functions_update(&self, tx: &Transaction, done: usize, all: usize) {
        if let Some(row) = self.lock().rows.get_mut(tx) {
            if all != 0 {
                row.all = all;
            }
            if done != 0 {
                row.done = done;
            }
        }
    }

    /// `query_progress_finished()`: the row (if any) is finished and listed last; one row beyond [`CACHE_SIZE`]
    /// listed, the oldest, is dropped. `finished_ut` 0 is now.
    pub fn finished(&self, tx: &Transaction, finished_ut: u64, f: Finish) {
        let finished_ut = if finished_ut == 0 { (self.now_ut)() } else { finished_ut };
        let mut inner = self.lock();
        let listed = match inner.rows.get_mut(tx) {
            Some(row) => {
                row.sent_size = f.sent_size as u32;
                row.response_size = f.response_size as u32;
                row.response_code = f.code;
                row.duration_ut = f.duration_ut;
                row.finished_ut = finished_ut;
                Some(std::mem::replace(&mut row.listed, true))
            }
            None => None,
        };
        if let Some(was) = listed {
            if was {
                inner.unlist(tx);
            }
            inner.finished.push_back(*tx);
        }
        if inner.finished.len() > CACHE_SIZE
            && let Some(oldest) = inner.finished.pop_front()
        {
            inner.rows.remove(&oldest);
        }
    }

    /// `web_api_v2_report_progress()`'s members: a finished request at 100%, one in flight with its age and its
    /// steps' share (or the steps taken, when it announced none); the HTTP status, 404 for an unknown transaction.
    pub fn report(&self, tx: &Transaction, w: &mut JsonWriter) -> u16 {
        let found = self.lock().rows.get(tx).map(|r| (r.started_ut, r.finished_ut, r.all, r.done));
        let Some((started_ut, finished_ut, all, done)) = found else {
            w.member_add_uint64("status", u64::from(status::NOT_FOUND));
            w.member_add_string("message", "Transaction not found");
            return status::NOT_FOUND;
        };
        w.member_add_uint64("status", u64::from(status::OK));
        w.member_add_uint64("started_ut", started_ut);
        if finished_ut != 0 {
            w.member_add_uint64("finished_ut", finished_ut);
            w.member_add_double("progress", 100.0);
            w.member_add_uint64("age_ut", finished_ut.wrapping_sub(started_ut));
        } else {
            let now_ut = (self.now_ut)();
            w.member_add_uint64("now_ut", now_ut);
            w.member_add_uint64("age_ut", now_ut.wrapping_sub(started_ut));
            if all != 0 {
                w.member_add_double("progress", done as f64 * 100.0 / all as f64);
            } else {
                w.member_add_uint64("working", done as u64);
            }
        }
        status::OK
    }

    /// A data query's reporter for this transaction.
    pub fn tracker(&self, tx: Transaction) -> Tracker<'_> {
        Tracker { table: self, tx }
    }

    /// Every row, in no order.
    pub fn visit(&self, mut f: impl FnMut(&Transaction, RowView<'_>)) {
        for (tx, row) in &self.lock().rows {
            f(tx, row.view());
        }
    }
}

/// What a data query reports of its request (`qt->request.transaction` in `rrd2rrdr()`).
#[derive(Debug, Clone, Copy)]
pub struct Tracker<'a> {
    table: &'a Table,
    tx: Transaction,
}

impl Tracker<'_> {
    /// The metrics the query will run.
    pub fn finish_line(&self, all: usize) {
        self.table.set_finish_line(&self.tx, all);
    }

    /// One metric run.
    pub fn step(&self) {
        self.table.done_step(&self.tx, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdata_agent_text::json::JsonOptions;

    const NOW: u64 = 5_000_000;

    fn clock() -> u64 {
        NOW
    }

    fn tx(n: u32) -> Transaction {
        let mut t = [0xab; 16];
        t[..4].copy_from_slice(&n.to_be_bytes());
        t
    }

    fn get(s: Start<'static>) -> Start<'static> {
        Start { mode: Some(Mode::Get), ..s }
    }

    fn report(t: &Table, id: &Transaction) -> (u16, String) {
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        let code = t.report(id, &mut w);
        w.finalize();
        (code, String::from_utf8(w.into_bytes()).unwrap())
    }

    /// The body for a request in flight started at `started_ut`, ending with `tail`.
    fn running(started_ut: u64, tail: &str) -> String {
        format!(r#"{{"status":200,"started_ut":{started_ut},"now_ut":{NOW},"age_ut":{},{tail}}}"#, NOW - started_ut)
    }

    fn listed(t: &Table) -> Vec<Transaction> {
        t.lock().finished.iter().copied().collect()
    }

    /// `progress_unittest()` (`progress.c:618-655`), shortened: rows in flight survive any churn of finished ones,
    /// which settle at the cache size.
    #[test]
    fn rows_in_flight_are_never_evicted() {
        let t = Table::with_clock(clock);
        for n in 0..100 {
            t.start(&tx(n), 0, Start { acl: 1, query: b"permanent", client: b"test", ..get(Start::default()) });
        }
        for n in 1000..2000 {
            t.start(&tx(n), 0, Start { mode: Some(Mode::Options), acl: 2, query: b"ephemeral", client: b"test" });
            t.finished(&tx(n), 0, Finish { code: 200, duration_ut: 1234, response_size: 123, sent_size: 12 });
        }
        let mut running = 0;
        t.visit(|id, row| {
            if row.finished_ut == 0 {
                running += 1;
                assert!(u32::from_be_bytes(id[..4].try_into().unwrap()) < 100);
                assert_eq!(row.query, b"permanent");
            }
        });
        assert_eq!(running, 100);
        assert_eq!(listed(&t).len(), CACHE_SIZE);
        assert_eq!(t.lock().rows.len(), 100 + CACHE_SIZE);
    }

    /// An unknown transaction takes the oldest finished row over at its start once 200 are listed; a finish that
    /// lists a 201st drops the oldest.
    #[test]
    fn retention_is_cs() {
        let t = Table::with_clock(clock);
        for n in 0..200 {
            t.start(&tx(n), 0, Start::default());
            t.finished(&tx(n), 0, Finish::default());
        }
        assert_eq!(report(&t, &tx(0)).0, status::OK);
        t.start(&tx(500), 0, Start::default());
        assert_eq!(report(&t, &tx(0)).0, status::NOT_FOUND);
        assert_eq!(listed(&t).len(), 199);
        // 199 listed: the next start allocates
        t.start(&tx(501), 0, Start::default());
        assert_eq!(report(&t, &tx(1)).0, status::OK);
        t.finished(&tx(500), 0, Finish::default());
        assert_eq!(listed(&t).len(), 200);
        t.finished(&tx(501), 0, Finish::default());
        assert_eq!(listed(&t).len(), 200);
        assert_eq!(report(&t, &tx(1)).0, status::NOT_FOUND);
        assert_eq!(report(&t, &tx(2)).0, status::OK);
        assert_eq!(listed(&t).last(), Some(&tx(501)));
        // a finish of an unknown transaction creates nothing
        t.finished(&tx(900), 0, Finish::default());
        assert_eq!(report(&t, &tx(900)).0, status::NOT_FOUND);
    }

    /// A finished transaction started again is cleaned and leaves the list; one restarted in flight keeps its URL,
    /// client and steps; a second finish moves a row to the list's end.
    #[test]
    fn restarts_and_refinishes_are_cs() {
        let t = Table::with_clock(clock);
        let a = Start { mode: Some(Mode::Post), acl: 7, query: b"/api/v1/function?function=a", client: b"10.0.0.1" };
        t.start(&tx(1), 100, a);
        t.functions_update(&tx(1), 5, 10);
        t.start(&tx(1), 200, Start { query: b"/other", client: b"10.0.0.2", ..get(Start::default()) });
        let mut seen = None;
        t.visit(|_, row| seen = Some((row.started_ut, row.mode, row.acl, row.query.to_vec(), row.client.to_vec())));
        let first = (b"/api/v1/function?function=a".to_vec(), b"10.0.0.1".to_vec());
        assert_eq!(seen, Some((200, Some(Mode::Get), 0, first.0, first.1)));
        assert_eq!(report(&t, &tx(1)).1, running(200, r#""progress":50"#));

        t.finished(&tx(1), 300, Finish { code: 200, duration_ut: 9, response_size: 1 << 33, sent_size: 3 });
        let mut sizes = None;
        t.visit(|_, row| sizes = Some((row.response_code, row.duration_ut, row.response_size, row.sent_size)));
        assert_eq!(sizes, Some((200, 9, 0, 3)));
        t.start(&tx(1), 400, a);
        assert!(listed(&t).is_empty());
        assert_eq!(report(&t, &tx(1)).1, running(400, r#""working":0"#));

        t.start(&tx(2), 0, Start::default());
        t.finished(&tx(1), 0, Finish::default());
        t.finished(&tx(2), 0, Finish::default());
        t.finished(&tx(1), 0, Finish::default());
        assert_eq!(listed(&t), [tx(2), tx(1)]);
    }

    /// The steps: a finish line keeps the largest, steps add up, a plugin's totals replace (a 0 keeps the row's),
    /// and none of them makes a row.
    #[test]
    fn steps_are_cs() {
        let t = Table::with_clock(clock);
        t.set_finish_line(&tx(9), 5);
        t.done_step(&tx(9), 1);
        t.functions_update(&tx(9), 1, 2);
        assert!(t.lock().rows.is_empty());

        t.start(&tx(1), NOW, Start::default());
        let tracker = t.tracker(tx(1));
        tracker.finish_line(4);
        tracker.finish_line(3);
        tracker.step();
        assert_eq!(report(&t, &tx(1)).1, running(NOW, r#""progress":25"#));
        t.done_step(&tx(1), 2);
        t.set_finish_line(&tx(1), 9);
        assert_eq!(report(&t, &tx(1)).1, running(NOW, r#""progress":33.3333333"#));
        t.functions_update(&tx(1), 0, 12);
        t.functions_update(&tx(1), 6, 0);
        assert_eq!(report(&t, &tx(1)).1, running(NOW, r#""progress":50"#));
    }

    /// `web_api_v2_report_progress()`'s other bodies: finished, unknown, and an age that wraps as C's unsigned one.
    #[test]
    fn reports_are_cs() {
        let t = Table::with_clock(clock);
        t.start(&tx(1), 1_000, Start::default());
        t.finished(&tx(1), 4_500, Finish::default());
        assert_eq!(
            report(&t, &tx(1)),
            (status::OK, r#"{"status":200,"started_ut":1000,"finished_ut":4500,"progress":100,"age_ut":3500}"#.into())
        );
        assert_eq!(
            report(&t, &tx(2)),
            (status::NOT_FOUND, r#"{"status":404,"message":"Transaction not found"}"#.into())
        );
        t.start(&tx(3), NOW + 1, Start::default());
        assert_eq!(
            report(&t, &tx(3)).1,
            r#"{"status":200,"started_ut":5000001,"now_ut":5000000,"age_ut":18446744073709551615,"working":0}"#
        );
    }
}
