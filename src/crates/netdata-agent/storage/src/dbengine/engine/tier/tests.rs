use super::*;
use crate::dbengine::engine::io::open_for_io;
use std::path::Path;

fn data_file(dir: &Path, pos: u64, journal: bool) -> DataFile {
    let file = |name: &str| open_for_io(&dir.join(name), true, false).unwrap();
    DataFile::new(1, file("d"), pos, journal.then(|| file("j")), 4096)
}

/// `datafile_is_full()`: no room for the extent, a failed file, or no journal to write.
#[test]
fn files_are_full_as_c_says() {
    let dir = tempfile::tempdir().unwrap();
    const TARGET: u64 = 524_288;
    assert!(!data_file(dir.path(), 520_192, true).is_full(4096, TARGET));
    assert!(data_file(dir.path(), 520_193, true).is_full(4096, TARGET));
    let failed = data_file(dir.path(), 4096, true);
    failed.mark_failed();
    assert!(failed.is_full(4096, TARGET));
    assert!(data_file(dir.path(), 4096, false).is_full(4096, TARGET));
}

/// `pgc_open_add_hot_page()`: a page at a start already held replaces it only when it ends later; a file's pages
/// come in the order they joined, without the ones another file's page replaced, and leave together.
#[test]
fn open_pages_keep_the_longer_page_at_a_start() {
    const A: [u8; 16] = [0xaa; 16];
    const B: [u8; 16] = [0xbb; 16];
    let page = |end_time_s, fileno, block| OpenPage {
        end_time_s,
        update_every_s: 1,
        fileno,
        block,
        bytes: 100,
    };
    let mut open = OpenList::default();
    open.add(A, 10, page(19, 1, 1), (0, 0));
    open.add(A, 10, page(29, 2, 1), (8192, 0));
    open.add(A, 10, page(24, 3, 1), (8192, 0));
    assert_eq!(open.pages(&A).unwrap()[&10].fileno, 2);
    assert!(open.pages(&[0xcc; 16]).is_none());
    assert!(open.file_pages(1).is_empty());
    assert!(open.file_pages(3).is_empty());

    open.add(B, 30, page(39, 2, 3), (12288, 0));
    open.add(B, 20, page(29, 2, 2), (4096, 1));
    let order: Vec<([u8; 16], i64)> = open
        .file_pages(2)
        .iter()
        .map(|p| (p.uuid, p.start_time_s))
        .collect();
    assert_eq!(order, [(B, 20), (A, 10), (B, 30)]);
    open.remove_file(2);
    assert!(open.pages(&A).is_none() && open.pages(&B).is_none());
    assert!(open.file_pages(2).is_empty());
}
