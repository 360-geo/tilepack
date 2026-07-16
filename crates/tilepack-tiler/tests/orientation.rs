//! End-to-end orientation check against the production cube convention.
//!
//! The equirect is painted so each pixel's RGB encodes its unit direction
//! `(x, y, z)`. After remapping, each face's in-plane gradients must match the
//! convention's `(a, b)` axes: moving +a (column) or +b (row) changes exactly
//! the direction components the `face_dir` table predicts. This catches any
//! mirror or axis swap without needing a human to look at a render.

use std::f64::consts::{PI, TAU};

use tilepack::layout::Face;
use tilepack_tiler::RgbSlab;
use tilepack_tiler::remap::remap_face;

fn enc(v: f64) -> u8 {
    (((v + 1.0) * 0.5 * 255.0).round()).clamp(0.0, 255.0) as u8
}

/// Paint a W x H (2:1) equirect where each pixel encodes its unit direction.
fn painted_equirect(w: u32, h: u32) -> RgbSlab {
    let mut slab = RgbSlab::new(w, h);
    for py in 0..h {
        let lat = PI * (py as f64 + 0.5) / h as f64;
        let (slat, clat) = (lat.sin(), lat.cos());
        for px in 0..w {
            let lon = TAU * (px as f64 + 0.5) / w as f64;
            let x = slat * lon.cos();
            let y = slat * lon.sin();
            let z = clat;
            slab.set_pixel(px, py, [enc(x), enc(y), enc(z)]);
        }
    }
    slab
}

/// For each face: which color channel changes along +column (+a) and +row
/// (+b), and with what sign, per the production `face_dir` table.
/// dir = face_dir(face, a, b); channels are (R,G,B) = (x,y,z).
fn expected(face: Face) -> ((usize, i32), (usize, i32)) {
    match face {
        // front (-1, -a, -b): +a -> y down (G-), +b -> z down (B-)
        Face::Front => ((1, -1), (2, -1)),
        // back (1, a, -b): +a -> y up (G+), +b -> z down (B-)
        Face::Back => ((1, 1), (2, -1)),
        // left (-a, 1, -b): +a -> x down (R-), +b -> z down (B-)
        Face::Left => ((0, -1), (2, -1)),
        // right (a, -1, -b): +a -> x up (R+), +b -> z down (B-)
        Face::Right => ((0, 1), (2, -1)),
        // down (b, -a, -1): +a -> y down (G-), +b -> x up (R+)
        Face::Down => ((1, -1), (0, 1)),
        // up (-b, -a, 1): +a -> y down (G-), +b -> x down (R-)
        Face::Up => ((1, -1), (0, -1)),
    }
}

#[test]
fn face_orientation_matches_production_convention() {
    let eq = painted_equirect(1024, 512);
    let face_size = 256u32;
    let c = face_size / 2;
    let d = face_size / 4;

    for face in Face::ALL {
        let slab = remap_face(&eq, face, face_size);
        let center = slab.pixel(c, c);
        let col_shift = slab.pixel(c + d, c);
        let row_shift = slab.pixel(c, c + d);

        let ((col_ch, col_sign), (row_ch, row_sign)) = expected(face);

        let col_delta = col_shift[col_ch] as i32 - center[col_ch] as i32;
        let row_delta = row_shift[row_ch] as i32 - center[row_ch] as i32;

        assert!(
            col_delta.signum() == col_sign,
            "{face:?}: +column should change channel {col_ch} with sign {col_sign}, got delta {col_delta}",
        );
        assert!(
            row_delta.signum() == row_sign,
            "{face:?}: +row should change channel {row_ch} with sign {row_sign}, got delta {row_delta}",
        );
    }
}

#[test]
fn face_centers_point_at_expected_axes() {
    let eq = painted_equirect(1024, 512);
    let face_size = 256u32;
    let c = face_size / 2;

    // Center of each face points along its constant axis; the encoding of that
    // axis component should be near 0 or 255, the others near 128.
    let checks = [
        (Face::Front, 0usize, 0u8), // -x -> R ~ 0
        (Face::Back, 0, 255),       // +x -> R ~ 255
        (Face::Left, 1, 255),       // +y -> G ~ 255
        (Face::Right, 1, 0),        // -y -> G ~ 0
        (Face::Down, 2, 0),         // -z -> B ~ 0
        (Face::Up, 2, 255),         // +z -> B ~ 255
    ];
    for (face, ch, want) in checks {
        let slab = remap_face(&eq, face, face_size);
        let px = slab.pixel(c, c);
        let diff = (px[ch] as i32 - want as i32).abs();
        assert!(diff < 12, "{face:?}: channel {ch} center = {} want ~{want}", px[ch]);
    }
}
