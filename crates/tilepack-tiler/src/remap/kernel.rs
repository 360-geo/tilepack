//! SIMD remap kernels, one generic body dispatched per `fearless_simd` level:
//! 16 `f32` lanes on AVX-512, 8 on AVX2, 4 on SSE and NEON.
//!
//! Lanes are `f32` where the scalar oracle in [`coords`](super::coords) is
//! `f64`. That costs about one f32 ulp of the coordinate: 0.001 px at 8192 px
//! wide, 0.004 px at 32768. The tests in this module hold it to that.
//!
//! Every function a kernel calls must stay `#[inline(always)]`: `dispatch!`
//! enables the target features on its own body only.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use fearless_simd::{Level, Simd, dispatch, prelude::*};
use tilepack::cube::{col_to_a, face_dir, row_to_b};
use tilepack::layout::Face;

use super::coords::face_source_coord;

#[cfg(feature = "convert")]
use super::sample_bilinear;
#[cfg(feature = "convert")]
use crate::slab::RgbSlab;

const TAN_PI_8: f32 = 0.414_213_57;
const TAN_3PI_8: f32 = 2.414_213_7;

/// Fill one destination row's source coordinates. The SIMD counterpart of
/// [`face_row_coords`](super::coords::face_row_coords).
pub fn face_row_coords_simd(face: Face, row: u32, face_size: u32, eq_w: u32, eq_h: u32, sx: &mut [f32], sy: &mut [f32]) {
    row_coords_at(Level::new(), face, row, face_size, eq_w, eq_h, sx, sy);
}

#[allow(clippy::too_many_arguments)]
fn row_coords_at(level: Level, face: Face, row: u32, face_size: u32, eq_w: u32, eq_h: u32, sx: &mut [f32], sy: &mut [f32]) {
    dispatch!(level, simd => row_coords(simd, face, row, face_size, eq_w, eq_h, sx, sy));
}

/// Sample one destination row of RGB at the given source coordinates.
#[cfg(feature = "convert")]
pub fn sample_row(eq: &RgbSlab, sx: &[f32], sy: &[f32], dst: &mut [u8]) {
    sample_row_at(Level::new(), eq, sx, sy, dst);
}

#[cfg(feature = "convert")]
fn sample_row_at(level: Level, eq: &RgbSlab, sx: &[f32], sy: &[f32], dst: &mut [u8]) {
    dispatch!(level, simd => sample(simd, eq, sx, sy, dst));
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn row_coords<S: Simd>(simd: S, face: Face, row: u32, face_size: u32, eq_w: u32, eq_h: u32, sx: &mut [f32], sy: &mut [f32]) {
    let lanes = S::f32s::LEN;
    let b = row_to_b(row, face_size);

    // Within a row the direction is affine in `a`. Read it off `face_dir` so
    // cube.rs stays the only encoding of the face convention.
    let origin = face_dir(face, 0.0, b).map(|v| v as f32);
    let [dx, dy, dz] = {
        let (o, u) = (face_dir(face, 0.0, b), face_dir(face, 1.0, b));
        [u[0] - o[0], u[1] - o[1], u[2] - o[2]].map(|v| v as f32)
    };
    // Measure `a` from the face center: a half-integer offset is exact, so `a`
    // keeps its relative precision near zero, where the pole's longitude needs it.
    let half = face_size as f32 / 2.0;
    let a_step = (2.0 / face_size as f64) as f32;

    let to_sx = eq_w as f32 / TAU;
    let to_sy = eq_h as f32 / PI;
    let mut centered = S::f32s::from_fn(simd, |i| i as f32 + 0.5 - half);

    for (out_x, out_y) in sx.chunks_exact_mut(lanes).zip(sy.chunks_exact_mut(lanes)) {
        let a = centered * a_step;
        let x = a.mul_add(dx, origin[0]);
        let y = a.mul_add(dy, origin[1]);
        let z = a.mul_add(dz, origin[2]);

        let lon = atan2(simd, y, x);
        let lon = lon.simd_lt(0.0).select(lon + TAU, lon);
        // The scalar oracle's `acos(z / r)` loses f32 precision at the poles;
        // this form stays conditioned everywhere.
        let lat = atan2(simd, (x * x + y * y).sqrt(), z);

        (lon * to_sx - 0.5).store_slice(out_x);
        (lat * to_sy - 0.5).store_slice(out_y);
        centered += lanes as f32;
    }

    let done = sx.len() - sx.len() % lanes;
    for col in done..sx.len() {
        let (x, y) = face_source_coord(face, col_to_a(col as u32, face_size), b, eq_w, eq_h);
        sx[col] = x;
        sy[col] = y;
    }
}

/// Four-quadrant arctangent, `(-pi, pi]`. Octant reduction selects numerator
/// and denominator so all three branches share one division; the polynomial
/// is Cephes `atanf`.
#[inline(always)]
fn atan2<S: Simd>(simd: S, y: S::f32s, x: S::f32s) -> S::f32s {
    let (ax, ay) = (x.abs(), y.abs());
    let high = ay.simd_gt(ax * TAN_3PI_8);
    let mid = ay.simd_gt(ax * TAN_PI_8);

    let num = high.select(-ax, mid.select(ay - ax, ay));
    // Only `x == y == 0` reaches a zero denominator; the floor yields 0, not NaN.
    let den = high.select(ay, mid.select(ay + ax, ax)).max(f32::MIN_POSITIVE);
    let base = high.select(
        S::f32s::splat(simd, FRAC_PI_2),
        mid.select(S::f32s::splat(simd, FRAC_PI_4), S::f32s::splat(simd, 0.0)),
    );

    let t = num / den;
    let t2 = t * t;
    let poly = (t2 * 8.053_744_5e-2 - 1.387_768_6e-1)
        .mul_add(t2, 1.997_771_1e-1)
        .mul_add(t2, -3.333_295e-1);
    let octant = base + (poly * t2).mul_add(t, t);

    x.simd_lt(0.0).select(-octant + PI, octant).copysign(y)
}

#[cfg(feature = "convert")]
#[inline(always)]
fn sample<S: Simd>(simd: S, eq: &RgbSlab, sx: &[f32], sy: &[f32], dst: &mut [u8]) {
    let lanes = S::f32s::LEN;
    let stride = eq.w as usize * 3;
    let (w, h) = (eq.w as i32, eq.h as i32);

    for ((xs, ys), out) in sx
        .chunks_exact(lanes)
        .zip(sy.chunks_exact(lanes))
        .zip(dst.chunks_exact_mut(lanes * 3))
    {
        let (x, y) = (S::f32s::from_slice(simd, xs), S::f32s::from_slice(simd, ys));
        let (x_floor, y_floor) = (x.floor(), y.floor());
        let (fx, fy) = (x - x_floor, y - y_floor);
        let x0: S::i32s = x_floor.to_int();
        let y0: S::i32s = y_floor.to_int();

        // `sx` lies within one pixel of `[0, w)`, so a conditional wrap
        // replaces the scalar sampler's integer division.
        let xa = x0.simd_lt(0).select(x0 + w, x0);
        let xa = xa.simd_ge(w).select(xa - w, xa);
        let xb = xa + 1;
        let xb = xb.simd_ge(w).select(xb - w, xb);
        let ya = y0.max(0).min(h - 1);
        let yb = (y0 + 1).max(0).min(h - 1);

        let mut corners = [S::i32s::splat(simd, 0); 4];
        for i in 0..lanes {
            let (ra, rb) = (ya.as_slice()[i] as usize * stride, yb.as_slice()[i] as usize * stride);
            let (ca, cb) = (xa.as_slice()[i] as usize * 3, xb.as_slice()[i] as usize * 3);
            corners[0].as_mut_slice()[i] = texel(&eq.data, ra + ca);
            corners[1].as_mut_slice()[i] = texel(&eq.data, ra + cb);
            corners[2].as_mut_slice()[i] = texel(&eq.data, rb + ca);
            corners[3].as_mut_slice()[i] = texel(&eq.data, rb + cb);
        }

        let r = blend_channel::<S>(corners, 0, fx, fy);
        let g = blend_channel::<S>(corners, 8, fx, fy);
        let b = blend_channel::<S>(corners, 16, fx, fy);
        let packed = r | (g << 8) | (b << 16);
        for (px, v) in out.chunks_exact_mut(3).zip(packed.as_slice()) {
            px.copy_from_slice(&v.to_le_bytes()[..3]);
        }
    }

    let done = sx.len() - sx.len() % lanes;
    for i in done..sx.len() {
        dst[i * 3..i * 3 + 3].copy_from_slice(&sample_bilinear(eq, sx[i], sy[i]));
    }
}

/// One RGB pixel packed into an `i32`, red in the low byte. The top byte is
/// whatever follows blue; [`blend_channel`] masks it off. Only the image's
/// last texel has nothing to follow it.
#[cfg(feature = "convert")]
#[inline(always)]
fn texel(data: &[u8], offset: usize) -> i32 {
    match data[offset..].first_chunk::<4>() {
        Some(px) => i32::from_le_bytes(*px),
        None => data[offset] as i32 | (data[offset + 1] as i32) << 8 | (data[offset + 2] as i32) << 16,
    }
}

#[cfg(feature = "convert")]
#[inline(always)]
fn blend_channel<S: Simd>(corners: [S::i32s; 4], shift: u32, fx: S::f32s, fy: S::f32s) -> S::i32s {
    let channel = |v: S::i32s| -> S::f32s { ((v >> shift) & 0xFF).to_float() };
    let lerp = |a: S::f32s, b: S::f32s, t: S::f32s| (b - a).mul_add(t, a);
    let top = lerp(channel(corners[0]), channel(corners[1]), fx);
    let bot = lerp(channel(corners[2]), channel(corners[3]), fx);
    // Non-negative, so truncating after the offset rounds to nearest.
    (lerp(top, bot, fy) + 0.5).to_int()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remap::coords::face_row_coords;
    use crate::remap::{remap_face, remap_face_scalar};

    /// Every level this machine can run, best first.
    fn levels() -> Vec<Level> {
        let best = Level::new();
        #[cfg(target_arch = "x86_64")]
        let lower = [
            best.as_avx2().map(Level::Avx2),
            best.as_sse4_2().map(Level::Sse4_2),
            best.as_sse2().map(Level::Sse2),
        ];
        #[cfg(not(target_arch = "x86_64"))]
        let lower: [Option<Level>; 0] = [];
        std::iter::once(best).chain(lower.into_iter().flatten()).collect()
    }

    #[inline(always)]
    fn atan2_slice<S: Simd>(simd: S, y: &[f32], x: &[f32], out: &mut [f32]) {
        let lanes = S::f32s::LEN;
        for ((ys, xs), o) in y.chunks_exact(lanes).zip(x.chunks_exact(lanes)).zip(out.chunks_exact_mut(lanes)) {
            atan2(simd, S::f32s::from_slice(simd, ys), S::f32s::from_slice(simd, xs)).store_slice(o);
        }
    }

    #[test]
    fn atan2_matches_libm_around_the_circle() {
        // Round the circle at several radii, plus every axis and diagonal.
        let mut pts = Vec::new();
        for i in 0..1600 {
            let t = i as f64 / 1600.0 * std::f64::consts::TAU;
            for r in [1e-4, 0.1, 1.0, 3.0] {
                pts.push(((r * t.sin()) as f32, (r * t.cos()) as f32));
            }
        }
        pts.extend([
            (0.0, 1.0),
            (0.0, -1.0),
            (-0.0, -1.0),
            (1.0, 0.0),
            (-1.0, 0.0),
            (1.0, 1.0),
            (-1.0, -1.0),
            (1.0, -1.0),
        ]);
        pts.resize(pts.len().next_multiple_of(16), (0.5, 0.5));
        let (y, x): (Vec<f32>, Vec<f32>) = pts.iter().copied().unzip();

        for level in levels() {
            let mut got = vec![f32::NAN; y.len()];
            dispatch!(level, simd => atan2_slice(simd, &y, &x, &mut got));
            for i in 0..y.len() {
                let want = (y[i] as f64).atan2(x[i] as f64);
                let err = (got[i] as f64 - want).abs();
                assert!(
                    err < 4e-7,
                    "{level:?} atan2({}, {}) = {} want {want}, err {err}",
                    y[i],
                    x[i],
                    got[i]
                );
            }
        }
    }

    #[test]
    fn atan2_of_the_origin_is_finite() {
        for level in levels() {
            let (y, x) = ([0.0f32; 16], [0.0f32; 16]);
            let mut got = [f32::NAN; 16];
            dispatch!(level, simd => atan2_slice(simd, &y, &x, &mut got));
            assert!(got.iter().all(|v| v.is_finite()), "{level:?}: {got:?}");
        }
    }

    #[test]
    fn coords_match_the_scalar_oracle() {
        for level in levels() {
            for (eq_w, face_size) in [(64u32, 3u32), (64, 17), (1024, 256), (8192, 2048), (16384, 4097), (32768, 8192)] {
                let eq_h = eq_w / 2;
                let n = face_size as usize;
                let step = (face_size / 64).max(1);
                let (mut sx, mut sy) = (vec![0f32; n], vec![0f32; n]);
                let (mut want_x, mut want_y) = (vec![0f32; n], vec![0f32; n]);
                let tolerance = 1e-3 + eq_w as f32 * 3e-7;

                for face in Face::ALL {
                    for row in (0..face_size).step_by(step as usize).chain([face_size / 2, face_size - 1]) {
                        row_coords_at(level, face, row, face_size, eq_w, eq_h, &mut sx, &mut sy);
                        face_row_coords(face, row, face_size, eq_w, eq_h, &mut want_x, &mut want_y);
                        for col in 0..n {
                            let [x, y, _] = face_dir(face, col_to_a(col as u32, face_size), row_to_b(row, face_size));
                            assert!(sx[col].is_finite() && sy[col].is_finite(), "{face:?} {row},{col}");
                            let dy = (sy[col] - want_y[col]).abs();
                            assert!(dy < tolerance, "{level:?} {face:?} {eq_w}/{face_size} {row},{col}: sy err {dy}");
                            // Longitude is undefined exactly on the pole axis.
                            if x == 0.0 && y == 0.0 {
                                continue;
                            }
                            let wrapped = (sx[col] - want_x[col]).rem_euclid(eq_w as f32);
                            let dx = wrapped.min(eq_w as f32 - wrapped);
                            assert!(dx < tolerance, "{level:?} {face:?} {eq_w}/{face_size} {row},{col}: sx err {dx}");
                        }
                    }
                }
            }
        }
    }

    /// Noise equirect: every texel differs from its neighbours.
    fn noise_equirect(w: u32, h: u32) -> RgbSlab {
        let mut state = 0x2545_F491u32;
        let mut data = vec![0u8; (w * h * 3) as usize];
        for v in &mut data {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *v = state as u8;
        }
        RgbSlab::from_data(w, h, data)
    }

    #[test]
    fn sample_matches_scalar_including_seam_and_poles() {
        let (w, h) = (64u32, 32u32);
        let eq = noise_equirect(w, h);
        let mut state = 0x9E37_79B9u32;
        let mut unit = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as f32 / u32::MAX as f32
        };
        // A length of 67 leaves a tail on every level.
        let mut sx: Vec<f32> = (0..67).map(|_| unit() * w as f32 - 0.5).collect();
        let mut sy: Vec<f32> = (0..67).map(|_| unit() * h as f32 - 0.5).collect();
        sx[..6].copy_from_slice(&[-0.5, -0.25, w as f32 - 1.0, w as f32 - 0.5, 0.0, 12.0]);
        sy[..6].copy_from_slice(&[-0.5, h as f32 - 0.5, 3.0, 4.5, -0.25, h as f32 - 1.0]);

        for level in levels() {
            let mut got = vec![0u8; 67 * 3];
            sample_row_at(level, &eq, &sx, &sy, &mut got);
            for (i, px) in got.chunks_exact(3).enumerate() {
                let want = sample_bilinear(&eq, sx[i], sy[i]);
                for c in 0..3 {
                    assert!(
                        px[c].abs_diff(want[c]) <= 1,
                        "{level:?} px {i} ({}, {}): {px:?} want {want:?}",
                        sx[i],
                        sy[i]
                    );
                }
            }
        }
    }

    #[test]
    fn remapped_faces_match_the_scalar_path() {
        let eq = noise_equirect(512, 256);
        for face_size in [64, 65, 128] {
            for face in Face::ALL {
                let (mut got, mut want) = (remap_face(&eq, face, face_size), remap_face_scalar(&eq, face, face_size));
                // An odd face has a pixel exactly on the pole axis, where longitude
                // is undefined and either answer is right.
                if face_size % 2 == 1 && matches!(face, Face::Up | Face::Down) {
                    let c = face_size / 2;
                    got.set_pixel(c, c, [0; 3]);
                    want.set_pixel(c, c, [0; 3]);
                }
                let worst = got.data.iter().zip(&want.data).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
                assert!(worst <= 2, "{face:?} {face_size}: worst channel diff {worst}");
            }
        }
    }
}
