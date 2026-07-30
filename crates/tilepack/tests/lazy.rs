//! Lazy-reader contract: the golden fixture reads identically through the
//! eager and sparse paths, staged prelude parsing reports exact needs, and
//! adversarial index entries are rejected at decode time (the lazy path's
//! substitute for `FrontMatter::parse`'s whole-index validation).

use std::io::Cursor;

use tilepack::{FrontMatter, HEADER_LEN, ParseError, Prelude, SparseReader, TilepackView};

const GOLDEN: &[u8] = include_bytes!("../../../fixtures/golden.tpc");

#[test]
fn golden_sparse_equals_eager() {
    let view = TilepackView::new(GOLDEN).unwrap();
    let mut sparse = SparseReader::new(Cursor::new(GOLDEN)).unwrap();

    let total = sparse.prelude().layout.total_tiles() as usize;
    assert_eq!(total as u64, view.fm.layout.total_tiles());
    for ord in 0..total {
        let eager = view.tile_by_ordinal(ord).map(<[u8]>::to_vec);
        let lazy = sparse.read_tile_by_ordinal(ord).unwrap();
        assert_eq!(lazy, eager, "ordinal {ord}");
    }
}

#[test]
fn prelude_parse_reports_staged_needs() {
    let err = Prelude::parse(&GOLDEN[..4]).unwrap_err();
    assert_eq!(err, ParseError::Truncated { needed: HEADER_LEN });

    let descriptors_end = match Prelude::parse(&GOLDEN[..HEADER_LEN]) {
        Err(ParseError::Truncated { needed }) => needed,
        other => panic!("expected truncated at descriptor table, got {other:?}"),
    };
    assert!(descriptors_end > HEADER_LEN);

    // Exactly the descriptor end suffices — the index is never required.
    let prelude = Prelude::parse(&GOLDEN[..descriptors_end]).unwrap();
    let fm = FrontMatter::parse(GOLDEN).unwrap();
    assert_eq!(prelude.front_matter_len(), fm.offsets()[0]);
}

#[test]
fn entry_validation_rejects_corruption() {
    let prelude = Prelude::parse(GOLDEN).unwrap();
    let fm_len = prelude.front_matter_len();
    let file_len = GOLDEN.len() as u64;

    let entry = |start: u64, end: u64| -> [u8; 16] {
        let mut e = [0u8; 16];
        e[..8].copy_from_slice(&start.to_le_bytes());
        e[8..].copy_from_slice(&end.to_le_bytes());
        e
    };

    // Valid present tile.
    assert_eq!(
        prelude.tile_range_from_entry(&entry(fm_len, fm_len + 10), Some(file_len)).unwrap(),
        Some(fm_len..fm_len + 10)
    );
    // Absent tile: equal adjacent offsets.
    assert_eq!(prelude.tile_range_from_entry(&entry(fm_len, fm_len), Some(file_len)).unwrap(), None);
    // Backwards range.
    assert!(matches!(
        prelude.tile_range_from_entry(&entry(fm_len + 10, fm_len), Some(file_len)),
        Err(ParseError::BadIndex(_))
    ));
    // Points into the front matter.
    assert!(matches!(
        prelude.tile_range_from_entry(&entry(0, 10), Some(file_len)),
        Err(ParseError::BadIndex(_))
    ));
    // Runs past the end of the file (only checkable when the length is known).
    assert!(matches!(
        prelude.tile_range_from_entry(&entry(fm_len, file_len + 1), Some(file_len)),
        Err(ParseError::BadIndex(_))
    ));
    assert_eq!(
        prelude.tile_range_from_entry(&entry(fm_len, file_len + 1), None).unwrap(),
        Some(fm_len..file_len + 1),
        "without a known file length the oversize entry is the caller's to catch"
    );
}
