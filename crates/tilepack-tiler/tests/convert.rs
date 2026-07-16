//! The panorama converter produces a valid, parseable cubemap tilepack whose
//! tiles decode to the dimensions the header promises.

use tilepack::layout::{Face, TileLoc};
use tilepack::{Codec, TilepackView};
use tilepack_tiler::{PanoOptions, RgbSlab, convert_equirect};

/// A cheap non-uniform equirect so encoded tiles are non-trivial.
fn gradient_equirect(w: u32, h: u32) -> RgbSlab {
    let mut slab = RgbSlab::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let r = (x * 255 / w) as u8;
            let g = (y * 255 / h) as u8;
            let b = ((x + y) * 255 / (w + h)) as u8;
            slab.set_pixel(x, y, [r, g, b]);
        }
    }
    slab
}

#[test]
fn convert_produces_valid_tilepack() {
    let eq = gradient_equirect(2048, 1024);
    let opts = PanoOptions {
        tile_size: 256,
        quality: 80.0,
        face_size: Some(512),
    };
    let tpc = convert_equirect(&eq, &opts).expect("convert");

    let view = TilepackView::new(&tpc).expect("parse output");
    let h = &view.fm.header;
    assert_eq!(h.face_count, 6);
    assert_eq!(h.tile_size, 256);
    assert_eq!((h.root_w, h.root_h), (512, 512));
    // 512 face, tile 256 -> levels: 512, 256 = 2.
    assert_eq!(h.levels, 2);
    assert_eq!(view.fm.layout.groups()[0].codec, Codec::Webp);

    // Every tile is present and decodes to its header-implied dimensions.
    let layout = &view.fm.layout;
    for face in Face::ALL {
        for level in layout.group_levels(0) {
            let (cols, rows) = layout.grid(level);
            for row in 0..rows {
                for col in 0..cols {
                    let loc = TileLoc::new(0, level, face, row, col);
                    let blob = view.tile(loc).expect("tile present");
                    assert_eq!(&blob[0..4], b"RIFF", "tile is webp");

                    let decoded = webp::Decoder::new(blob).decode().expect("decode webp tile");
                    let (want_w, want_h) = layout.tile_dims(0, level, col, row);
                    assert_eq!(
                        (decoded.width(), decoded.height()),
                        (want_w, want_h),
                        "{face:?} L{level} {col},{row}"
                    );
                }
            }
        }
    }
}
