//! Equirectangular source coordinates for cube-face pixels.
//!
//! Matches the production `dzp` sampling: from a face pixel's `(a, b)`, take
//! the local-frame direction, then `lon = atan2(y, x)` wrapped to `[0, 2pi)`
//! and `lat = acos(z / |d|)`, and map to equirect pixel coordinates with the
//! texel-center offset. The scalar function here is the oracle the SIMD
//! kernel in this module's parent is checked against.

use std::f64::consts::{PI, TAU};

use tilepack::cube::{col_to_a, face_dir, row_to_b};
use tilepack::layout::Face;

/// Equirect source pixel coordinate `(sx, sy)` for one face pixel. `sx` may be
/// outside `[0, eq_w)` and wraps; `sy` is clamped by the sampler.
#[inline]
pub fn face_source_coord(face: Face, a: f64, b: f64, eq_w: u32, eq_h: u32) -> (f32, f32) {
    let [x, y, z] = face_dir(face, a, b);
    let r = (x * x + y * y + z * z).sqrt();
    let lon = y.atan2(x).rem_euclid(TAU);
    let lat = (z / r).clamp(-1.0, 1.0).acos();
    let sx = eq_w as f64 * lon / TAU - 0.5;
    let sy = eq_h as f64 * lat / PI - 0.5;
    (sx as f32, sy as f32)
}

/// Fill one destination row's source coordinates. Scalar reference used by the
/// remapper and as the SIMD parity oracle.
pub fn face_row_coords(face: Face, row: u32, face_size: u32, eq_w: u32, eq_h: u32, sx: &mut [f32], sy: &mut [f32]) {
    let b = row_to_b(row, face_size);
    for col in 0..face_size as usize {
        let a = col_to_a(col as u32, face_size);
        let (x, y) = face_source_coord(face, a, b, eq_w, eq_h);
        sx[col] = x;
        sy[col] = y;
    }
}
