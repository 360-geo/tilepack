//! Equirectangular panorama to cube-face remapping.

pub mod coords;

use rayon::prelude::*;
use tilepack::layout::Face;

use crate::slab::RgbSlab;
use coords::face_row_coords;

/// Bilinear sample of an equirectangular image. Longitude wraps at the seam;
/// latitude clamps at the poles.
#[inline]
fn sample_bilinear(eq: &RgbSlab, sx: f32, sy: f32) -> [u8; 3] {
    let w = eq.w as i64;
    let h = eq.h as i64;

    let x0f = sx.floor();
    let y0f = sy.floor();
    let fx = sx - x0f;
    let fy = sy - y0f;
    let x0 = x0f as i64;
    let y0 = y0f as i64;

    let xa = x0.rem_euclid(w) as u32;
    let xb = (x0 + 1).rem_euclid(w) as u32;
    let ya = y0.clamp(0, h - 1) as u32;
    let yb = (y0 + 1).clamp(0, h - 1) as u32;

    let p00 = eq.pixel(xa, ya);
    let p10 = eq.pixel(xb, ya);
    let p01 = eq.pixel(xa, yb);
    let p11 = eq.pixel(xb, yb);

    let mut out = [0u8; 3];
    for c in 0..3 {
        let top = p00[c] as f32 * (1.0 - fx) + p10[c] as f32 * fx;
        let bot = p01[c] as f32 * (1.0 - fx) + p11[c] as f32 * fx;
        out[c] = (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// Render one cube face at `face_size x face_size` by sampling the equirect.
/// Rows are processed in parallel.
pub fn remap_face(eq: &RgbSlab, face: Face, face_size: u32) -> RgbSlab {
    let mut out = RgbSlab::new(face_size, face_size);
    let row_stride = face_size as usize * 3;

    out.data.par_chunks_mut(row_stride).enumerate().for_each(|(row, dst)| {
        let mut sx = vec![0f32; face_size as usize];
        let mut sy = vec![0f32; face_size as usize];
        face_row_coords(face, row as u32, face_size, eq.w, eq.h, &mut sx, &mut sy);
        for col in 0..face_size as usize {
            let rgb = sample_bilinear(eq, sx[col], sy[col]);
            dst[col * 3] = rgb[0];
            dst[col * 3 + 1] = rgb[1];
            dst[col * 3 + 2] = rgb[2];
        }
    });

    out
}
