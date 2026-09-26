//! GORILLA_32BIT pages, ported from `src/libnetdata/gorilla/gorilla.{h,cc}` and the gorilla parts of
//! `src/database/engine/page.c`: storage numbers XOR-compressed into a chain of 512-byte buffers. Brief
//! `knowledge/brief-dbengine-s0.md` §3.4 in the status repository.
//!
//! A buffer is a 16-byte header (`next` 8, `entries` 4, `nbits` 4) and 124 `u32` data words filled LSB first. On disk
//! C writes its heap pointer in `next`; readers only test it for zero, so a Rust writer stores 1 in every buffer but
//! the last (D50 point 4).

use super::EmptyPage;

/// `RRDENG_GORILLA_32BIT_BUFFER_SIZE`.
pub const BUFFER_SIZE: usize = 512;
const HEADER_SIZE: usize = 16;
const DATA_WORDS: usize = (BUFFER_SIZE - HEADER_SIZE) / 4;
/// `gorilla_buffer_data_bits()` of a whole buffer: 3968.
pub const CAPACITY_BITS: u32 = (DATA_WORDS * 32) as u32;

/// One buffer of the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Buffer {
    pub entries: u32,
    pub nbits: u32,
    pub data: [u32; DATA_WORDS],
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer {
            entries: 0,
            nbits: 0,
            data: [0; DATA_WORDS],
        }
    }
}

impl Buffer {
    /// The 512 bytes C writes, with `next` as given.
    pub fn to_bytes(&self, next: u64) -> [u8; BUFFER_SIZE] {
        let mut b = [0u8; BUFFER_SIZE];
        b[0..8].copy_from_slice(&next.to_le_bytes());
        b[8..12].copy_from_slice(&self.entries.to_le_bytes());
        b[12..16].copy_from_slice(&self.nbits.to_le_bytes());
        for (i, w) in self.data.iter().enumerate() {
            b[HEADER_SIZE + i * 4..HEADER_SIZE + i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        b
    }
}

/// `bit_buffer_write()`: at a word boundary the word is stored whole; otherwise the low part is ORed in and what
/// spills is stored into the next word.
fn write_bits(data: &mut [u32; DATA_WORDS], pos: u32, v: u32, nbits: u32) {
    let index = (pos / 32) as usize;
    let offset = pos % 32;
    if offset == 0 {
        data[index] = v;
    } else {
        let remaining = 32 - offset;
        let low_mask = (1u32 << remaining) - 1;
        data[index] |= (v & low_mask) << offset;
        if nbits > remaining {
            data[index + 1] = (v & !low_mask) >> remaining;
        }
    }
}

/// `bit_buffer_read()`.
fn read_bits(data: &[u32; DATA_WORDS], pos: u32, nbits: u32) -> u32 {
    let index = (pos / 32) as usize;
    let offset = pos % 32;
    let mask = |n: u32| if n == 32 { u32::MAX } else { (1u32 << n) - 1 };
    let word = data[index];
    if offset == 0 {
        return word & mask(nbits);
    }
    let remaining = 32 - offset;
    if nbits < remaining {
        return (word >> offset) & mask(nbits);
    }
    let low = (word >> offset) & mask(remaining);
    let rest = nbits - remaining;
    if rest == 0 {
        return low;
    }
    low | ((data[index + 1] & mask(rest)) << remaining)
}

/// `gorilla_writer_t`.
#[derive(Debug, Clone)]
pub struct Writer {
    buffers: Vec<Buffer>,
    prev_number: u32,
    prev_xor_lzc: u32,
}

impl Default for Writer {
    fn default() -> Self {
        Writer {
            buffers: vec![Buffer::default()],
            prev_number: 0,
            prev_xor_lzc: 0,
        }
    }
}

impl Writer {
    pub fn buffers(&self) -> &[Buffer] {
        &self.buffers
    }

    /// `gorilla_writer_add_buffer()`: a fresh buffer at the end; the XOR state restarts.
    pub fn add_buffer(&mut self) {
        self.buffers.push(Buffer::default());
        self.prev_number = 0;
        self.prev_xor_lzc = 0;
    }

    /// `gorilla_writer_write()` into the last buffer. `false` when it is full; the bits written before the failing
    /// check stay, as in C.
    pub fn write(&mut self, number: u32) -> bool {
        let b = self
            .buffers
            .last_mut()
            .expect("a writer always has a buffer");
        let fits = |b: &Buffer, n: u32| b.nbits + n < CAPACITY_BITS;
        if b.entries == 0 {
            if !fits(b, 32) {
                return false;
            }
            write_bits(&mut b.data, b.nbits, number, 32);
            b.nbits += 32;
            b.entries += 1;
            self.prev_number = number;
            return true;
        }
        if number == self.prev_number {
            if !fits(b, 1) {
                return false;
            }
            write_bits(&mut b.data, b.nbits, 1, 1);
            b.nbits += 1;
            b.entries += 1;
            return true;
        }
        if !fits(b, 1) {
            return false;
        }
        write_bits(&mut b.data, b.nbits, 0, 1);
        b.nbits += 1;
        let xor_value = self.prev_number ^ number;
        let xor_lzc = xor_value.leading_zeros();
        let same_lzc = u32::from(xor_lzc == self.prev_xor_lzc);
        if !fits(b, 1) {
            return false;
        }
        write_bits(&mut b.data, b.nbits, same_lzc, 1);
        b.nbits += 1;
        if same_lzc == 0 {
            if !fits(b, 5) {
                return false;
            }
            write_bits(&mut b.data, b.nbits, xor_lzc, 5);
            b.nbits += 5;
        }
        let bits = 32 - xor_lzc;
        if !fits(b, bits) {
            return false;
        }
        write_bits(&mut b.data, b.nbits, xor_value, bits);
        b.nbits += bits;
        b.entries += 1;
        self.prev_number = number;
        self.prev_xor_lzc = xor_lzc;
        true
    }

    /// The gorilla branch of `pgd_append_point()`: a full buffer gets a new one and the value is written again.
    /// `true` when a buffer was added (C returns its size).
    pub fn append(&mut self, number: u32) -> bool {
        if self.write(number) {
            return false;
        }
        self.add_buffer();
        let written = self.write(number);
        debug_assert!(written, "a fresh buffer takes any value");
        true
    }

    /// `gorilla_writer_entries()`.
    pub fn entries(&self) -> u32 {
        self.buffers.iter().map(|b| b.entries).sum()
    }

    /// `gorilla_writer_serialize()`: every buffer whole, `next` 1 on all but the last.
    pub fn serialize(&self) -> Vec<u8> {
        let last = self.buffers.len() - 1;
        self.buffers
            .iter()
            .enumerate()
            .flat_map(|(i, b)| b.to_bytes(u64::from(i != last)))
            .collect()
    }

    /// A reader of what was written so far (`gorilla_writer_get_reader()`).
    pub fn reader(&self) -> Reader<'_> {
        Reader::new(&self.buffers)
    }
}

/// A gorilla page loaded from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskPage {
    /// The chain as `next` links it.
    pub buffers: Vec<Buffer>,
    /// The entries `gorilla_buffer_patch()` counts over the chain.
    pub entries: u32,
    /// `pg->used` and `pg->slots`: the entries of the chain, truncated to 16 bits as C's page fields are.
    pub slots: u16,
}

/// `pgd_create_from_disk_data()` for GORILLA_32BIT with `gorilla_buffer_patch()`: `EmptyPage::InvalidChain` (C logs
/// "invalid gorilla disk page chain.") for a buffer whose `nbits` reaches the capacity or a `next` past the last whole
/// buffer; `EmptyPage::Unfit` below 4 bytes and where C would read past the page (a buffer or its bits beyond `size`),
/// which C does silently.
pub fn load(bytes: &[u8]) -> Result<DiskPage, EmptyPage> {
    if bytes.len() < 4 {
        return Err(EmptyPage::Unfit);
    }
    let nbuffers = bytes.len() / BUFFER_SIZE;
    let mut buffers = Vec::new();
    let mut entries: u32 = 0;
    loop {
        let start = buffers.len() * BUFFER_SIZE;
        let header = bytes
            .get(start..start + HEADER_SIZE)
            .ok_or(EmptyPage::Unfit)?;
        let word = |at: usize| {
            u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
        };
        let next = u64::from(word(0)) | u64::from(word(4)) << 32;
        let buffer_entries = word(8);
        let nbits = word(12);
        if nbits >= CAPACITY_BITS {
            return Err(EmptyPage::InvalidChain);
        }
        let available = bytes.len().min(start + BUFFER_SIZE) - start - HEADER_SIZE;
        if nbits as usize > available * 8 {
            return Err(EmptyPage::Unfit);
        }
        let mut b = Buffer {
            entries: buffer_entries,
            nbits,
            data: [0; DATA_WORDS],
        };
        for (i, w) in b.data.iter_mut().enumerate() {
            let at = start + HEADER_SIZE + i * 4;
            if let Some(word) = bytes.get(at..at + 4) {
                *w = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
            }
        }
        buffers.push(b);
        entries = entries.wrapping_add(buffer_entries);
        if next == 0 {
            break;
        }
        if buffers.len() == nbuffers {
            return Err(EmptyPage::InvalidChain);
        }
    }
    Ok(DiskPage {
        buffers,
        entries,
        slots: entries as u16,
    })
}

/// `load()` without the reason.
pub fn from_disk(bytes: &[u8]) -> Option<DiskPage> {
    load(bytes).ok()
}

/// `gorilla_reader_t`: the numbers of a chain in order.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buffers: &'a [Buffer],
    at: usize,
    entries: u32,
    index: u32,
    capacity: u32,
    position: u32,
    prev_number: u32,
    prev_xor_lzc: u32,
}

impl<'a> Reader<'a> {
    /// # Panics
    ///
    /// Without a buffer: a chain has at least one.
    pub fn new(buffers: &'a [Buffer]) -> Self {
        let first = &buffers[0];
        Reader {
            buffers,
            at: 0,
            entries: first.entries,
            index: 0,
            capacity: first.nbits,
            position: 0,
            prev_number: 0,
            prev_xor_lzc: 0,
        }
    }

    /// `gorilla_reader_read_bits()`: nothing past the buffer's written bits.
    fn bits(&mut self, nbits: u32) -> Option<u32> {
        if self.position > self.capacity || nbits > self.capacity - self.position {
            return None;
        }
        let v = read_bits(&self.buffers[self.at].data, self.position, nbits);
        self.position += nbits;
        Some(v)
    }

    /// `gorilla_reader_read()`: the next number, `None` at the end of the chain or where the bits run out.
    pub fn read(&mut self) -> Option<u32> {
        while self.index + 1 > self.entries {
            let b = &self.buffers[self.at];
            self.entries = b.entries;
            self.capacity = b.nbits;
            if self.index + 1 > self.entries {
                if self.at + 1 >= self.buffers.len() {
                    return None;
                }
                // gorilla_reader_init() of the next buffer
                let next = &self.buffers[self.at + 1];
                *self = Reader {
                    at: self.at + 1,
                    entries: next.entries,
                    capacity: next.nbits,
                    ..Reader::new(self.buffers)
                };
            } else {
                break;
            }
        }
        if self.index == 0 {
            let n = self.bits(32)?;
            self.index += 1;
            self.prev_number = n;
            return Some(n);
        }
        if self.bits(1)? == 1 {
            self.index += 1;
            return Some(self.prev_number);
        }
        let mut xor_lzc = self.prev_xor_lzc;
        if self.bits(1)? == 0 {
            xor_lzc = self.bits(5)?;
        }
        let xor_value = self.bits(32 - xor_lzc)?;
        let n = self.prev_number ^ xor_value;
        self.index += 1;
        self.prev_number = n;
        self.prev_xor_lzc = xor_lzc;
        Some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(next: u64, entries: u32, nbits: u32) -> [u8; BUFFER_SIZE] {
        let mut b = [0u8; BUFFER_SIZE];
        b[0..8].copy_from_slice(&next.to_le_bytes());
        b[8..12].copy_from_slice(&entries.to_le_bytes());
        b[12..16].copy_from_slice(&nbits.to_le_bytes());
        b
    }

    fn page(buffers: &[[u8; BUFFER_SIZE]], len: usize) -> Vec<u8> {
        let mut v: Vec<u8> = buffers.iter().flatten().copied().collect();
        v.resize(len, 0);
        v
    }

    fn entries(bytes: &[u8]) -> Result<u32, EmptyPage> {
        load(bytes).map(|p| p.entries)
    }

    /// `InvalidChain` exactly where `gorilla_buffer_patch()` returns false, in its order; `Unfit` only where C reads
    /// past the page or is handed less than 4 bytes.
    #[test]
    fn empty_pages_are_classified_as_c() {
        use EmptyPage::{InvalidChain, Unfit};
        let cases: [(&str, Vec<u8>, Result<u32, EmptyPage>); 14] = [
            (
                "next on the last buffer",
                page(&[buffer(1, 5, 10)], 512),
                Err(InvalidChain),
            ),
            (
                "next on the second of two",
                page(&[buffer(1, 5, 10), buffer(1, 5, 10)], 1024),
                Err(InvalidChain),
            ),
            (
                "one whole buffer of 1000 bytes",
                page(&[buffer(1, 5, 10), buffer(0, 5, 10)], 1000),
                Err(InvalidChain),
            ),
            (
                "nbits at capacity",
                page(&[buffer(0, 5, 3968)], 512),
                Err(InvalidChain),
            ),
            (
                "nbits at capacity later",
                page(&[buffer(1, 5, 10), buffer(0, 5, 3968)], 1024),
                Err(InvalidChain),
            ),
            (
                "nbits before next",
                page(&[buffer(1, 5, 4000)], 512),
                Err(InvalidChain),
            ),
            ("valid, one buffer", page(&[buffer(0, 5, 3967)], 512), Ok(5)),
            (
                "valid, two buffers",
                page(&[buffer(1, 5, 10), buffer(0, 7, 10)], 1024),
                Ok(12),
            ),
            ("under 4 bytes", vec![0; 3], Err(Unfit)),
            (
                "next past a short page",
                page(&[buffer(1, 5, 10)], 100),
                Err(Unfit),
            ),
            (
                "short page, bits inside",
                page(&[buffer(0, 5, 10)], 100),
                Ok(5),
            ),
            (
                "short page, bits past it",
                page(&[buffer(0, 5, 1000)], 100),
                Err(Unfit),
            ),
            ("header past the page", vec![0; 10], Err(Unfit)),
            (
                "short page, nbits at capacity",
                page(&[buffer(0, 5, 3968)], 100),
                Err(InvalidChain),
            ),
        ];
        for (name, bytes, want) in cases {
            assert_eq!(entries(&bytes), want, "{name}");
        }
    }
}
