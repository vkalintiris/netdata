use super::*;

fn cursor_at(ts: i64) -> Cursor {
    Cursor {
        timestamp_ns: ts,
        file_seq: 1,
        position: ts as u32,
    }
}

fn timestamps(cursors: &[Cursor]) -> Vec<i64> {
    cursors.iter().map(|c| c.timestamp_ns).collect()
}

#[test]
fn page_merge_backward_keeps_nearest_and_finalize_flags_more() {
    // Backward: closest-to-anchor is the largest (newest) cursor. With
    // limit 2 the bound is 3; merge keeps the nearest 3, finalize takes 2.
    let a = PageShard {
        cursors: vec![cursor_at(50), cursor_at(20)],
        has_opposite: false,
    };
    let b = PageShard {
        cursors: vec![cursor_at(40), cursor_at(30), cursor_at(10)],
        has_opposite: true,
    };

    let merged = PageShard::merge(vec![a, b], Direction::Backward, Some(3));
    assert_eq!(timestamps(&merged.cursors), vec![50, 40, 30]);
    assert!(merged.has_opposite);

    let selected = finalize_page(merged, Direction::Backward, 2);
    // Page is newest-first; backward is already in that order.
    assert_eq!(timestamps(&selected.cursors), vec![50, 40]);
    assert!(
        selected.has_older,
        "a 3rd candidate (30) lies beyond the page"
    );
    assert!(
        selected.has_newer,
        "has_opposite -> rows newer than the anchor"
    );
}

#[test]
fn page_merge_forward_orders_oldest_first_and_outputs_newest_first() {
    // Forward: closest-to-anchor is the smallest (oldest) cursor; the page
    // is reversed to newest-first for output, and the flags swap sides.
    let a = PageShard {
        cursors: vec![cursor_at(50), cursor_at(20)],
        has_opposite: true,
    };
    let b = PageShard {
        cursors: vec![cursor_at(10), cursor_at(30), cursor_at(40)],
        has_opposite: false,
    };

    let merged = PageShard::merge(vec![a, b], Direction::Forward, Some(3));
    assert_eq!(timestamps(&merged.cursors), vec![10, 20, 30]);
    assert!(merged.has_opposite);

    let selected = finalize_page(merged, Direction::Forward, 2);
    // Nearest 2 are [10, 20] (oldest-first), reversed to newest-first.
    assert_eq!(timestamps(&selected.cursors), vec![20, 10]);
    assert!(
        selected.has_newer,
        "a 3rd candidate (30) lies beyond the page"
    );
    assert!(
        selected.has_older,
        "has_opposite -> rows older than the anchor"
    );
}
