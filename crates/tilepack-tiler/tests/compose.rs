//! Composition: merging a same-geometry sibling, merging a lower-resolution
//! depth sibling onto its pyramid level, and stripping finest levels — all
//! without re-encoding.

use tilepack::layout::{Face, TileLoc};
use tilepack::{Semantic, TilepackView};
use tilepack_tiler::RgbSlab;
use tilepack_tiler::{
    DepthOptions, PanoOptions, PlanarOptions, Radiometrics, RasterOptions, U16Slab, convert_depth_cubemap, convert_equirect,
    convert_planar, convert_raster_split16, merge_groups, nearest_level_face_size, strip_finest_levels,
};

fn rgb(w: u32, h: u32) -> RgbSlab {
    let mut s = RgbSlab::new(w, h);
    for y in 0..h {
        for x in 0..w {
            s.set_pixel(x, y, [(x % 256) as u8, (y % 256) as u8, 100]);
        }
    }
    s
}

fn nir(w: u32, h: u32) -> U16Slab {
    let mut s = U16Slab::new(w, h);
    for (i, v) in s.data.iter_mut().enumerate() {
        *v = (i as u16).wrapping_mul(7).max(1);
    }
    s
}

#[test]
fn merge_rgb_and_nir_siblings() {
    let w = 400;
    let h = 300;
    let ts = 128;
    let rgb_tpc = convert_planar(
        &rgb(w, h),
        &PlanarOptions {
            tile_size: ts,
            quality: 80.0,
        },
    )
    .unwrap();
    let nir_tpc = convert_raster_split16(
        &nir(w, h),
        &RasterOptions {
            tile_size: ts,
            semantic: Semantic::Nir,
            radiometry: Radiometrics {
                scale: 1.0,
                offset: 0.0,
                unit: "".into(),
                nodata: 0,
                min: 0,
                max: 65535,
            },
            gray8: tilepack_tiler::Gray8Encoding::Lossless,
        },
    )
    .unwrap();

    let merged = merge_groups(&rgb_tpc, &nir_tpc).unwrap();
    let mv = TilepackView::new(&merged).unwrap();
    assert_eq!(mv.fm.header.group_count, 2);
    assert_eq!(mv.fm.layout.groups()[0].semantic, Semantic::Rgb);
    assert_eq!(mv.fm.layout.groups()[1].semantic, Semantic::Nir);

    // Group 0 tiles equal the original RGB file's tiles; group 1 equals NIR's.
    let rv = TilepackView::new(&rgb_tpc).unwrap();
    let nv = TilepackView::new(&nir_tpc).unwrap();
    for level in mv.fm.layout.group_levels(0) {
        let (cols, rows) = mv.fm.layout.grid(level);
        for row in 0..rows {
            for col in 0..cols {
                let loc = TileLoc::new(0, level, Face::Front, row, col);
                assert_eq!(mv.tile(loc), rv.tile(loc), "rgb tile {level} {col},{row}");
                let nloc = TileLoc::new(1, level, Face::Front, row, col);
                let n0 = TileLoc::new(0, level, Face::Front, row, col);
                assert_eq!(mv.tile(nloc), nv.tile(n0), "nir tile {level} {col},{row}");
            }
        }
    }
}

/// The production pano shape: an RGB cubemap pyramid plus a depth field at a
/// quarter of the RGB face size, merged into one file. The depth group must
/// re-anchor onto the matching pyramid level via level_skip, blobs verbatim.
#[test]
fn merge_lower_resolution_depth_into_rgb_pano() {
    // 1024x512 equirect -> 256px RGB faces, tile 128 -> 2 levels (128, 256).
    let mut eq = RgbSlab::new(1024, 512);
    for y in 0..512 {
        for x in 0..1024 {
            eq.set_pixel(x, y, [(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8]);
        }
    }
    let rgb_tpc = convert_equirect(
        &eq,
        &PanoOptions {
            tile_size: 128,
            quality: 80.0,
            face_size: Some(256),
        },
    )
    .unwrap();
    let rv = TilepackView::new(&rgb_tpc).unwrap();
    assert_eq!(rv.fm.header.levels, 2);

    // Depth at half the RGB resolution: native ~130 px faces snap to the
    // 128 px pyramid level.
    let mut deq = U16Slab::new(1024, 512);
    for (i, v) in deq.data.iter_mut().enumerate() {
        *v = ((i * 13) % 60000) as u16 + 1;
    }
    let face = nearest_level_face_size(256, 2, 130);
    assert_eq!(face, 128);
    let depth_tpc = convert_depth_cubemap(&deq, face, &DepthOptions::default()).unwrap();

    let merged = merge_groups(&rgb_tpc, &depth_tpc).unwrap();
    let mv = TilepackView::new(&merged).unwrap();

    // Header keeps the primary geometry; the depth group re-anchored.
    assert_eq!(mv.fm.header.levels, 2);
    assert_eq!((mv.fm.header.root_w, mv.fm.header.root_h), (256, 256));
    assert_eq!(mv.fm.header.group_count, 2);
    let depth = &mv.fm.layout.groups()[1];
    assert_eq!(depth.semantic, Semantic::Depth);
    assert_eq!(depth.level_count, 1);
    assert_eq!(depth.level_skip, 1, "depth anchors one level below the finest");
    assert_eq!(mv.fm.layout.group_levels(1), 0..1);
    assert_eq!(mv.fm.layout.level_dims(0), (128, 128));

    // RGB tiles verbatim at their original locations.
    for level in mv.fm.layout.group_levels(0) {
        let (cols, rows) = mv.fm.layout.grid(level);
        for face_i in 0..6 {
            let f = Face::from_index(face_i).unwrap();
            for row in 0..rows {
                for col in 0..cols {
                    let loc = TileLoc::new(0, level, f, row, col);
                    assert_eq!(mv.tile(loc), rv.tile(loc), "rgb tile L{level} f{face_i} {col},{row}");
                }
            }
        }
    }
    // Depth blobs verbatim: merged (group 1, level 0) equals sibling's
    // (group 0, level 0), face by face.
    let dv = TilepackView::new(&depth_tpc).unwrap();
    for face_i in 0..6 {
        let f = Face::from_index(face_i).unwrap();
        let got = mv.tile(TileLoc::new(1, 0, f, 0, 0));
        let want = dv.tile(TileLoc::new(0, 0, f, 0, 0));
        assert!(want.is_some(), "sibling depth face {face_i} present");
        assert_eq!(got, want, "depth face {face_i} verbatim");
    }

    // A depth sibling whose resolution matches no pyramid level is refused.
    let bad_depth = convert_depth_cubemap(&deq, 100, &DepthOptions::default()).unwrap();
    assert!(merge_groups(&rgb_tpc, &bad_depth).is_err());

    // Stripping the finest RGB level leaves depth at what is now the finest
    // level: its window is untouched, only its skip shrinks.
    let stripped = strip_finest_levels(&merged, 1).unwrap();
    let sv = TilepackView::new(&stripped).unwrap();
    assert_eq!(sv.fm.header.levels, 1);
    assert_eq!(sv.fm.header.group_count, 2);
    let sdepth = &sv.fm.layout.groups()[1];
    assert_eq!((sdepth.level_count, sdepth.level_skip), (1, 0));
    for face_i in 0..6 {
        let f = Face::from_index(face_i).unwrap();
        let got = sv.tile(TileLoc::new(1, 0, f, 0, 0));
        let want = dv.tile(TileLoc::new(0, 0, f, 0, 0));
        assert_eq!(got, want, "stripped depth face {face_i} verbatim");
    }
}

#[test]
fn strip_finest_level_keeps_coarse_tiles_verbatim() {
    let w = 1000;
    let h = 800;
    let ts = 256;
    let full = convert_planar(
        &rgb(w, h),
        &PlanarOptions {
            tile_size: ts,
            quality: 80.0,
        },
    )
    .unwrap();
    let fv = TilepackView::new(&full).unwrap();
    let full_levels = fv.fm.header.levels;

    let stripped = strip_finest_levels(&full, 1).unwrap();
    let sv = TilepackView::new(&stripped).unwrap();
    assert_eq!(sv.fm.header.levels, full_levels - 1);
    // New root is the old second-finest level's size.
    let (nw, nh) = fv.fm.layout.level_dims(full_levels - 2);
    assert_eq!((sv.fm.header.root_w, sv.fm.header.root_h), (nw, nh));

    // Every retained level's tiles are byte-identical to the original.
    for level in 0..sv.fm.header.levels {
        let (cols, rows) = sv.fm.layout.grid(level);
        for row in 0..rows {
            for col in 0..cols {
                let loc = TileLoc::new(0, level, Face::Front, row, col);
                assert_eq!(sv.tile(loc), fv.tile(loc), "coarse tile L{level} {col},{row} must be verbatim");
            }
        }
    }
}
