//! Malformed and truncated input must never panic, must never allocate on an
//! untrusted count, and must return precise errors.

use proptest::prelude::*;
use tilepack::descriptor::{Codec, GroupDescriptor, GroupFlags, Radiometry, SampleType, Semantic};
use tilepack::error::ParseError;
use tilepack::{FrontMatter, TilepackView, Writer, WriterParams, required_len};

/// A small, valid multi-group cubemap file for mutation.
fn valid_file() -> Vec<u8> {
    let params = WriterParams {
        face_count: 6,
        levels: 2,
        tile_size: 64,
        root_w: 128,
        root_h: 128,
    };
    let groups = vec![
        GroupDescriptor {
            semantic: Semantic::Rgb,
            codec: Codec::Webp,
            sample: SampleType::Rgb8,
            flags: GroupFlags::default(),
            level_count: 2,
            level_skip: 0,
            radiometry: Radiometry::default(),
        },
        GroupDescriptor {
            semantic: Semantic::Depth,
            codec: Codec::Depthpack,
            sample: SampleType::U16,
            flags: GroupFlags::new(true, false),
            level_count: 1,
            level_skip: 0,
            radiometry: Radiometry {
                scale: 0.001,
                offset: 0.0,
                nodata: 0,
                min: 0,
                max: 65000,
                unit: Radiometry::unit_from_str("m"),
            },
        },
    ];
    let mut w = Writer::new(params, groups).unwrap();
    let total = w.total_tiles();
    for ord in 0..total {
        w.set_ordinal(ord, vec![ord as u8; (ord % 7) + 1]).unwrap();
    }
    w.finish().unwrap()
}

#[test]
fn bad_magic() {
    let mut f = valid_file();
    f[0] = b'X';
    assert_eq!(FrontMatter::parse(&f).unwrap_err(), ParseError::BadMagic);
}

#[test]
fn bad_version() {
    let mut f = valid_file();
    f[4] = 99;
    assert!(matches!(FrontMatter::parse(&f), Err(ParseError::BadVersion { found: 99, .. })));
}

#[test]
fn bad_face_count() {
    for fc in [0u8, 2, 3, 5, 7, 255] {
        let mut f = valid_file();
        f[5] = fc;
        assert_eq!(FrontMatter::parse(&f).unwrap_err(), ParseError::BadFaceCount(fc), "face_count {fc}");
    }
}

#[test]
fn truncation_is_staged_and_monotonic() {
    let f = valid_file();
    let fm = FrontMatter::parse(&f).unwrap();
    let fm_len = (fm.offsets().len() * 8) + 24 + 48 * 2;

    let mut last_needed = 0usize;
    for len in 0..fm_len {
        match FrontMatter::parse(&f[..len]) {
            Err(ParseError::Truncated { needed }) => {
                assert!(needed > len, "needed {needed} must exceed have {len}");
                assert!(needed <= fm_len, "needed {needed} must not exceed front matter {fm_len}");
                assert!(needed >= last_needed, "needed must not shrink");
                last_needed = needed;
            }
            other => panic!("prefix len {len} should be Truncated, got {other:?}"),
        }
        // required_len tracks the same staged target and never panics.
        let need = required_len(&f[..len]).unwrap();
        assert!(need >= len.min(fm_len) || need == 24);
    }
    // Exactly the front-matter length parses.
    assert!(FrontMatter::parse(&f[..fm_len]).is_ok());
}

#[test]
fn whole_file_truncation_reports_file_len() {
    let f = valid_file();
    let fm = FrontMatter::parse(&f).unwrap();
    let file_len = fm.file_len() as usize;
    // Front matter present but body cut short.
    let cut = file_len - 1;
    assert!(matches!(TilepackView::new(&f[..cut]), Err(ParseError::Truncated { .. })));
    assert!(TilepackView::new(&f).is_ok());
}

#[test]
fn decreasing_offset_rejected() {
    let f = valid_file();
    let fm = FrontMatter::parse(&f).unwrap();
    let index_start = 24 + 48 * 2;
    // Corrupt the second offset to be less than the first.
    let mut bad = f.clone();
    let victim = index_start + 8; // offsets[1]
    bad[victim..victim + 8].copy_from_slice(&0u64.to_le_bytes());
    assert!(matches!(FrontMatter::parse(&bad), Err(ParseError::BadIndex(_))));
    let _ = fm;
}

#[test]
fn wrong_first_offset_rejected() {
    let f = valid_file();
    let index_start = 24 + 48 * 2;
    let mut bad = f.clone();
    // offsets[0] must equal front-matter length; corrupt it.
    bad[index_start..index_start + 8].copy_from_slice(&999_999u64.to_le_bytes());
    assert!(matches!(FrontMatter::parse(&bad), Err(ParseError::BadIndex(_))));
}

#[test]
fn adversarial_tile_count_is_capped_not_allocated() {
    // Hand-build a 24-byte header + 1 descriptor claiming a gigantic pyramid:
    // levels 255, tile_size 1, root u32::MAX. The implied index would be
    // astronomically large; parsing must reject via the sanity cap without
    // trying to allocate it.
    let mut buf = vec![0u8; 24 + 48];
    buf[0..4].copy_from_slice(b"TPCK");
    buf[4] = 1; // version
    buf[5] = 1; // face_count planar
    buf[6] = 255; // levels
    buf[7] = 1; // group_count
    buf[8..10].copy_from_slice(&1u16.to_le_bytes()); // tile_size = 1
    buf[12..16].copy_from_slice(&u32::MAX.to_le_bytes()); // root_w
    buf[16..20].copy_from_slice(&u32::MAX.to_le_bytes()); // root_h
    // descriptor: rgb/webp/rgb8, level_count 255
    buf[24] = 0;
    buf[25] = 1;
    buf[26] = 0;
    buf[28] = 255; // level_count at descriptor offset 4 -> buf[24+4]
    let err = FrontMatter::parse(&buf).unwrap_err();
    assert!(matches!(err, ParseError::Inconsistent(_)), "got {err:?}");
}

#[test]
fn level_skip_overflow_rejected() {
    // valid_file has levels = 2; group 0 covers both. Setting its level_skip
    // to 1 makes skip + count = 3 > levels, which must fail, not wrap.
    let f = valid_file();
    let mut bad = f.clone();
    bad[24 + 5] = 1; // group 0 descriptor, level_skip at offset 5
    assert!(matches!(FrontMatter::parse(&bad), Err(ParseError::Inconsistent(_))));

    // Skip 255 with count 255 in a 255-level header must not underflow either.
    let mut buf = vec![0u8; 24 + 48];
    buf[0..4].copy_from_slice(b"TPCK");
    buf[4] = 1;
    buf[5] = 1;
    buf[6] = 255; // levels
    buf[7] = 1;
    buf[8..10].copy_from_slice(&1u16.to_le_bytes());
    buf[12..16].copy_from_slice(&16u32.to_le_bytes());
    buf[16..20].copy_from_slice(&16u32.to_le_bytes());
    buf[25] = 1; // codec webp
    buf[28] = 255; // level_count
    buf[29] = 255; // level_skip
    assert!(matches!(FrontMatter::parse(&buf), Err(ParseError::Inconsistent(_))));
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    /// Arbitrary bytes must never panic the parser.
    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let _ = FrontMatter::parse(&bytes);
        let _ = required_len(&bytes);
        let _ = TilepackView::new(&bytes);
    }

    /// Mutating a valid file by a single byte must never panic.
    #[test]
    fn single_byte_mutation_never_panics(pos in 0usize..200, val in any::<u8>()) {
        let mut f = valid_file();
        if pos < f.len() {
            f[pos] = val;
        }
        let _ = FrontMatter::parse(&f);
        let _ = TilepackView::new(&f);
    }
}
