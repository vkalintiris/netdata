//! Extents (`struct rrdeng_df_extent_header` and trailer in `rrddiskprotocol.h`): up to 109 pages behind their
//! descriptors, compressed as one stream, CRC-checked, padded to 4096 bytes. Encoding follows `rrdengine.c`
//! (`datafile_extent_build()`), decoding and C's error outcomes `pdc.c` (`epdl_populate_pages_from_extent_data()`).
//! Brief `knowledge/brief-dbengine-s0.md` §2.3 in the status repository.

use super::crc::{crc_bytes, crc_matches, crc32};
use super::descriptor::{
    DESCRIPTOR_SIZE, GORILLA_BUFFER_SIZE, PAGE_TYPE_GORILLA_32BIT, PageDescriptor,
};
use super::{BLOCK_SIZE, MAX_EXTENT_UNCOMPRESSED_SIZE, MAX_PAGES_PER_EXTENT};

/// `RRDENG_COMPRESSION_*`.
pub const COMPRESSION_NONE: u8 = 0;
pub const COMPRESSION_LZ4: u8 = 1;
pub const COMPRESSION_ZSTD: u8 = 2;

const HEADER_SIZE: usize = 6;
const TRAILER_SIZE: usize = 4;
/// The smallest and largest extents C accepts: one empty page, and 109 full gorilla pages.
const MIN_EXTENT_SIZE: usize = HEADER_SIZE + DESCRIPTOR_SIZE + TRAILER_SIZE;
const MAX_EXTENT_SIZE: usize = HEADER_SIZE
    + DESCRIPTOR_SIZE * MAX_PAGES_PER_EXTENT
    + MAX_EXTENT_UNCOMPRESSED_SIZE
    + TRAILER_SIZE;
/// `rrdeng_valid_extent_disk_size()`: an extent's size as a journal records it.
pub fn valid_disk_size(size: u32) -> bool {
    (MIN_EXTENT_SIZE..=MAX_EXTENT_SIZE).contains(&(size as usize))
}

/// ZSTD's level in `dbengine_compress()`.
const ZSTD_LEVEL: i32 = 3;

/// An extent C would not read at all ("header is INVALID"): no page of it is cached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderInvalid(pub &'static str);

impl std::fmt::Display for HeaderInvalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "extent header is invalid: {}", self.0)
    }
}

impl std::error::Error for HeaderInvalid {}

/// A decoded extent.
#[derive(Debug, Clone)]
pub struct Extent {
    pub compression: u8,
    pub descriptors: Vec<PageDescriptor>,
    pub crc_ok: bool,
    /// C's `have_read_error`: a bad CRC, an impossible page length, or a failed decompression. Every page of the
    /// extent then validates as invalid.
    pub read_error: bool,
    /// The pages' bytes: the stored payload, or what decompression produced.
    payload: Vec<u8>,
}

/// What a page of an extent holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSlot<'a> {
    Page(&'a [u8]),
    /// A page with no length or a start in the first second: C logs "is EMPTY" and does not cache it.
    Skipped,
    /// A page past the payload: `PGD_EMPTY`.
    Empty,
}

impl Extent {
    /// How many bytes the pages have: the stored payload's, or what decompression produced.
    pub fn payload_len(&self) -> usize {
        self.payload.len()
    }

    /// The page of descriptor `i`: pages sit one after the other in descriptor order.
    ///
    /// # Panics
    ///
    /// When `i` is not a descriptor's index.
    pub fn page(&self, i: usize) -> PageSlot<'_> {
        let d = &self.descriptors[i];
        // C's u32 offset, which wraps
        let start = self.descriptors[..i]
            .iter()
            .fold(0u32, |at, d| at.wrapping_add(d.page_length)) as usize;
        let len = d.page_length as usize;
        if len == 0 || d.start_time_ut / 1_000_000 == 0 {
            return PageSlot::Skipped;
        }
        let payload = self.payload.len();
        if len > payload || start > payload - len {
            return PageSlot::Empty;
        }
        PageSlot::Page(&self.payload[start..start + len])
    }
}

/// Decodes the `bytes` of an extent (its size as the journal records it, padding excluded).
pub fn decode(bytes: &[u8]) -> Result<Extent, HeaderInvalid> {
    let len = bytes.len();
    if !u32::try_from(len).is_ok_and(valid_disk_size) {
        return Err(HeaderInvalid("size"));
    }
    let payload_length = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let compression = bytes[4];
    let pages = bytes[5] as usize;
    if pages == 0 || pages > MAX_PAGES_PER_EXTENT {
        return Err(HeaderInvalid("number of pages"));
    }
    if !matches!(
        compression,
        COMPRESSION_NONE | COMPRESSION_LZ4 | COMPRESSION_ZSTD
    ) {
        return Err(HeaderInvalid("compression algorithm"));
    }
    let descriptors_end = HEADER_SIZE + DESCRIPTOR_SIZE * pages;
    if len < descriptors_end + TRAILER_SIZE
        || payload_length != len - TRAILER_SIZE - descriptors_end
    {
        return Err(HeaderInvalid("payload length"));
    }
    let descriptors: Vec<PageDescriptor> = (0..pages)
        .filter_map(|i| PageDescriptor::decode(&bytes[HEADER_SIZE + i * DESCRIPTOR_SIZE..]))
        .collect();
    let stored = &bytes[descriptors_end..descriptors_end + payload_length];
    let crc_at = descriptors_end + payload_length;
    let crc_ok = crc_matches(
        &bytes[crc_at..crc_at + TRAILER_SIZE],
        crc32(&bytes[..crc_at]),
    );
    let mut read_error = !crc_ok;
    // C's u32 sum, which wraps
    let uncompressed = descriptors
        .iter()
        .fold(0u32, |sum, d| sum.wrapping_add(d.page_length)) as usize;
    let payload = if compression == COMPRESSION_NONE {
        stored.to_vec()
    } else {
        // page lengths a compressed extent cannot hold
        let impossible = descriptors.iter().any(|d| {
            let len = d.page_length as usize;
            len > BLOCK_SIZE
                && (d.page_type != PAGE_TYPE_GORILLA_32BIT
                    || !(len - BLOCK_SIZE).is_multiple_of(GORILLA_BUFFER_SIZE))
        });
        if impossible || uncompressed > MAX_EXTENT_UNCOMPRESSED_SIZE {
            read_error = true;
            Vec::new()
        } else {
            match decompress(compression, stored, uncompressed) {
                Some(out) if !out.is_empty() => out,
                _ => {
                    read_error = true;
                    Vec::new()
                }
            }
        }
    };
    Ok(Extent {
        compression,
        descriptors,
        crc_ok,
        read_error,
        payload,
    })
}

/// `dbengine_decompress()`: at most `capacity` bytes; C does not compare the count with the pages' total.
fn decompress(compression: u8, src: &[u8], capacity: usize) -> Option<Vec<u8>> {
    match compression {
        COMPRESSION_LZ4 => {
            let mut out = vec![0u8; capacity];
            let n = lz4_flex::block::decompress_into(src, &mut out).ok()?;
            out.truncate(n);
            Some(out)
        }
        COMPRESSION_ZSTD => zstd::bulk::decompress(src, capacity).ok(),
        _ => None,
    }
}

/// An encoded extent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    /// The extent padded with zeros to a multiple of 4096 bytes (C leaves stale bytes there).
    pub bytes: Vec<u8>,
    /// The extent without its padding: what the journal records as its size.
    pub size_bytes: usize,
}

/// Builds an extent of `pages` (each descriptor with its page bytes, at most 109) with `compression`, falling back
/// to none when it does not make the payload smaller, as `datafile_extent_build()` does.
///
/// # Panics
///
/// With no page or more than 109.
pub fn encode(pages: &[(PageDescriptor, &[u8])], compression: u8) -> Encoded {
    assert!(
        (1..=MAX_PAGES_PER_EXTENT).contains(&pages.len()),
        "an extent holds 1 to 109 pages"
    );
    let payload: Vec<u8> = pages
        .iter()
        .flat_map(|(_, bytes)| bytes.iter().copied())
        .collect();
    let compressed = match compression {
        COMPRESSION_ZSTD => zstd::bulk::compress(&payload, ZSTD_LEVEL).ok(),
        COMPRESSION_LZ4 => Some(lz4_flex::block::compress(&payload)),
        _ => None,
    }
    .filter(|c| !c.is_empty() && c.len() < payload.len());
    let (compression, stored) = match &compressed {
        Some(c) => (compression, c.as_slice()),
        None => (COMPRESSION_NONE, payload.as_slice()),
    };
    let mut bytes = Vec::with_capacity(
        HEADER_SIZE + DESCRIPTOR_SIZE * pages.len() + stored.len() + TRAILER_SIZE,
    );
    bytes.extend_from_slice(&(stored.len() as u32).to_le_bytes());
    bytes.push(compression);
    bytes.push(pages.len() as u8);
    for (d, _) in pages {
        bytes.extend_from_slice(&d.encode());
    }
    bytes.extend_from_slice(stored);
    let crc = crc32(&bytes);
    bytes.extend_from_slice(&crc_bytes(crc));
    let size_bytes = bytes.len();
    bytes.resize(size_bytes.div_ceil(BLOCK_SIZE) * BLOCK_SIZE, 0);
    Encoded { bytes, size_bytes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbengine::format::descriptor::PAGE_TYPE_ARRAY_32BIT;

    const T: u64 = 1_700_000_000_000_000;

    fn array_page(len: usize, fill: u8) -> (PageDescriptor, Vec<u8>) {
        let d = PageDescriptor::array(
            PAGE_TYPE_ARRAY_32BIT,
            [fill; 16],
            len as u32,
            T,
            T + 1_000_000,
        );
        (d, vec![fill; len])
    }

    fn extent(pages: &[(PageDescriptor, Vec<u8>)], compression: u8) -> Encoded {
        let refs: Vec<(PageDescriptor, &[u8])> =
            pages.iter().map(|(d, b)| (*d, b.as_slice())).collect();
        encode(&refs, compression)
    }

    #[test]
    fn compressions_round_trip_and_fall_back() {
        let pages = vec![array_page(4096, 7), array_page(400, 9)];
        for compression in [COMPRESSION_ZSTD, COMPRESSION_LZ4] {
            let e = extent(&pages, compression);
            assert_eq!(e.bytes.len() % BLOCK_SIZE, 0);
            let x = decode(&e.bytes[..e.size_bytes]).unwrap();
            assert_eq!(x.compression, compression);
            assert!(x.crc_ok && !x.read_error);
            assert_eq!(x.page(0), PageSlot::Page(&pages[0].1));
            assert_eq!(x.page(1), PageSlot::Page(&pages[1].1));
        }
        // random bytes do not compress: stored as they are
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let noise: Vec<u8> = (0..400)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 32) as u8
            })
            .collect();
        let d = PageDescriptor::array(PAGE_TYPE_ARRAY_32BIT, [1; 16], 400, T, T + 1_000_000);
        let e = encode(&[(d, &noise)], COMPRESSION_ZSTD);
        assert_eq!(e.bytes[4], COMPRESSION_NONE);
        assert_eq!(
            decode(&e.bytes[..e.size_bytes]).unwrap().page(0),
            PageSlot::Page(&noise[..])
        );
    }

    #[test]
    fn invalid_headers_as_c() {
        let e = extent(&[array_page(16, 1)], COMPRESSION_NONE);
        let good = e.bytes[..e.size_bytes].to_vec();
        assert!(decode(&good).is_ok());
        assert_eq!(decode(&good[..46]).unwrap_err(), HeaderInvalid("size"));
        let mut b = good.clone();
        b[5] = 0;
        assert_eq!(decode(&b).unwrap_err(), HeaderInvalid("number of pages"));
        let mut b = good.clone();
        b[4] = 3;
        assert_eq!(
            decode(&b).unwrap_err(),
            HeaderInvalid("compression algorithm")
        );
        let mut b = good.clone();
        b[0] = b[0].wrapping_add(1);
        assert_eq!(decode(&b).unwrap_err(), HeaderInvalid("payload length"));
        let mut b = good.clone();
        b[5] = 110;
        assert_eq!(decode(&b).unwrap_err(), HeaderInvalid("number of pages"));
    }

    #[test]
    fn a_compressed_extent_whose_pages_exceed_the_maximum_is_a_read_error() {
        // one gorilla page of 4096 + 1000 buffers: allowed alone, but past 109 pages' worth
        let d = PageDescriptor::gorilla([1; 16], 4096 + 1000 * 512, 1_000_000, 1, 0);
        let mut b = 4u32.to_le_bytes().to_vec();
        b.extend_from_slice(&[COMPRESSION_ZSTD, 1]);
        b.extend_from_slice(&d.encode());
        b.extend_from_slice(&[0; 4]);
        let crc = crc_bytes(crc32(&b));
        b.extend_from_slice(&crc);
        let x = decode(&b).unwrap();
        assert!(x.crc_ok && x.read_error);
    }

    #[test]
    fn read_errors_as_c() {
        // a flipped byte fails the CRC: read on, every page invalid
        let e = extent(&[array_page(16, 1)], COMPRESSION_NONE);
        let mut b = e.bytes[..e.size_bytes].to_vec();
        b[50] ^= 1;
        let x = decode(&b).unwrap();
        assert!(!x.crc_ok && x.read_error);
        // a corrupt compressed stream
        let e = extent(&[array_page(4096, 1)], COMPRESSION_ZSTD);
        let mut b = e.bytes[..e.size_bytes].to_vec();
        let at = HEADER_SIZE + DESCRIPTOR_SIZE + 2;
        b[at] ^= 0xff;
        let crc_at = b.len() - TRAILER_SIZE;
        let crc = crc32(&b[..crc_at]);
        b[crc_at..].copy_from_slice(&crc_bytes(crc));
        assert!(decode(&b).unwrap().read_error);
        // page lengths a compressed extent cannot hold
        let long = |len: usize, page_type: u8| {
            let mut e = extent(&[array_page(4096, 1)], COMPRESSION_ZSTD).bytes;
            let size = u32::from_le_bytes([e[0], e[1], e[2], e[3]]) as usize
                + HEADER_SIZE
                + DESCRIPTOR_SIZE
                + 4;
            e[HEADER_SIZE] = page_type;
            e[HEADER_SIZE + 17..HEADER_SIZE + 21].copy_from_slice(&(len as u32).to_le_bytes());
            let crc_at = size - TRAILER_SIZE;
            let crc = crc32(&e[..crc_at]);
            e[crc_at..size].copy_from_slice(&crc_bytes(crc));
            decode(&e[..size]).unwrap().read_error
        };
        assert!(long(4100, PAGE_TYPE_ARRAY_32BIT));
        assert!(long(4600, PAGE_TYPE_GORILLA_32BIT));
        // the right gorilla length only fails to decompress to that much, which C does not check
        assert!(!long(4608, PAGE_TYPE_GORILLA_32BIT));
    }

    #[test]
    fn page_offsets_wrap_and_starts_count_in_seconds_as_c() {
        // an uncompressed extent around `descriptors` and `payload`, with its CRC
        let raw = |descriptors: &[PageDescriptor], payload: &[u8]| {
            let mut b = (payload.len() as u32).to_le_bytes().to_vec();
            b.extend_from_slice(&[COMPRESSION_NONE, descriptors.len() as u8]);
            for d in descriptors {
                b.extend_from_slice(&d.encode());
            }
            b.extend_from_slice(payload);
            let crc = crc_bytes(crc32(&b));
            b.extend_from_slice(&crc);
            b
        };
        let d = |len: u32, start_ut: u64| {
            PageDescriptor::array(
                PAGE_TYPE_ARRAY_32BIT,
                [1; 16],
                len,
                start_ut,
                start_ut + 3_000_000,
            )
        };
        // two lengths that wrap C's u32 offset back to 4
        let b = raw(
            &[
                d(0x8000_0000, 1_000_000),
                d(0x8000_0004, 1_000_000),
                d(4, 1_000_000),
            ],
            &[1, 2, 3, 4, 5, 6, 7, 8],
        );
        let x = decode(&b).unwrap();
        assert_eq!((x.page(0), x.page(1)), (PageSlot::Empty, PageSlot::Empty));
        assert_eq!(x.page(2), PageSlot::Page(&[5, 6, 7, 8]));
        // a start inside the first second is no start
        let b = raw(&[d(4, 999_999)], &[1, 2, 3, 4]);
        assert_eq!(decode(&b).unwrap().page(0), PageSlot::Skipped);
    }

    #[test]
    fn pages_past_the_payload_are_empty() {
        let (mut d, bytes) = array_page(16, 1);
        let e = encode(&[(d, &bytes)], COMPRESSION_NONE);
        let mut b = e.bytes[..e.size_bytes].to_vec();
        d.page_length = 20;
        b[HEADER_SIZE..HEADER_SIZE + DESCRIPTOR_SIZE].copy_from_slice(&d.encode());
        let x = decode(&b).unwrap();
        assert_eq!(x.page(0), PageSlot::Empty);
        let skipped = PageDescriptor {
            page_length: 0,
            ..d
        };
        b[HEADER_SIZE..HEADER_SIZE + DESCRIPTOR_SIZE].copy_from_slice(&skipped.encode());
        assert_eq!(decode(&b).unwrap().page(0), PageSlot::Skipped);
    }
}
