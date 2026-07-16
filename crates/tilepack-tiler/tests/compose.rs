//! Composition: merging a same-geometry sibling and stripping finest levels,
//! both without re-encoding.

use tilepack::layout::{Face, TileLoc};
use tilepack::{Semantic, TilepackView};
use tilepack_tiler::RgbSlab;
use tilepack_tiler::{
    PlanarOptions, Radiometrics, RasterOptions, U16Slab, convert_planar, convert_raster_split16, merge_groups, strip_finest_levels,
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
