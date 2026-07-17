//! The golden fixture must parse to a known structure and its bytes must be
//! stable across releases. Regenerate with
//! `cargo run -p tilepack --example gen_golden` and review any diff.

use tilepack::descriptor::{Codec, GroupDescriptor, GroupFlags, Radiometry, SampleType, Semantic};
use tilepack::{TilepackView, Writer, WriterParams};

const GOLDEN: &[u8] = include_bytes!("../../../fixtures/golden.tpc");

/// Must stay identical to `examples/gen_golden.rs`.
fn golden_bytes() -> Vec<u8> {
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
            level_skip: 1,
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
        if ord % 5 == 4 {
            continue;
        }
        let blob: Vec<u8> = (0..=(ord as u8 % 11)).map(|i| i.wrapping_mul(37).wrapping_add(ord as u8)).collect();
        w.set_ordinal(ord, blob).unwrap();
    }
    w.finish().unwrap()
}

#[test]
fn golden_bytes_are_stable() {
    assert_eq!(golden_bytes(), GOLDEN, "writer output drifted from the golden fixture");
}

#[test]
fn golden_parses_to_expected_structure() {
    let view = TilepackView::new(GOLDEN).unwrap();
    let h = &view.fm.header;
    assert_eq!(h.face_count, 6);
    assert_eq!(h.levels, 2);
    assert_eq!(h.group_count, 2);
    assert_eq!(h.tile_size, 64);
    assert_eq!((h.root_w, h.root_h), (128, 128));

    let groups = view.fm.layout.groups();
    assert_eq!(groups[0].semantic, Semantic::Rgb);
    assert_eq!(groups[0].codec, Codec::Webp);
    assert_eq!(groups[0].level_count, 2);
    assert!(!groups[0].flags.untiled());

    assert_eq!(groups[1].semantic, Semantic::Depth);
    assert_eq!(groups[1].codec, Codec::Depthpack);
    assert_eq!(groups[1].level_count, 1);
    assert_eq!(groups[1].level_skip, 1);
    assert_eq!(view.fm.layout.group_levels(1), 0..1, "depth anchored at the coarse level");
    assert!(groups[1].flags.untiled());
    assert_eq!(groups[1].radiometry.scale, 0.001);
    assert_eq!(groups[1].radiometry.unit_str(), Some("m"));

    // RGB group: level 0 (coarse) is 6 faces * 1 tile; level 1 (fine) is
    // 6 faces * 4 tiles (128/64 = 2x2). Depth group: 6 faces * 1 untiled at
    // level 0. Total = 6 + 24 + 6 = 36.
    assert_eq!(view.fm.layout.total_tiles(), 36);

    // Every 5th tile is absent by construction.
    let total = view.fm.layout.total_tiles() as usize;
    for ord in 0..total {
        let present = view.tile_by_ordinal(ord).is_some();
        assert_eq!(present, ord % 5 != 4, "presence at ordinal {ord}");
    }
}
