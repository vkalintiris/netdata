//! The receivers' waiting list (`src/streaming/stream-waiting-list.c`): a stream thread admits at most one queued
//! receiver per dequeue, none while a tier backfill runs, and none in a dequeue that finds the thread's parsing moved
//! past a hundred definitions or finished replications since the last one. C's 5 s spacing between admissions never
//! applies: its `last_accepted_ut` is never written.

use std::collections::VecDeque;

/// One stream thread's queued receivers (`sth->queue.receivers`) and its last comparison (`sth->waiting_list`).
pub(crate) struct WaitingList<T> {
    queue: VecDeque<T>,
    /// `sth->waiting_list.{metadata,replication}`: the thread's counts, rounded down to hundreds, when last compared.
    seen: (usize, usize),
}

impl<T> Default for WaitingList<T> {
    fn default() -> Self {
        WaitingList {
            queue: VecDeque::new(),
            seen: (0, 0),
        }
    }
}

impl<T> WaitingList<T> {
    pub(crate) fn push(&mut self, receiver: T) {
        self.queue.push_back(receiver);
    }

    /// `stream_thread_process_waiting_list_unsafe()`: the first queued receiver, when children may be `accepted` and
    /// the thread's `throttle` counts, rounded down to hundreds, are those of the last comparison. A comparison that
    /// differs keeps the new counts and admits nothing; an empty queue or a refusal compares nothing.
    pub(crate) fn next(&mut self, accepted: bool, throttle: (usize, usize)) -> Option<T> {
        if self.queue.is_empty() || !accepted {
            return None;
        }
        let now = (throttle.0 / 100 * 100, throttle.1 / 100 * 100);
        if now != self.seen {
            self.seen = now;
            return None;
        }
        self.queue.pop_front()
    }

    /// `stream_receiver_move_entire_queue_to_running_unsafe()`'s queue: everything waiting, in order.
    pub(crate) fn take_all(&mut self) -> VecDeque<T> {
        std::mem::take(&mut self.queue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A receiver queued (or not), whether children may be accepted, the thread's counts; then what `next()` admits.
    type Step = (Option<u32>, bool, (usize, usize), Option<u32>);

    /// The comparison starts from zeroes, as C's zeroed globals and thread-local counters do.
    #[test]
    fn one_receiver_per_dequeue_while_the_counts_hold_still() {
        let cases: &[(&str, &[Step])] = &[
            ("fifo, one per dequeue", &[
                (Some(1), true, (0, 0), Some(1)),
                (Some(2), true, (0, 0), Some(2)),
                (Some(3), true, (0, 0), Some(3)),
                (None, true, (0, 0), None),
            ]),
            ("two queued, one per dequeue", &[
                (Some(1), true, (0, 0), Some(1)),
                (Some(2), false, (0, 0), None),
                (Some(3), true, (0, 0), Some(2)),
                (None, true, (0, 0), Some(3)),
            ]),
            ("a backfill admits nothing and compares nothing", &[
                (Some(1), false, (250, 0), None),
                (None, false, (250, 0), None),
                // the first comparison after it still finds the counts moved
                (None, true, (250, 0), None),
                (None, true, (250, 0), Some(1)),
            ]),
            ("counts compare by hundreds", &[
                (Some(1), true, (99, 99), Some(1)),
                (Some(2), true, (100, 0), None),
                (None, true, (199, 50), Some(2)),
                (Some(3), true, (199, 100), None),
                (None, true, (199, 100), Some(3)),
            ]),
            ("an empty queue compares nothing", &[
                (None, true, (300, 0), None),
                (Some(1), true, (300, 0), None),
                (None, true, (300, 0), Some(1)),
            ]),
        ];
        for (name, steps) in cases {
            let mut list = WaitingList::default();
            let admitted: Vec<_> = steps
                .iter()
                .map(|&(queued, accepted, throttle, _)| {
                    if let Some(r) = queued {
                        list.push(r);
                    }
                    list.next(accepted, throttle)
                })
                .collect();
            let want: Vec<_> = steps.iter().map(|s| s.3).collect();
            assert_eq!(admitted, want, "{name}");
        }
    }

    #[test]
    fn the_exit_takes_everything_in_order() {
        let mut list = WaitingList::default();
        for r in [1, 2, 3] {
            list.push(r);
        }
        assert_eq!(list.take_all(), [1, 2, 3]);
        assert_eq!(list.next(true, (0, 0)), None);
    }
}
