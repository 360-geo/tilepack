//! Property test: any valid layout with any subset of present tiles survives
//! Writer -> bytes -> reader byte-identically, with correct ranges.
//!
//! The tile loops index by ordinal deliberately — the same ordinal drives
//! `set_ordinal`, `ordinal_loc`, `tile_ordinal`, and the expected-blob lookup —
//! so the range-loop lint does not apply.
#![allow(clippy::needless_range_loop)]

use proptest::prelude::*;
use tilepack::descriptor::{Codec, GroupDescriptor, GroupFlags, Radiometry, SampleType, Semantic};
use tilepack::layout::Layout;
use tilepack::{FrontMatter, Header, TilepackView, Writer, WriterParams};

/// Smallest `levels` whose coarsest level fits in one tile.
fn levels_for(root_w: u32, root_h: u32, tile_size: u16) -> u8 {
    let mut levels = 1u8;
    let (mut w, mut h) = (root_w, root_h);
    while w.max(h) > tile_size as u32 {
        w = w.div_ceil(2);
        h = h.div_ceil(2);
        levels += 1;
    }
    levels
}

#[derive(Debug, Clone)]
struct Config {
    face_count: u8,
    tile_size: u16,
    root_w: u32,
    root_h: u32,
    groups: Vec<(u8, bool)>, // (level_count_offset_from_full, untiled)
}

fn config_strategy() -> impl Strategy<Value = Config> {
    let tile = prop::sample::select(vec![16u16, 32, 64]);
    (tile, any::<bool>()).prop_flat_map(|(tile_size, cube)| {
        let face_count = if cube { 6u8 } else { 1u8 };
        let dim = 1u32..=(6 * tile_size as u32);
        let dim2 = dim.clone();
        let groups = prop::collection::vec((0u8..3, any::<bool>()), 1..=3);
        (Just(tile_size), Just(face_count), dim, dim2, groups).prop_map(move |(tile_size, face_count, root_w, mut root_h, groups)| {
            if face_count == 6 {
                root_h = root_w;
            }
            Config {
                face_count,
                tile_size,
                root_w,
                root_h,
                groups,
            }
        })
    })
}

fn build(config: &Config) -> (Header, Vec<GroupDescriptor>, u8) {
    let levels = levels_for(config.root_w, config.root_h, config.tile_size);
    let groups: Vec<GroupDescriptor> = config
        .groups
        .iter()
        .map(|&(off, untiled)| {
            let level_count = (levels - (off % levels)).max(1);
            GroupDescriptor {
                semantic: Semantic::Rgb,
                codec: Codec::Webp,
                sample: SampleType::Rgb8,
                flags: GroupFlags::new(untiled, false),
                level_count,
                radiometry: Radiometry::default(),
            }
        })
        .collect();
    let header = Header {
        version: tilepack::VERSION,
        face_count: config.face_count,
        levels,
        group_count: groups.len() as u8,
        tile_size: config.tile_size,
        root_w: config.root_w,
        root_h: config.root_h,
    };
    (header, groups, levels)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    #[test]
    fn writer_reader_roundtrip(config in config_strategy(), seed in any::<u64>()) {
        let (header, groups, levels) = build(&config);
        let layout = Layout::new(header, groups.clone()).unwrap();
        let total = layout.total_tiles() as usize;
        prop_assume!(total <= 20_000); // keep the test fast

        // Deterministic pseudo-random present/absent + blob contents.
        let mut state = seed | 1;
        let mut next = || { state ^= state << 13; state ^= state >> 7; state ^= state << 17; state };

        let params = WriterParams {
            face_count: config.face_count,
            levels,
            tile_size: config.tile_size,
            root_w: config.root_w,
            root_h: config.root_h,
        };
        let mut writer = Writer::new(params, groups).unwrap();

        let mut expected: Vec<Option<Vec<u8>>> = vec![None; total];
        for ord in 0..total {
            let r = next();
            if r % 5 == 0 {
                continue; // absent
            }
            let len = (r >> 8) as usize % 19 + 1;
            let blob: Vec<u8> = (0..len).map(|i| (next() >> (i % 8)) as u8).collect();
            writer.set_ordinal(ord, blob.clone()).unwrap();
            expected[ord] = Some(blob);
        }

        let bytes = writer.finish().unwrap();
        let view = TilepackView::new(&bytes).expect("parse whole file");

        // File length matches the index.
        prop_assert_eq!(view.fm.file_len() as usize, bytes.len());
        prop_assert_eq!(view.fm.layout.total_tiles() as usize, total);

        // Every tile reads back byte-identically; absent stays absent.
        for ord in 0..total {
            let got = view.tile_by_ordinal(ord).map(|s| s.to_vec());
            prop_assert_eq!(&got, &expected[ord], "tile ordinal {}", ord);

            // Location round-trips through ordinal.
            let loc = view.fm.layout.ordinal_loc(ord).unwrap();
            prop_assert_eq!(view.fm.layout.tile_ordinal(loc), Some(ord));
            let via_loc = view.tile(loc).map(|s| s.to_vec());
            prop_assert_eq!(&via_loc, &expected[ord]);
        }

        // Re-parsing front matter alone agrees.
        let fm = FrontMatter::parse(&bytes).unwrap();
        prop_assert_eq!(fm.offsets(), view.fm.offsets());

        // level_range spans exactly its tiles' offsets.
        for g in 0..view.fm.layout.groups().len() {
            for level in view.fm.layout.group_levels(g) {
                let run = view.fm.layout.level_run(g, level).unwrap();
                let lr = view.fm.level_range(g, level).unwrap();
                prop_assert_eq!(lr.start, view.fm.offsets()[run.start]);
                prop_assert_eq!(lr.end, view.fm.offsets()[run.end]);
            }
        }
    }
}
