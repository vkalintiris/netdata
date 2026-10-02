//! A stream connection's output buffer (`src/streaming/stream-circular-buffer.c`), a sender's to its parent and a
//! receiver's to its child, over libnetdata's ring (`src/libnetdata/circular_buffer/circular_buffer.c`), with C's
//! growth, capacity, contiguous chunks and statistics: the sizes appear in records. Map:
//! `knowledge/map-m7-commit4-runtime.md` §1.

/// `CBUFFER_INITIAL_SIZE`.
pub const INITIAL_SIZE: usize = 16 * 1024;
/// `CBUFFER_INITIAL_MAX_SIZE`.
pub const INITIAL_MAX_SIZE: usize = 10 * 1024 * 1024;
/// `STREAM_CIRCULAR_BUFFER_ADAPT_TO_TIMES_MAX_SIZE`: a commit raises the maximum to three times its size.
pub const ADAPT_TO_TIMES_MAX_SIZE: usize = 3;
/// How long a drained buffer keeps a grown ring before it shrinks back (`recreate_timed`).
const RECREATE_EVERY_UT: u64 = 300 * 1_000_000;

pub use crate::upstream::Traffic;

/// `STREAM_CIRCULAR_BUFFER_STATS`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    pub adds: usize,
    pub sends: usize,
    pub recreates: usize,
    pub bytes_added: usize,
    pub bytes_uncompressed: usize,
    pub bytes_sent: usize,
    pub bytes_size: usize,
    pub bytes_max_size: usize,
    /// The ring's first contiguous chunk (`cbuffer_next()`), not every byte waiting.
    pub bytes_outstanding: usize,
    pub bytes_available: usize,
    pub buffer_ratio: f64,
    pub bytes_sent_by_type: [usize; 4],
}

/// `struct circular_buffer`: `write` never reaches `size`; one byte of `max_size` is never used.
#[derive(Debug)]
struct Ring {
    data: Vec<u8>,
    write: usize,
    read: usize,
    max_size: usize,
}

impl Ring {
    fn new(initial: usize, max_size: usize) -> Self {
        Ring { data: vec![0; initial], write: 0, read: 0, max_size }
    }

    fn size(&self) -> usize {
        self.data.len()
    }

    /// `cbuffer_used_size_unsafe()`.
    fn used(&self) -> usize {
        if self.write >= self.read { self.write - self.read } else { self.size() - self.read + self.write }
    }

    /// `cbuffer_available_size_unsafe()` (C's `size_t` arithmetic).
    fn available(&self) -> usize {
        self.max_size.wrapping_sub(self.used())
    }

    /// `cbuffer_realloc_unsafe()`: twice the size (the maximum past half of it), the data moved to the start; false
    /// at the maximum.
    fn grow(&mut self) -> bool {
        if self.size() >= self.max_size {
            return false;
        }
        let new_size = if self.size() > self.max_size / 2 {
            self.max_size
        } else if self.size() == 0 {
            1
        } else {
            self.size() * 2
        };
        let mut data = vec![0; new_size];
        if self.read == self.write {
            self.write = 0;
        } else if self.read < self.write {
            data[..self.write - self.read].copy_from_slice(&self.data[self.read..self.write]);
            self.write -= self.read;
        } else {
            let top = self.size() - self.read;
            data[..top].copy_from_slice(&self.data[self.read..]);
            data[top..top + self.write].copy_from_slice(&self.data[..self.write]);
            self.write += top;
        }
        self.read = 0;
        self.data = data;
        true
    }

    /// `cbuffer_add_unsafe()`: false when the data do not fit under the maximum.
    fn add(&mut self, d: &[u8]) -> bool {
        let len = self.used();
        if len >= self.max_size || d.len() >= self.max_size - len {
            return false;
        }
        while len >= self.size() || d.len() >= self.size() - len {
            if !self.grow() {
                return false;
            }
        }
        let size = self.size();
        if d.len() < size - self.write {
            self.data[self.write..self.write + d.len()].copy_from_slice(d);
            self.write += d.len();
        } else {
            let top = size - self.write;
            self.data[self.write..].copy_from_slice(&d[..top]);
            self.data[..d.len() - top].copy_from_slice(&d[top..]);
            self.write = d.len() - top;
        }
        true
    }

    /// `cbuffer_remove_unsafe()`.
    fn remove(&mut self, n: usize) {
        let n = n.min(self.used());
        self.read += n;
        if self.read >= self.size() {
            self.read -= self.size();
        }
    }

    /// `cbuffer_next_unsafe()`: the contiguous chunk at the read position.
    fn next(&self) -> &[u8] {
        if self.read <= self.write { &self.data[self.read..self.write] } else { &self.data[self.read..] }
    }
}

/// `struct stream_circular_buffer`.
#[derive(Debug)]
pub struct CircularBuffer {
    ring: Ring,
    stats: Stats,
    last_recreate_ut: u64,
    last_sent_ut: u64,
    /// `atomic.buffer_ratio`: the used percentage, rounded.
    buffer_ratio: usize,
    last_flush_ut: u64,
}

impl Default for CircularBuffer {
    /// `stream_circular_buffer_create()`.
    fn default() -> Self {
        let mut b = CircularBuffer {
            ring: Ring::new(INITIAL_SIZE, INITIAL_MAX_SIZE),
            stats: Stats::default(),
            last_recreate_ut: 0,
            last_sent_ut: 0,
            buffer_ratio: 0,
            last_flush_ut: 0,
        };
        b.update_stats();
        b
    }
}

impl CircularBuffer {
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// `stream_sender_get_buffer_used_percent()`.
    pub fn used_percent(&self) -> usize {
        self.buffer_ratio
    }

    /// `stream_circular_buffer_last_flush_ut()`.
    pub fn last_flush_ut(&self) -> u64 {
        self.last_flush_ut
    }

    pub fn last_sent_ut(&self) -> u64 {
        self.last_sent_ut
    }

    /// `stream_circular_buffer_stats_update_unsafe()`.
    fn update_stats(&mut self) {
        let s = &mut self.stats;
        s.bytes_size = self.ring.size();
        s.bytes_max_size = self.ring.max_size;
        s.bytes_outstanding = self.ring.next().len();
        s.bytes_available = self.ring.available();
        s.buffer_ratio = if self.ring.max_size == 0 {
            0.0
        } else {
            (self.ring.max_size.wrapping_sub(s.bytes_available)) as f64 * 100.0 / self.ring.max_size as f64
        };
        self.buffer_ratio = s.buffer_ratio.round() as usize;
    }

    /// `stream_circular_buffer_set_max_size_unsafe()`: true when it changed the maximum.
    pub fn set_max_size(&mut self, max_size: usize, force: bool) -> bool {
        if force || self.ring.max_size < max_size {
            self.ring.max_size = max_size;
            self.update_stats();
            return true;
        }
        false
    }

    /// `stream_circular_buffer_flush_unsafe()`: empty, statistics zeroed, the maximum back to `max_size`, and the
    /// ring shrunk.
    pub fn flush(&mut self, max_size: usize, now_ut: u64) {
        self.last_flush_ut = now_ut;
        self.last_sent_ut = now_ut;
        self.ring.write = 0;
        self.ring.read = 0;
        self.stats = Stats::default();
        self.set_max_size(max_size, true);
        self.recreate_timed(now_ut, true);
    }

    /// `stream_circular_buffer_recreate_timed_unsafe()`: a grown ring shrinks back to 16 KiB when drained, at most
    /// every 5 minutes (always when forced); the count goes up either way.
    pub fn recreate_timed(&mut self, now_ut: u64, force: bool) {
        // clocks_usec_delta_or_zero_with_rebase()
        if now_ut < self.last_recreate_ut {
            self.last_recreate_ut = now_ut;
        }
        if !force
            && (self.stats.bytes_outstanding != 0 || now_ut - self.last_recreate_ut < RECREATE_EVERY_UT)
        {
            return;
        }
        self.last_recreate_ut = now_ut;
        self.stats.recreates += 1;
        if self.ring.size() > INITIAL_SIZE {
            self.ring = Ring::new(INITIAL_SIZE, self.ring.max_size);
        }
    }

    /// `stream_circular_buffer_add_unsafe()` without autoscale (the sender's): the counters go up even when the data
    /// do not fit, the sizes only when they do.
    pub fn add(&mut self, data: &[u8], uncompressed: usize, traffic: Traffic) -> bool {
        let s = &mut self.stats;
        s.adds += 1;
        s.bytes_added += data.len();
        s.bytes_uncompressed += uncompressed;
        s.bytes_sent_by_type[traffic as usize] += data.len();
        if !self.ring.add(data) {
            return false;
        }
        self.update_stats();
        true
    }

    /// `stream_circular_buffer_get_unsafe()`: the first contiguous chunk.
    pub fn next(&self) -> &[u8] {
        self.ring.next()
    }

    /// `stream_circular_buffer_del_unsafe()`: `bytes` sent.
    pub fn del(&mut self, bytes: usize, now_ut: u64) {
        self.last_sent_ut = now_ut;
        self.stats.sends += 1;
        self.stats.bytes_sent += bytes;
        self.ring.remove(bytes);
        self.update_stats();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_grows_to_its_maximum_less_one() {
        let mut r = Ring::new(4, 16);
        assert!(r.add(b"abc"));
        // 3 + 1 >= 4: grows to 8
        assert!(r.add(b"d"));
        assert_eq!((r.size(), r.used()), (8, 4));
        assert!(r.add(b"efghijk"));
        assert_eq!((r.size(), r.used()), (16, 11));
        // capacity is max - 1
        assert!(r.add(b"lmno"));
        assert!(!r.add(b"p"));
        assert_eq!(r.used(), 15);
    }

    #[test]
    fn chunks_are_contiguous_and_wrap() {
        let mut r = Ring::new(8, 8);
        assert!(r.add(b"abcdef"));
        r.remove(4);
        assert!(r.add(b"ghij"));
        // write wrapped: the first chunk is the top part only
        assert_eq!(r.next(), b"efgh");
        r.remove(4);
        assert_eq!(r.next(), b"ij");
        r.remove(10);
        assert_eq!((r.used(), r.next()), (0, &b""[..]));
    }

    #[test]
    fn growth_keeps_the_order_of_wrapped_data() {
        let mut r = Ring::new(8, 64);
        assert!(r.add(b"abcdef"));
        r.remove(5);
        assert!(r.add(b"ghijkl"));
        // f g h | i j k l wraps; this add grows and moves them down
        assert!(r.add(b"mnop"));
        assert_eq!(r.next(), b"fghijklmnop");
    }

    #[test]
    fn stats_count_failed_adds_and_the_contiguous_outstanding() {
        let mut b = CircularBuffer::default();
        b.set_max_size(32, true);
        assert!(b.add(&[1; 20], 40, Traffic::Data));
        assert!(!b.add(&[2; 20], 20, Traffic::Metadata));
        let s = *b.stats();
        assert_eq!((s.adds, s.bytes_added, s.bytes_uncompressed), (2, 40, 60));
        assert_eq!(s.bytes_sent_by_type, [0, 0, 20, 20]);
        // the failed add left the sizes of the first
        assert_eq!((s.bytes_outstanding, s.bytes_available, s.bytes_max_size), (20, 12, 32));
        assert_eq!(b.used_percent(), 63);
        b.del(20, 1);
        assert_eq!((b.stats().sends, b.stats().bytes_sent, b.stats().bytes_outstanding), (1, 20, 0));
    }

    #[test]
    fn flush_zeroes_and_shrinks() {
        let mut b = CircularBuffer::default();
        b.set_max_size(1 << 20, true);
        assert!(b.add(&vec![7; 100_000], 100_000, Traffic::Data));
        assert!(b.stats().bytes_size > INITIAL_SIZE);
        b.flush(INITIAL_MAX_SIZE, 10);
        let s = *b.stats();
        // as C's: the statistics were taken before the ring shrank
        assert_eq!((s.bytes_size, s.bytes_max_size, s.recreates, s.adds, s.bytes_outstanding), (131_072, INITIAL_MAX_SIZE, 1, 0, 0));
        assert!(b.add(b"x", 1, Traffic::Data));
        assert_eq!(b.stats().bytes_size, INITIAL_SIZE);
        b.del(1, 11);
        assert_eq!(b.last_flush_ut(), 10);
        // a timed recreate waits 5 minutes and for the buffer to drain
        b.recreate_timed(20, false);
        assert_eq!(b.stats().recreates, 1);
        b.recreate_timed(10 + RECREATE_EVERY_UT, false);
        assert_eq!(b.stats().recreates, 2);
    }
}
