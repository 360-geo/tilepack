//! Brute-force oracle for the canonical tile order and pyramid geometry.
//! Independently enumerates every tile and checks it against `Layout`'s
//! ordinal math, dimension math, and run spans.

use tilepack::descriptor::{Codec, GroupDescriptor, GroupFlags, Radiometry, SampleType, Semantic};
use tilepack::header::{Header, VERSION};
use tilepack::layout::{Face, Layout, TileLoc};

fn header(face_count: u8, levels: u8, tile_size: u16, root_w: u32, root_h: u32, group_count: u8) -> Header {
    Header {
        version: VERSION,
        face_count,
        levels,
        group_count,
        tile_size,
        root_w,
        root_h,
    }
}

fn group(level_count: u8, untiled: bool) -> GroupDescriptor {
    GroupDescriptor {
        semantic: Semantic::Rgb,
        codec: Codec::Webp,
        sample: SampleType::Rgb8,
        flags: GroupFlags::new(untiled, false),
        level_count,
        radiometry: Radiometry::default(),
    }
}

/// Independent enumeration of the canonical order from the spec.
fn brute_order(layout: &Layout) -> Vec<TileLoc> {
    let mut out = Vec::new();
    for g in 0..layout.groups().len() {
        for level in layout.group_levels(g) {
            for face_i in 0..layout.face_count() {
                let face = Face::from_index(face_i as usize).unwrap();
                let (cols, rows) = layout.group_grid(g, level);
                for row in 0..rows {
                    for col in 0..cols {
                        out.push(TileLoc::new(g as u8, level, face, row, col));
                    }
                }
            }
        }
    }
    out
}

fn check(header: Header, groups: Vec<GroupDescriptor>) {
    let layout = Layout::new(header, groups).expect("layout builds");
    let order = brute_order(&layout);

    // Total count agrees.
    assert_eq!(order.len() as u64, layout.total_tiles(), "total tile count");

    // Canonical order is a bijection with ordinals, and ordinal_loc inverts it.
    for (i, &loc) in order.iter().enumerate() {
        assert_eq!(layout.tile_ordinal(loc), Some(i), "ordinal of {loc:?}");
        assert_eq!(layout.ordinal_loc(i), Some(loc), "loc of ordinal {i}");
    }

    // group_run and level_run are contiguous and partition the ordinal space.
    let mut expected_start = 0usize;
    for g in 0..layout.groups().len() {
        let run = layout.group_run(g);
        assert_eq!(run.start, expected_start, "group {g} run start");
        let mut level_start = run.start;
        for level in layout.group_levels(g) {
            let lr = layout.level_run(g, level).expect("covered level has a run");
            assert_eq!(lr.start, level_start, "group {g} level {level} run start");
            // Every ordinal in the level run maps back to this group and level.
            for ord in lr.clone() {
                let loc = layout.ordinal_loc(ord).unwrap();
                assert_eq!(loc.group as usize, g);
                assert_eq!(loc.level, level);
            }
            level_start = lr.end;
        }
        assert_eq!(level_start, run.end, "group {g} levels fill the group run");
        expected_start = run.end;
    }
    assert_eq!(expected_start as u64, layout.total_tiles());

    // front_matter_len matches the formula.
    let expected_fm = 24 + 48 * header.group_count as u64 + 8 * (layout.total_tiles() + 1);
    assert_eq!(layout.front_matter_len(), expected_fm);

    // Levels below the finest halve exactly; tiles tile the level without gaps.
    for g in 0..layout.groups().len() {
        for level in layout.group_levels(g) {
            let (lw, lh) = layout.level_dims(level);
            let (cols, rows) = layout.group_grid(g, level);
            if layout.groups()[g].flags.untiled() {
                assert_eq!((cols, rows), (1, 1));
                assert_eq!(layout.tile_dims(g, level, 0, 0), (lw, lh));
            } else {
                // Sum of tile widths across a row equals the level width.
                let mut wsum = 0u32;
                for col in 0..cols {
                    wsum += layout.tile_dims(g, level, col, 0).0;
                }
                assert_eq!(wsum, lw, "row of tiles covers level width at level {level}");
                let mut hsum = 0u32;
                for row in 0..rows {
                    hsum += layout.tile_dims(g, level, 0, row).1;
                }
                assert_eq!(hsum, lh, "column of tiles covers level height at level {level}");
            }
        }
    }
}

#[test]
fn cubemap_power_of_two() {
    // 4096 face, tile 512 -> 4 levels: 512,1024,2048,4096.
    check(header(6, 4, 512, 4096, 4096, 1), vec![group(4, false)]);
}

#[test]
fn cubemap_multigroup_partial_levels() {
    // RGB full pyramid + depth single finest level, untiled.
    let h = header(6, 4, 512, 4096, 4096, 2);
    check(h, vec![group(4, false), group(1, true)]);
}

#[test]
fn planar_non_power_of_two() {
    // A 5000x3000 oblique-ish planar image, tile 512.
    // levels: ceil(log2(5000/512)) + 1 = ceil(3.28)+1 = 5 -> coarsest 313x188.
    check(header(1, 5, 512, 5000, 3000, 1), vec![group(5, false)]);
}

#[test]
fn planar_multigroup_rgb_nir() {
    let h = header(1, 5, 512, 5000, 3000, 2);
    check(h, vec![group(5, false), group(3, false)]);
}

#[test]
fn cubemap_non_power_of_two_face() {
    // Deliberately not tile_size * 2^k; legal (MUST is coarsest <= tile_size).
    // 3000 face, tile 512: levels ceil(log2(3000/512))+1 = ceil(2.55)+1 = 4
    // coarsest 375 <= 512. ok.
    check(header(6, 4, 512, 3000, 3000, 1), vec![group(4, false)]);
}

#[test]
fn tiny_single_level() {
    check(header(1, 1, 512, 300, 200, 1), vec![group(1, false)]);
    check(header(6, 1, 512, 400, 400, 1), vec![group(1, false)]);
}
