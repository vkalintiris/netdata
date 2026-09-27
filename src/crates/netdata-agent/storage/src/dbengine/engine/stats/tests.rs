use super::*;
use crate::dbengine::format::journal_v2::{ExtentEntry, MetricEntry};

/// A page `delta_start_s..delta_end_s` into its file holding `page_length` bytes.
fn page(delta_start_s: u32, delta_end_s: u32, page_length: u16) -> PageEntry {
    PageEntry {
        delta_start_s,
        delta_end_s,
        page_length,
        ..PageEntry::default()
    }
}

/// A metric's page list where the file places it: its offset, the entries its header claims, its pages.
struct List {
    offset: u32,
    entries: u32,
    pages: Vec<PageEntry>,
}

/// A v2 file of `size` bytes starting at 1000 s: the header, `extents` right after it, the metric list after them
/// (its offset and count as given, `None` for where the entries are written), and each metric's page list.
fn file(
    size: usize,
    extents: &[(u32, u8)],
    metric_list: Option<(u32, u32)>,
    lists: &[List],
) -> Vec<u8> {
    let mut b = vec![0u8; size];
    let extent_offset = HEADER_SIZE as u32;
    let metric_offset = extent_offset + (extents.len() * EXTENT_SIZE) as u32;
    let (metric_offset, metric_count) = metric_list.unwrap_or((metric_offset, lists.len() as u32));
    let h = Header {
        start_time_ut: 1000 * USEC_PER_SEC,
        extent_count: extents.len() as u32,
        extent_offset,
        metric_count,
        metric_offset,
        ..Header::default()
    };
    b[..HEADER_SIZE].copy_from_slice(&h.encode());
    for (i, &(datafile_size, pages)) in extents.iter().enumerate() {
        let at = extent_offset as usize + i * EXTENT_SIZE;
        let e = ExtentEntry {
            datafile_size,
            pages,
            ..ExtentEntry::default()
        };
        b[at..at + EXTENT_SIZE].copy_from_slice(&e.encode());
    }
    for (i, list) in lists.iter().enumerate() {
        let at = HEADER_SIZE + extents.len() * EXTENT_SIZE + i * METRIC_SIZE;
        let m = MetricEntry {
            page_offset: list.offset,
            ..MetricEntry::default()
        };
        if at + METRIC_SIZE <= size {
            b[at..at + METRIC_SIZE].copy_from_slice(&m.encode());
        }
        let at = list.offset as usize;
        if at + PAGE_HEADER_SIZE <= size {
            let ph = PageHeader {
                entries: list.entries,
                ..PageHeader::default()
            };
            b[at..at + PAGE_HEADER_SIZE].copy_from_slice(&ph.encode());
        }
        for (j, p) in list.pages.iter().enumerate() {
            let at = at + PAGE_HEADER_SIZE + j * PAGE_SIZE;
            b[at..at + PAGE_SIZE].copy_from_slice(&p.encode());
        }
    }
    b
}

/// Where the page lists of a file with `extents` extents and `metrics` metrics start.
fn lists_at(extents: usize, metrics: usize) -> u32 {
    (HEADER_SIZE + extents * EXTENT_SIZE + metrics * METRIC_SIZE) as u32
}

/// The totals of one file walked with 4-byte points and a 1 s granularity.
fn walk(data: &[u8]) -> SizeStats {
    let mut s = SizeStats::default();
    s.add_file(data, data.len() as u64, 4, 1);
    s
}

/// A file's extents, metrics and pages add up as C adds them: a page of fewer than two points spans the granularity,
/// one of more spans its points' update every; the first time is a page's start less its update every.
#[test]
fn a_file_adds_its_extents_metrics_and_pages() {
    let at = lists_at(2, 2);
    let data = file(
        1024,
        &[(100, 2), (50, 1)],
        None,
        &[
            List {
                offset: at,
                entries: 2,
                pages: vec![page(0, 10, 0), page(10, 20, 16)],
            },
            List {
                offset: at + (PAGE_HEADER_SIZE + 2 * PAGE_SIZE) as u32,
                entries: 1,
                pages: vec![page(5, 5, 0)],
            },
        ],
    );
    let mut got = walk(&data);
    got.derive();
    let want = SizeStats {
        extents: 2,
        extents_pages: 3,
        extents_compressed_bytes: 150,
        metrics: 2,
        metrics_pages: 3,
        points: 4,
        pages_uncompressed_bytes: 16,
        // 10 + 1, 10 + 10 / 3, 0 + 1
        pages_duration_secs: 25,
        single_point_pages: 2,
        first_time_s: 999,
        last_time_s: 1020,
        database_retention_secs: 21,
        average_page_size_bytes: 16.0 / 3.0,
        average_compression_savings: 100.0 - 150.0 * 100.0 / 16.0,
        average_point_duration_secs: 25.0 / 4.0,
        average_metric_retention_secs: 12.5,
        estimated_concurrently_collected_metrics: 1,
        ephemeral_metrics_per_day_percent: (200.0 - 100.0) / (21.0 / 86400.0),
        ..SizeStats::default()
    };
    assert_eq!(got, want);
}

/// C's bounds checks against the file's size: an extent list past the end adds no extents; a metric list past the
/// end adds nothing more; a metric counts before its page list is checked, and a page header or list past the end
/// adds no pages.
#[test]
fn out_of_bounds_lists_are_passed_over_as_c() {
    let pages = |offset, entries| List {
        offset,
        entries,
        pages: vec![],
    };
    // the extent list claims more entries than the file holds
    let mut data = file(512, &[(100, 2)], None, &[pages(lists_at(1, 1), 0)]);
    let mut h = Header::decode(data.first_chunk().unwrap());
    h.extent_count = 1000;
    data[..HEADER_SIZE].copy_from_slice(&h.encode());
    let got = walk(&data);
    assert_eq!((got.extents, got.metrics), (0, 1), "extents past the end");

    // the metric list starts past the end
    let data = file(512, &[(100, 2)], Some((513, 1)), &[]);
    let got = walk(&data);
    assert_eq!(
        (got.extents, got.metrics),
        (1, 0),
        "a metric list past the end"
    );

    // the metric list's entries do not fit
    let data = file(512, &[], Some((HEADER_SIZE as u32, 13)), &[]);
    assert_eq!(walk(&data).metrics, 0, "a metric list longer than the file");

    // page headers: past the end, 27 bytes before it; a list longer than its room
    let size = 512u32;
    let data = file(
        size as usize,
        &[],
        None,
        &[
            pages(size + 1, 0),
            pages(size - 27, 0),
            pages(lists_at(0, 3), 18),
        ],
    );
    let got = walk(&data);
    assert_eq!((got.metrics, got.metrics_pages), (3, 0));

    // a header with one entry and room for exactly one page counts it; one byte less of room, none
    for (offset, want) in [(size - 48, 1), (size - 47, 0)] {
        let data = file(size as usize, &[], None, &[pages(offset, 1)]);
        assert_eq!(walk(&data).metrics_pages, want, "at {offset}");
    }

    // a list that just fits counts its entries
    let at = lists_at(0, 1);
    let room = (size - at - PAGE_HEADER_SIZE as u32) / PAGE_SIZE as u32;
    let data = file(size as usize, &[], None, &[pages(at, room)]);
    let got = walk(&data);
    assert_eq!(
        (got.metrics_pages, got.single_point_pages),
        (u64::from(room), u64::from(room))
    );

    // a file shorter than its header adds nothing
    assert_eq!(walk(&data[..HEADER_SIZE - 1]), SizeStats::default());

    // a file shorter than its size (a truncated one) keeps what the walk counted before the read failed, as C's
    // fault does
    let at = lists_at(1, 1);
    let data = file(512, &[(100, 2)], None, &[pages(at, 1)]);
    let mut got = SizeStats::default();
    got.add_file(&data[..at as usize], 512, 4, 1);
    assert_eq!((got.extents, got.metrics, got.metrics_pages), (1, 1, 0));
}

/// The update every of a page of several points divides its span unsigned, as C divides a `time_t` by a `size_t`:
/// a page ending before its start gets a huge one.
#[test]
fn the_update_every_divides_unsigned() {
    let at = lists_at(0, 1);
    let one = |p| {
        walk(&file(
            512,
            &[],
            None,
            &[List {
                offset: at,
                entries: 1,
                pages: vec![p],
            }],
        ))
    };
    // two points 10 s back: the span, -10, as a size_t over 1
    let got = one(page(20, 10, 8));
    assert_eq!(
        (got.pages_duration_secs, got.first_time_s, got.last_time_s),
        (-20, 1030, 1010)
    );
    // three points: (2^64 - 10) / 2
    let got = one(page(20, 10, 12));
    let update_every = ((-10i64) as u64 / 2) as i64;
    assert_eq!(got.pages_duration_secs, -10 + update_every);
    assert_eq!(got.first_time_s, 1020 - update_every);
}

/// With fewer concurrently collected metrics estimated than one, the ephemeral percentage divides by 0: C's infinity.
#[test]
fn an_estimate_of_no_metrics_gives_an_infinite_ephemeral_percentage() {
    let mut s = SizeStats {
        metrics: 1,
        pages_duration_secs: 1,
        first_time_s: 1,
        last_time_s: 1_000_001,
        ..SizeStats::default()
    };
    s.derive();
    assert_eq!(s.estimated_concurrently_collected_metrics, 0);
    assert_eq!(s.ephemeral_metrics_per_day_percent, f64::INFINITY);
    // no retention: no estimate
    let mut s = SizeStats {
        metrics: 1,
        pages_duration_secs: 1,
        first_time_s: 5,
        last_time_s: 5,
        ..SizeStats::default()
    };
    s.derive();
    assert_eq!(
        (
            s.average_metric_retention_secs,
            s.estimated_concurrently_collected_metrics,
            s.ephemeral_metrics_per_day_percent
        ),
        (1.0, 0, 0.0)
    );
}
