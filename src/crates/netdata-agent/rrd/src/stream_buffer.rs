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

    /// `stream_circular_buffer_add_unsafe()`: the counters go up even when the data do not fit, the sizes only when
    /// they do. With `autoscale` (a receiver's, for its child; the sender's has none) the maximum first doubles when
    /// the free space is less than the data, and stays doubled whatever the add does; data of exactly the free space
    /// do not fit (the ring keeps one byte), so they are refused unscaled.
    pub fn add(&mut self, data: &[u8], uncompressed: usize, traffic: Traffic, autoscale: bool) -> bool {
        let s = &mut self.stats;
        s.adds += 1;
        s.bytes_added += data.len();
        s.bytes_uncompressed += uncompressed;
        s.bytes_sent_by_type[traffic as usize] += data.len();
        if autoscale && self.ring.available() < data.len() {
            let max_size = self.ring.max_size.saturating_mul(2);
            if max_size > self.ring.max_size {
                self.set_max_size(max_size, true);
            }
        }
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
        assert!(b.add(&[1; 20], 40, Traffic::Data, false));
        assert!(!b.add(&[2; 20], 20, Traffic::Metadata, false));
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
        assert!(b.add(&vec![7; 100_000], 100_000, Traffic::Data, false));
        assert!(b.stats().bytes_size > INITIAL_SIZE);
        b.flush(INITIAL_MAX_SIZE, 10);
        let s = *b.stats();
        // as C's: the statistics were taken before the ring shrank
        assert_eq!((s.bytes_size, s.bytes_max_size, s.recreates, s.adds, s.bytes_outstanding), (131_072, INITIAL_MAX_SIZE, 1, 0, 0));
        assert!(b.add(b"x", 1, Traffic::Data, false));
        assert_eq!(b.stats().bytes_size, INITIAL_SIZE);
        b.del(1, 11);
        assert_eq!(b.last_flush_ut(), 10);
        // a timed recreate waits 5 minutes and for the buffer to drain
        b.recreate_timed(20, false);
        assert_eq!(b.stats().recreates, 1);
        b.recreate_timed(10 + RECREATE_EVERY_UT, false);
        assert_eq!(b.stats().recreates, 2);
    }

    /// A receiver's buffer after `adds` of `added` bytes: what its statistics say.
    fn stats(adds: usize, added: usize, size: usize, max: usize, outstanding: usize, available: usize) -> Stats {
        Stats {
            adds,
            bytes_added: added,
            bytes_uncompressed: added,
            bytes_size: size,
            bytes_max_size: max,
            bytes_outstanding: outstanding,
            bytes_available: available,
            buffer_ratio: (max - available) as f64 * 100.0 / max as f64,
            bytes_sent_by_type: [0, 0, added, 0],
            ..Stats::default()
        }
    }

    /// `stream_circular_buffer_add_unsafe()` with autoscale against `cbuffer_add_unsafe()`: data of exactly the free
    /// space are refused with the maximum unscaled, one byte less fits, one byte more doubles the maximum first.
    #[test]
    fn autoscale_doubles_the_maximum_but_not_for_an_exact_fit() {
        let fill = INITIAL_MAX_SIZE - 4096;
        let filled = || {
            let mut b = CircularBuffer::default();
            assert!(b.add(&vec![0; fill], fill, Traffic::Metadata, true));
            assert_eq!(*b.stats(), stats(1, fill, INITIAL_MAX_SIZE, INITIAL_MAX_SIZE, fill, 4096));
            b
        };
        let mut b = filled();
        assert!(!b.add(&[0; 4096], 4096, Traffic::Metadata, true));
        // counted, but the sizes are the first add's
        let counted = stats(2, fill + 4096, INITIAL_MAX_SIZE, INITIAL_MAX_SIZE, fill, 4096);
        assert_eq!(*b.stats(), counted);
        assert!(b.add(&[0; 4095], 4095, Traffic::Metadata, true));
        assert_eq!(*b.stats(), stats(3, fill + 8191, INITIAL_MAX_SIZE, INITIAL_MAX_SIZE, fill + 4095, 1));
        let mut b = filled();
        assert!(b.add(&[0; 4097], 4097, Traffic::Metadata, true));
        let doubled = 2 * INITIAL_MAX_SIZE;
        assert_eq!(*b.stats(), stats(2, fill + 4097, doubled, doubled, 10_485_761, 10_485_759));
        assert_eq!(b.used_percent(), 50);
    }

    /// The doubling runs before the add: data of twice the maximum are refused, the maximum stays doubled, and the
    /// next add fits under it. Without autoscale (the sender's) nothing grows.
    #[test]
    fn a_refused_autoscaled_add_keeps_its_doubled_maximum() {
        let doubled = 2 * INITIAL_MAX_SIZE;
        let mut b = CircularBuffer::default();
        assert!(!b.add(&vec![0; doubled], doubled, Traffic::Metadata, true));
        assert_eq!(*b.stats(), stats(1, doubled, INITIAL_SIZE, doubled, 0, doubled));
        assert!(b.add(&vec![0; doubled - 1], doubled - 1, Traffic::Metadata, true));
        assert_eq!(b.stats().bytes_max_size, doubled);
        let mut b = CircularBuffer::default();
        assert!(!b.add(&vec![0; INITIAL_MAX_SIZE], INITIAL_MAX_SIZE, Traffic::Data, false));
        assert_eq!(b.stats().bytes_max_size, INITIAL_MAX_SIZE);
    }

    /// The numbers C's receiver records read off a wrapped, grown ring: the first contiguous chunk as "used" or
    /// "pending", the free space and the percentage under the grown maximum, the sends.
    #[test]
    fn a_wrapped_grown_ring_reports_as_cs() {
        let mut b = CircularBuffer::default();
        let add = |b: &mut CircularBuffer, n: usize| b.add(&vec![0; n], n, Traffic::Replication, true);
        assert!(add(&mut b, 8_388_608));
        b.del(8_388_608, 1);
        assert!(add(&mut b, 4_194_304));
        let sizes = |s: &Stats| (s.bytes_size, s.bytes_max_size, s.bytes_outstanding, s.bytes_available);
        let wrapped = (INITIAL_MAX_SIZE, INITIAL_MAX_SIZE, 2_097_152, 6_291_456);
        assert_eq!(sizes(b.stats()), wrapped);
        // the exact fit: what the overflow record prints
        assert!(!add(&mut b, 6_291_456));
        assert_eq!(sizes(b.stats()), wrapped);
        assert!(add(&mut b, 6_291_457));
        assert_eq!(b.stats().bytes_outstanding, 10_485_761);
        b.del(10_485_761, 2);
        assert!(add(&mut b, 12_000_000));
        let s = *b.stats();
        assert_eq!(
            (s.bytes_size, s.bytes_max_size, s.bytes_outstanding, s.bytes_available, s.bytes_sent, s.sends),
            (2 * INITIAL_MAX_SIZE, 2 * INITIAL_MAX_SIZE, 10_485_759, 8_971_520, 18_874_369, 2)
        );
        assert_eq!(format!("{:.2}", s.buffer_ratio), "57.22");
    }
}
